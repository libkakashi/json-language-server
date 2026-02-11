/// Document store with incremental sync via tree-sitter.
///
/// Each open document maintains:
/// - The current source text
/// - A line index for fast offset <-> position conversion
/// - A tree-sitter `Tree` that is incrementally updated on edits
/// - A per-document `JsonParser` instance
use std::collections::HashMap;
use std::sync::Arc;

use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
use lsp_types::{Position, Range, Uri};
use tree_sitter::Tree;

use crate::tree::{self, FieldIds, JsonParser, KindIds};

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
    is_ascii: bool,
    parser: JsonParser,
    /// Cached document symbols as a pre-serialized JSON string.
    /// Wrapped in Arc so the server can return it as a zero-copy reference.
    cached_symbols: Arc<str>,
}

impl Document {
    pub fn new(text: String, version: i32) -> Self {
        let mut parser = JsonParser::new();
        let tree = parser
            .parse(&text)
            .expect("tree-sitter parse should always succeed");
        let line_index = LineIndex::new(&text);
        let is_ascii = text.is_ascii();

        let mut doc = Document {
            text,
            version,
            line_index,
            tree,
            is_ascii,
            parser,
            cached_symbols: Arc::from("[]"),
        };
        doc.rebuild_symbols();
        doc
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
        self.is_ascii = self.text.is_ascii();
        self.rebuild_symbols();
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
        self.is_ascii = self.text.is_ascii();
        self.rebuild_symbols();
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

    /// Convert a tree-sitter `Point` to an LSP `Position`.
    /// For ASCII documents (the vast majority of JSON), this is O(1).
    /// For non-ASCII, skips the binary search by using the row from Point directly.
    #[inline]
    pub fn point_to_position(&self, point: tree_sitter::Point) -> Position {
        if self.is_ascii {
            Position {
                line: point.row as u32,
                character: point.column as u32,
            }
        } else {
            let line_col = LineCol {
                line: point.row as u32,
                col: point.column as u32,
            };
            let wide = self
                .line_index
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
    }

    /// Convert a tree-sitter node's range to an LSP Range using Points.
    #[inline]
    pub fn node_range(&self, node: &tree_sitter::Node) -> Range {
        Range {
            start: self.point_to_position(node.start_position()),
            end: self.point_to_position(node.end_position()),
        }
    }

    #[inline]
    pub fn kind_ids(&self) -> &KindIds {
        &self.parser.kind_ids
    }

    #[inline]
    pub fn field_ids(&self) -> &FieldIds {
        &self.parser.field_ids
    }

    /// Return cached document symbols as a pre-serialized JSON string.
    /// Cheap Arc clone — no deep copy or re-serialization needed.
    #[inline]
    pub fn symbols(&self) -> Arc<str> {
        Arc::clone(&self.cached_symbols)
    }

    /// Recompute and cache document symbols from the current tree.
    fn rebuild_symbols(&mut self) {
        let value = crate::symbols::document_symbols_value(self);
        self.cached_symbols = Arc::from(serde_json::to_string(&value).unwrap());
    }
}

// ---------------------------------------------------------------------------
// Document store
// ---------------------------------------------------------------------------

/// Manages all currently open documents.
pub struct DocumentStore {
    docs: HashMap<Uri, Document>,
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore {
            docs: HashMap::new(),
        }
    }

    pub fn open(&mut self, uri: Uri, text: String, version: i32) {
        self.docs.insert(uri, Document::new(text, version));
    }

    pub fn close(&mut self, uri: &Uri) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.docs.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Uri) -> Option<&mut Document> {
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
