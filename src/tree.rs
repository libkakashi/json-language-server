/// Tree-sitter integration layer for JSON.
///
/// Provides a thin wrapper over tree-sitter's concrete syntax tree (CST) with
/// a clean JSON-specific API. Handles:
/// - Parser creation and incremental reparsing
/// - Node kind constants
/// - String unescaping
/// - Convenient tree-walking helpers
use tree_sitter::{Node, Parser, Point, Tree};

// ---------------------------------------------------------------------------
// Node kind constants
// ---------------------------------------------------------------------------

pub mod kinds {
    pub const DOCUMENT: &str = "document";
    pub const OBJECT: &str = "object";
    pub const ARRAY: &str = "array";
    pub const PAIR: &str = "pair";
    pub const STRING: &str = "string";
    pub const STRING_CONTENT: &str = "string_content";
    pub const ESCAPE_SEQUENCE: &str = "escape_sequence";
    pub const NUMBER: &str = "number";
    pub const TRUE: &str = "true";
    pub const FALSE: &str = "false";
    pub const NULL: &str = "null";
    pub const COMMENT: &str = "comment";
    pub const ERROR: &str = "ERROR";
}

// ---------------------------------------------------------------------------
// Cached numeric IDs (resolved once from grammar)
// ---------------------------------------------------------------------------

/// Pre-cached node kind IDs for fast comparison via `node.kind_id()`.
/// Avoids the FFI string conversion that `node.kind()` performs.
pub struct KindIds {
    pub object: u16,
    pub array: u16,
    pub pair: u16,
    pub string: u16,
    pub number: u16,
    pub r#true: u16,
    pub r#false: u16,
    pub null: u16,
}

impl KindIds {
    fn new(lang: &tree_sitter::Language) -> Self {
        KindIds {
            object: lang.id_for_node_kind("object", true),
            array: lang.id_for_node_kind("array", true),
            pair: lang.id_for_node_kind("pair", true),
            string: lang.id_for_node_kind("string", true),
            number: lang.id_for_node_kind("number", true),
            r#true: lang.id_for_node_kind("true", true),
            r#false: lang.id_for_node_kind("false", true),
            null: lang.id_for_node_kind("null", true),
        }
    }
}

/// Pre-cached field IDs for fast child lookup via `node.child_by_field_id()`.
/// Avoids the C-side string hash that `child_by_field_name()` performs.
pub struct FieldIds {
    pub key: u16,
    pub value: u16,
}

impl FieldIds {
    fn new(lang: &tree_sitter::Language) -> Self {
        FieldIds {
            key: lang
                .field_id_for_name("key")
                .map(|id| id.get())
                .unwrap_or(0),
            value: lang
                .field_id_for_name("value")
                .map(|id| id.get())
                .unwrap_or(0),
        }
    }
}

/// Check if a node kind ID represents a JSON value.
#[inline]
pub fn is_value_node_id(kind_id: u16, kinds: &KindIds) -> bool {
    kind_id == kinds.object
        || kind_id == kinds.array
        || kind_id == kinds.string
        || kind_id == kinds.number
        || kind_id == kinds.r#true
        || kind_id == kinds.r#false
        || kind_id == kinds.null
}

// ---------------------------------------------------------------------------
// Parser wrapper
// ---------------------------------------------------------------------------

/// A JSON parser backed by tree-sitter. Reuse this across parses for
/// incremental parsing support.
pub struct JsonParser {
    parser: Parser,
    pub kind_ids: KindIds,
    pub field_ids: FieldIds,
}

impl JsonParser {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_json::LANGUAGE.into();
        parser
            .set_language(&lang)
            .expect("failed to load tree-sitter-json grammar");
        let kind_ids = KindIds::new(&lang);
        let field_ids = FieldIds::new(&lang);
        JsonParser {
            parser,
            kind_ids,
            field_ids,
        }
    }

    /// Parse source text from scratch.
    pub fn parse(&mut self, source: &str) -> Option<Tree> {
        self.parser.parse(source.as_bytes(), None)
    }

    /// Incremental re-parse: pass the old tree (after calling `tree.edit()`)
    /// and the new source text.
    pub fn reparse(&mut self, source: &str, old_tree: &Tree) -> Option<Tree> {
        self.parser.parse(source.as_bytes(), Some(old_tree))
    }
}

impl Default for JsonParser {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// InputEdit construction
// ---------------------------------------------------------------------------

/// Convert a byte offset to a tree-sitter `Point` (row, column in bytes).
pub fn byte_to_point(source: &str, byte_offset: usize) -> Point {
    let offset = byte_offset.min(source.len());
    let mut row = 0usize;
    let mut col = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if i == offset {
            break;
        }
        if b == b'\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Point::new(row, col)
}

// ---------------------------------------------------------------------------
// Node helpers
// ---------------------------------------------------------------------------

/// Extract the unquoted string content from a `string` node.
/// Returns the raw content between quotes (without unescaping).
///
/// Note: tree-sitter may split the content into multiple children
/// (string_content + escape_sequence + string_content...), so we always
/// strip quotes from the full node text to get the complete raw content.
pub fn string_content<'a>(node: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
    let text = node.utf8_text(source).ok()?;
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        Some(&text[1..text.len() - 1])
    } else {
        Some(text)
    }
}

/// Extract the fully unescaped string value from a `string` node.
pub fn string_value(node: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = string_content(node, source)?;
    unescape_json_string(raw)
}

/// Get the key name of a `pair` node.
pub fn pair_key<'a>(pair: Node<'a>, source: &'a [u8]) -> Option<&'a str> {
    let key_node = pair.child_by_field_name("key")?;
    string_content(key_node, source)
}

/// Get the unescaped key name of a `pair` node.
pub fn pair_key_unescaped(pair: Node<'_>, source: &[u8]) -> Option<String> {
    let key_node = pair.child_by_field_name("key")?;
    string_value(key_node, source)
}

/// Get the value node of a `pair`.
pub fn pair_value<'a>(pair: Node<'a>) -> Option<Node<'a>> {
    pair.child_by_field_name("value")
}

/// Iterate the key-value pairs of an `object` node.
pub fn object_pairs<'a>(
    object: Node<'a>,
    cursor: &mut tree_sitter::TreeCursor<'a>,
) -> Vec<Node<'a>> {
    debug_assert_eq!(object.kind(), kinds::OBJECT);
    object
        .named_children(cursor)
        .filter(|n| n.kind() == kinds::PAIR)
        .collect()
}

/// Iterate the value items of an `array` node.
pub fn array_items<'a>(array: Node<'a>, cursor: &mut tree_sitter::TreeCursor<'a>) -> Vec<Node<'a>> {
    debug_assert_eq!(array.kind(), kinds::ARRAY);
    array
        .named_children(cursor)
        .filter(|n| is_value_node(n))
        .collect()
}

/// Count the key-value pairs of an `object` node without allocating.
pub fn object_pair_count(object: Node<'_>) -> usize {
    debug_assert_eq!(object.kind(), kinds::OBJECT);
    let mut cursor = object.walk();
    object
        .named_children(&mut cursor)
        .filter(|n| n.kind() == kinds::PAIR)
        .count()
}

/// Count the value items of an `array` node without allocating.
pub fn array_item_count(array: Node<'_>) -> usize {
    debug_assert_eq!(array.kind(), kinds::ARRAY);
    let mut cursor = array.walk();
    array
        .named_children(&mut cursor)
        .filter(|n| is_value_node(n))
        .count()
}

/// Check if a node represents a JSON value.
pub fn is_value_node(node: &Node<'_>) -> bool {
    matches!(
        node.kind(),
        kinds::OBJECT
            | kinds::ARRAY
            | kinds::STRING
            | kinds::NUMBER
            | kinds::TRUE
            | kinds::FALSE
            | kinds::NULL
    )
}

/// Get the first value child of the document root.
pub fn root_value(tree: &Tree) -> Option<Node<'_>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    root.named_children(&mut cursor).find(|n| is_value_node(n))
}

/// Find the smallest named node at a byte offset.
pub fn node_at_offset<'a>(tree: &'a Tree, offset: usize) -> Option<Node<'a>> {
    tree.root_node()
        .named_descendant_for_byte_range(offset, offset)
}

/// Walk up from a node collecting the JSON path segments.
/// Returns segments like `["key", "0", "nested"]` for `/key/0/nested`.
pub fn json_path(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = node;

    while let Some(parent) = current.parent() {
        match parent.kind() {
            kinds::PAIR => {
                // This node is the value of a pair — the key is a sibling.
                if let Some(key) = pair_key_unescaped(parent, source) {
                    segments.push(key);
                }
            }
            kinds::ARRAY => {
                // Find this node's index among array items.
                let mut cursor = parent.walk();
                let mut idx = 0;
                for child in parent.named_children(&mut cursor) {
                    if child.id() == current.id() {
                        segments.push(idx.to_string());
                        break;
                    }
                    if is_value_node(&child) {
                        idx += 1;
                    }
                }
            }
            _ => {}
        }
        current = parent;
    }

    segments.reverse();
    segments
}

/// Format a JSON path as a JSON Pointer string (e.g. `/foo/0/bar`).
pub fn json_pointer(node: Node<'_>, source: &[u8]) -> String {
    let segs = json_path(node, source);
    if segs.is_empty() {
        String::new()
    } else {
        format!("/{}", segs.join("/"))
    }
}

// ---------------------------------------------------------------------------
// String unescaping
// ---------------------------------------------------------------------------

/// Unescape a JSON string body (without surrounding quotes).
/// Returns `None` if the string contains invalid escape sequences.
pub fn unescape_json_string(raw: &str) -> Option<String> {
    if !raw.contains('\\') {
        return Some(raw.to_string());
    }

    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let hex = read_hex4(&mut chars)?;
                // UTF-16 surrogate pair: \uD800-\uDBFF followed by \uDC00-\uDFFF
                // encodes a codepoint above U+FFFF (e.g. emoji).
                let cp = if (0xD800..=0xDBFF).contains(&hex) {
                    if chars.next() != Some('\\') || chars.next() != Some('u') {
                        return None;
                    }
                    let low = read_hex4(&mut chars)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return None;
                    }
                    0x10000 + ((hex as u32 - 0xD800) << 10) + (low as u32 - 0xDC00)
                } else {
                    hex as u32
                };
                out.push(char::from_u32(cp)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn read_hex4(chars: &mut std::str::Chars<'_>) -> Option<u16> {
    let mut val: u16 = 0;
    for _ in 0..4 {
        let c = chars.next()?;
        let digit = match c {
            '0'..='9' => c as u16 - b'0' as u16,
            'a'..='f' => c as u16 - b'a' as u16 + 10,
            'A'..='F' => c as u16 - b'A' as u16 + 10,
            _ => return None,
        };
        val = (val << 4) | digit;
    }
    Some(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Parsing --

    #[test]
    fn parse_simple_object() {
        let mut parser = JsonParser::new();
        let tree = parser.parse(r#"{"key": "value"}"#).unwrap();
        let root = tree.root_node();
        assert_eq!(root.kind(), kinds::DOCUMENT);
        let obj = root.named_child(0).unwrap();
        assert_eq!(obj.kind(), kinds::OBJECT);
    }

    #[test]
    fn parse_array() {
        let mut parser = JsonParser::new();
        let src = r#"[1, "two", true, null, []]"#;
        let tree = parser.parse(src).unwrap();
        let arr = root_value(&tree).unwrap();
        assert_eq!(arr.kind(), kinds::ARRAY);
        let mut cursor = arr.walk();
        assert_eq!(array_items(arr, &mut cursor).len(), 5);
    }

    #[test]
    fn parse_error_recovery() {
        let mut parser = JsonParser::new();
        let src = r#"{"a": 1, "b": }"#; // Missing value
        let tree = parser.parse(src).unwrap();
        assert!(tree.root_node().has_error());
        // Should still produce a tree with partial data.
        let root = root_value(&tree).unwrap();
        assert_eq!(root.kind(), kinds::OBJECT);
    }

    #[test]
    fn parse_nested_objects() {
        let mut parser = JsonParser::new();
        let src = r#"{"a": {"b": {"c": 42}}}"#;
        let tree = parser.parse(src).unwrap();
        assert!(!tree.root_node().has_error());
        let root = root_value(&tree).unwrap();
        assert_eq!(root.kind(), kinds::OBJECT);
    }

    #[test]
    fn parse_empty_document() {
        let mut parser = JsonParser::new();
        let tree = parser.parse("").unwrap();
        assert!(root_value(&tree).is_none());
    }

    #[test]
    fn incremental_reparse() {
        let mut parser = JsonParser::new();
        let tree = parser.parse(r#"{"a": 1}"#).unwrap();
        assert!(!tree.root_node().has_error());
        // Simulate editing — just full reparse via reparse with old tree.
        let new_tree = parser.reparse(r#"{"a": 2}"#, &tree).unwrap();
        assert!(!new_tree.root_node().has_error());
    }

    // -- Pair access --

    #[test]
    fn pair_access() {
        let mut parser = JsonParser::new();
        let src = r#"{"name": "Alice", "age": 30}"#;
        let tree = parser.parse(src).unwrap();
        let obj = root_value(&tree).unwrap();
        let mut cursor = obj.walk();
        let pairs = object_pairs(obj, &mut cursor);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pair_key(pairs[0], src.as_bytes()), Some("name"));
        assert_eq!(pair_key(pairs[1], src.as_bytes()), Some("age"));
    }

    #[test]
    fn pair_value_types() {
        let mut parser = JsonParser::new();
        let src = r#"{"s": "hi", "n": 42, "b": true, "null": null}"#;
        let tree = parser.parse(src).unwrap();
        let obj = root_value(&tree).unwrap();
        let mut cursor = obj.walk();
        let pairs = object_pairs(obj, &mut cursor);
        assert_eq!(pair_value(pairs[0]).unwrap().kind(), kinds::STRING);
        assert_eq!(pair_value(pairs[1]).unwrap().kind(), kinds::NUMBER);
        assert_eq!(pair_value(pairs[2]).unwrap().kind(), kinds::TRUE);
        assert_eq!(pair_value(pairs[3]).unwrap().kind(), kinds::NULL);
    }

    #[test]
    fn empty_object_pairs() {
        let mut parser = JsonParser::new();
        let tree = parser.parse("{}").unwrap();
        let obj = root_value(&tree).unwrap();
        let mut cursor = obj.walk();
        assert_eq!(object_pairs(obj, &mut cursor).len(), 0);
    }

    #[test]
    fn empty_array_items() {
        let mut parser = JsonParser::new();
        let tree = parser.parse("[]").unwrap();
        let arr = root_value(&tree).unwrap();
        let mut cursor = arr.walk();
        assert_eq!(array_items(arr, &mut cursor).len(), 0);
    }

    // -- String content --

    #[test]
    fn string_content_simple() {
        let mut parser = JsonParser::new();
        let src = r#""hello world""#;
        let tree = parser.parse(src).unwrap();
        let node = root_value(&tree).unwrap();
        assert_eq!(string_content(node, src.as_bytes()), Some("hello world"));
    }

    #[test]
    fn string_value_with_escapes() {
        let mut parser = JsonParser::new();
        let src = r#""line1\nline2""#;
        let tree = parser.parse(src).unwrap();
        let node = root_value(&tree).unwrap();
        assert_eq!(
            string_value(node, src.as_bytes()),
            Some("line1\nline2".into())
        );
    }

    // -- JSON path --

    #[test]
    fn json_path_works() {
        let mut parser = JsonParser::new();
        let src = r#"{"a": {"b": [1, 2]}}"#;
        let tree = parser.parse(src).unwrap();
        let node = tree
            .root_node()
            .named_descendant_for_byte_range(16, 17)
            .unwrap();
        assert_eq!(node.kind(), kinds::NUMBER);
        let path = json_pointer(node, src.as_bytes());
        assert_eq!(path, "/a/b/1");
    }

    #[test]
    fn json_path_root_value() {
        let mut parser = JsonParser::new();
        let src = r#"{"a": 1}"#;
        let tree = parser.parse(src).unwrap();
        let root = root_value(&tree).unwrap();
        let path = json_pointer(root, src.as_bytes());
        assert_eq!(path, "");
    }

    #[test]
    fn json_path_deep_nesting() {
        let mut parser = JsonParser::new();
        let src = r#"{"x": {"y": {"z": "leaf"}}}"#;
        let tree = parser.parse(src).unwrap();
        // Find the "leaf" string node at bytes 18..23 ("leaf" with quotes).
        let node = tree
            .root_node()
            .named_descendant_for_byte_range(18, 23)
            .unwrap();
        assert_eq!(node.kind(), kinds::STRING);
        let path = json_pointer(node, src.as_bytes());
        assert_eq!(path, "/x/y/z");
    }

    // -- is_value_node --

    #[test]
    fn is_value_node_checks() {
        let mut parser = JsonParser::new();
        let src = r#"[1, "s", true, false, null, {}, []]"#;
        let tree = parser.parse(src).unwrap();
        let arr = root_value(&tree).unwrap();
        let mut cursor = arr.walk();
        let items = array_items(arr, &mut cursor);
        assert_eq!(items.len(), 7);
        for item in &items {
            assert!(is_value_node(item));
        }
    }

    // -- node_at_offset --

    #[test]
    fn node_at_offset_finds_string() {
        let mut parser = JsonParser::new();
        let src = r#"{"key": "val"}"#;
        let tree = parser.parse(src).unwrap();
        // Offset 8 is the opening quote of "val" — should find the string node.
        let node = node_at_offset(&tree, 8).unwrap();
        // Could be string or string_content depending on exact offset.
        assert!(
            node.kind() == kinds::STRING || node.kind() == kinds::STRING_CONTENT,
            "expected string or string_content, got {}",
            node.kind()
        );
    }

    // -- Unescape --

    #[test]
    fn unescape_basic() {
        assert_eq!(unescape_json_string("hello"), Some("hello".into()));
        assert_eq!(unescape_json_string(r"a\nb"), Some("a\nb".into()));
        assert_eq!(unescape_json_string(r"\u0041"), Some("A".into()));
    }

    #[test]
    fn unescape_surrogate_pair() {
        assert_eq!(
            unescape_json_string(r"\uD83D\uDE00"),
            Some("\u{1F600}".into())
        );
    }

    #[test]
    fn unescape_all_escapes() {
        assert_eq!(unescape_json_string(r#"\""#), Some("\"".into()));
        assert_eq!(unescape_json_string(r"\\"), Some("\\".into()));
        assert_eq!(unescape_json_string(r"\/"), Some("/".into()));
        assert_eq!(unescape_json_string(r"\b"), Some("\u{0008}".into()));
        assert_eq!(unescape_json_string(r"\f"), Some("\u{000C}".into()));
        assert_eq!(unescape_json_string(r"\r"), Some("\r".into()));
        assert_eq!(unescape_json_string(r"\t"), Some("\t".into()));
    }

    #[test]
    fn unescape_invalid() {
        assert_eq!(unescape_json_string(r"\x"), None);
        assert_eq!(unescape_json_string(r"\uZZZZ"), None);
        assert_eq!(unescape_json_string(r"\"), None);
        // Lone high surrogate.
        assert_eq!(unescape_json_string(r"\uD800"), None);
        // High surrogate without low surrogate.
        assert_eq!(unescape_json_string(r"\uD800\u0041"), None);
    }

    #[test]
    fn unescape_no_escapes_passthrough() {
        assert_eq!(
            unescape_json_string("plain text"),
            Some("plain text".into())
        );
    }

    // -- byte_to_point --

    #[test]
    fn byte_to_point_basic() {
        let src = "ab\ncd\nef";
        assert_eq!(byte_to_point(src, 0), Point::new(0, 0));
        assert_eq!(byte_to_point(src, 2), Point::new(0, 2)); // At \n
        assert_eq!(byte_to_point(src, 3), Point::new(1, 0)); // Start of line 2
        assert_eq!(byte_to_point(src, 6), Point::new(2, 0)); // Start of line 3
    }

    #[test]
    fn byte_to_point_past_end() {
        let src = "abc";
        let p = byte_to_point(src, 100);
        assert_eq!(p, Point::new(0, 3));
    }
}
