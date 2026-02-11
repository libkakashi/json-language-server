/// Document store with incremental sync via tree-sitter.
///
/// Each open document maintains:
/// - The current source text
/// - A line index for fast offset <-> position conversion
/// - A tree-sitter `Tree` that is incrementally updated on edits
/// - A per-document `JsonParser` instance
use std::collections::HashMap;

use tower_lsp::lsp_types::{Position, Range, Url};
use tree_sitter::Tree;

use crate::tree::{self, JsonParser};

// ---------------------------------------------------------------------------
// Line index — fast UTF-16 <-> byte offset conversion
// ---------------------------------------------------------------------------

/// Pre-computed line start offsets for a document.
///
/// LSP positions use (line, character) where character is in UTF-16 code
/// units. Tree-sitter uses byte offsets. This index makes both conversions
/// O(1) for the line lookup + O(line_length) for the column.
pub struct LineIndex {
    /// Byte offset of the start of each line. Index 0 is always 0.
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex {
            line_starts: starts,
        }
    }

    /// Incrementally update line starts after a text edit.
    /// `start_byte` is where the edit begins, `old_len` is the byte length
    /// of the removed region, `new_text` is the replacement text.
    pub fn update(&mut self, text: &str, start_byte: usize, old_len: usize, new_text: &str) {
        let old_end = start_byte + old_len;
        let new_len = new_text.len();
        let delta = new_len as isize - old_len as isize;

        // Find the range of lines affected by the edit.
        // First line that starts at or after start_byte+1 (lines whose start
        // was within or after the old region).
        let first_removed = self.line_starts.partition_point(|&s| s <= start_byte);
        let last_removed = self.line_starts.partition_point(|&s| s <= old_end);

        // Remove the line starts that fell inside the old region.
        // These will be replaced by new line starts from the replacement text.
        let lines_after: Vec<usize> = self.line_starts[last_removed..]
            .iter()
            .map(|&s| (s as isize + delta) as usize)
            .collect();

        // Compute new line starts within the replacement text.
        let mut new_starts = Vec::new();
        for (i, b) in new_text.bytes().enumerate() {
            if b == b'\n' {
                new_starts.push(start_byte + i + 1);
            }
        }

        // Rebuild: keep lines before the edit, add new interior lines,
        // then shifted lines after.
        self.line_starts.truncate(first_removed);
        self.line_starts.extend(new_starts);
        self.line_starts.extend(lines_after);

        // Safety: if something went wrong, fall back to full rebuild.
        // This should never happen, but protects against edge cases.
        if self.line_starts.is_empty() || self.line_starts[0] != 0 {
            *self = LineIndex::new(text);
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Convert an LSP `Position` (line, character in UTF-16 CUs) to a byte offset.
    pub fn offset_of(&self, text: &str, pos: Position) -> usize {
        let line = pos.line as usize;
        if line >= self.line_starts.len() {
            return text.len();
        }
        let line_start = self.line_starts[line];
        let line_text = if line + 1 < self.line_starts.len() {
            &text[line_start..self.line_starts[line + 1]]
        } else {
            &text[line_start..]
        };

        let mut utf16_offset = 0u32;
        let mut byte_offset = 0usize;
        for ch in line_text.chars() {
            if utf16_offset >= pos.character {
                break;
            }
            utf16_offset += ch.len_utf16() as u32;
            byte_offset += ch.len_utf8();
        }

        line_start + byte_offset
    }

    /// Convert a byte offset to an LSP `Position`.
    pub fn position_of(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(text.len());

        // Binary search for the line.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert) => insert.saturating_sub(1),
        };

        let line_start = self.line_starts[line];
        let prefix = &text[line_start..offset];

        // Count UTF-16 code units for the character offset.
        let character: u32 = prefix.chars().map(|c| c.len_utf16() as u32).sum();

        Position {
            line: line as u32,
            character,
        }
    }

    /// Convert a byte range to an LSP `Range`.
    pub fn range_of(&self, text: &str, start: usize, end: usize) -> Range {
        Range {
            start: self.position_of(text, start),
            end: self.position_of(text, end),
        }
    }
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
        let start_byte = self.line_index.offset_of(&self.text, range.start);
        let old_end_byte = self.line_index.offset_of(&self.text, range.end);
        let old_len = old_end_byte - start_byte;
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
        self.line_index
            .update(&self.text, start_byte, old_len, new_text);
    }

    /// Convenience: convert an LSP Position to a byte offset.
    #[inline]
    pub fn offset_of(&self, pos: Position) -> usize {
        self.line_index.offset_of(&self.text, pos)
    }

    /// Convenience: convert a byte offset to an LSP Position.
    #[inline]
    pub fn position_of(&self, offset: usize) -> Position {
        self.line_index.position_of(&self.text, offset)
    }

    /// Convenience: convert a byte range to an LSP Range.
    #[inline]
    pub fn range_of(&self, start: usize, end: usize) -> Range {
        self.line_index.range_of(&self.text, start, end)
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
