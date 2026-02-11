/// Document store with incremental sync via tree-sitter.
///
/// Each open document maintains:
/// - The current source text
/// - A line index for fast offset <-> position conversion
/// - A tree-sitter `Tree` that is incrementally updated on edits
/// - A per-document `JsonParser` instance
use std::collections::HashMap;

use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
use tower_lsp::lsp_types::{Position, Range, Url};
use tree_sitter::Tree;

use crate::tree::{self, JsonParser};

// ---------------------------------------------------------------------------
// Helpers: line-index <-> LSP type conversion
// ---------------------------------------------------------------------------

#[inline]
fn to_lsp_position(index: &LineIndex, offset: line_index::TextSize) -> Position {
    let line_col = index.line_col(offset);
    let wide = index
        .to_wide(WideEncoding::Utf16, line_col)
        .unwrap_or(WideLineCol {
            line: line_col.line,
            col: line_col.col,
        });
    Position {
        line: wide.line,
        character: wide.col,
    }
}

#[inline]
fn from_lsp_position(index: &LineIndex, pos: Position) -> line_index::TextSize {
    let wide = WideLineCol {
        line: pos.line,
        col: pos.character,
    };
    let line_col = index.to_utf8(WideEncoding::Utf16, wide).unwrap_or(LineCol {
        line: wide.line,
        col: wide.col,
    });
    index.offset(line_col).unwrap_or(index.len())
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// An open text document with its parse tree.
pub struct Document {
    pub text: String,
    pub version: i32,
    pub line_index: LineIndex,
    pub tree: Tree,
    parser: JsonParser,
}

impl Document {
    pub fn new(text: String, version: i32) -> Self {
        let mut parser = JsonParser::new();
        let tree = parser
            .parse(&text)
            .expect("tree-sitter parse should always succeed");
        let line_index = LineIndex::new(&text);

        Document {
            text,
            version,
            line_index,
            tree,
            parser,
        }
    }

    /// Replace the entire document text.
    pub fn replace_full(&mut self, text: String, version: i32) {
        self.tree = self
            .parser
            .parse(&text)
            .expect("tree-sitter parse should always succeed");
        self.text = text;
        self.version = version;
        self.line_index = LineIndex::new(&self.text);
    }

    /// Apply an incremental edit from LSP range + new text.
    /// Order matters: compute old positions from pre-edit text, apply the text
    /// change, compute new positions from post-edit text, then tell tree-sitter.
    pub fn apply_edit(&mut self, range: Range, new_text: &str, version: i32) {
        let start_byte = self.offset_of(range.start);
        let old_end_byte = self.offset_of(range.end);
        let new_end_byte = start_byte + new_text.len();

        // Compute old positions from the current (pre-edit) source.
        let start_position = tree::byte_to_point(&self.text, start_byte);
        let old_end_position = tree::byte_to_point(&self.text, old_end_byte);

        // Apply the text change.
        self.text.replace_range(start_byte..old_end_byte, new_text);

        // Compute new_end_position from the updated source.
        let new_end_position = tree::byte_to_point(&self.text, new_end_byte);

        let edit = tree_sitter::InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position,
        };

        // Tell tree-sitter about the edit, then incrementally re-parse.
        self.tree.edit(&edit);
        self.tree = self
            .parser
            .reparse(&self.text, &self.tree)
            .expect("tree-sitter reparse should always succeed");

        self.version = version;
        // Rebuild the line index (SIMD-accelerated, fast enough for incremental edits).
        self.line_index = LineIndex::new(&self.text);
    }

    /// Convenience: convert an LSP Position to a byte offset.
    #[inline]
    pub fn offset_of(&self, pos: Position) -> usize {
        from_lsp_position(&self.line_index, pos).into()
    }

    /// Convenience: convert a byte offset to an LSP Position.
    #[inline]
    pub fn position_of(&self, offset: usize) -> Position {
        to_lsp_position(&self.line_index, line_index::TextSize::new(offset as u32))
    }

    /// Convenience: convert a byte range to an LSP Range.
    #[inline]
    pub fn range_of(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.position_of(start),
            end: self.position_of(end),
        }
    }

    /// Source bytes for passing to tree-sitter node methods.
    #[inline]
    pub fn source(&self) -> &[u8] {
        self.text.as_bytes()
    }
}

// ---------------------------------------------------------------------------
// Document store
// ---------------------------------------------------------------------------

/// Manages all currently open documents.
pub struct DocumentStore {
    docs: HashMap<Url, Document>,
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore {
            docs: HashMap::new(),
        }
    }

    pub fn open(&mut self, uri: Url, text: String, version: i32) {
        self.docs.insert(uri, Document::new(text, version));
    }

    pub fn close(&mut self, uri: &Url) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<&Document> {
        self.docs.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Url) -> Option<&mut Document> {
        self.docs.get_mut(uri)
    }
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_roundtrip() {
        let doc = Document::new("hello\nworld\n".into(), 0);
        let pos = Position {
            line: 1,
            character: 3,
        };
        let offset = doc.offset_of(pos);
        assert_eq!(offset, 9); // "hello\n" = 6, + 3 = 9
        assert_eq!(doc.position_of(offset), pos);
    }

    #[test]
    fn incremental_edit() {
        let mut doc = Document::new(r#"{"a": 1}"#.into(), 0);
        assert!(!doc.tree.root_node().has_error());

        // Replace "1" with "2".
        let range = Range {
            start: Position {
                line: 0,
                character: 6,
            },
            end: Position {
                line: 0,
                character: 7,
            },
        };
        doc.apply_edit(range, "2", 1);
        assert_eq!(doc.text, r#"{"a": 2}"#);
        assert!(!doc.tree.root_node().has_error());
    }

    #[test]
    fn utf16_offset() {
        // Emoji U+1F600 = 2 UTF-16 code units, 4 UTF-8 bytes.
        let doc = Document::new("a\u{1F600}b".into(), 0);
        let offset = doc.offset_of(Position {
            line: 0,
            character: 3,
        });
        assert_eq!(offset, 5); // 1 + 4 = 5
    }

    #[test]
    fn multiline_edit() {
        let mut doc = Document::new("{\n  \"a\": 1\n}".into(), 0);
        // Insert a new property after "a": 1
        let range = Range {
            start: Position {
                line: 1,
                character: 8,
            },
            end: Position {
                line: 1,
                character: 8,
            },
        };
        doc.apply_edit(range, ",\n  \"b\": 2", 1);
        assert!(doc.text.contains("\"b\": 2"));
        assert!(!doc.tree.root_node().has_error());
    }
}
