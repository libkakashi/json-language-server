/// Document symbols provider: produces a hierarchical outline of all keys
/// and values in a JSON document.
///
/// Uses direct recursive traversal matching the approach of
/// vscode-json-languageservice. All node dispatch uses pre-cached numeric
/// kind/field IDs and position conversion uses the O(1) ASCII fast-path.
use lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::{self, FieldIds, KindIds, is_value_node_id};

use serde_json::{Map, Number, Value};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Produce hierarchical document symbols as a pre-built `serde_json::Value`
/// array, skipping the intermediate `Vec<DocumentSymbol>` → `to_value()` pass.
pub fn document_symbols_value(doc: &Document) -> Value {
    let Some(root_value) = tree::root_value(&doc.tree) else {
        return Value::Array(Vec::new());
    };

    let kinds = doc.kind_ids();
    let fields = doc.field_ids();
    let source = doc.source();
    let root_kind = root_value.kind_id();

    let items = if root_kind == kinds.object {
        collect_object_v(doc, source, root_value, kinds, fields)
    } else if root_kind == kinds.array {
        collect_array_v(doc, source, root_value, kinds, fields)
    } else {
        Vec::new()
    };
    Value::Array(items)
}

/// Produce hierarchical document symbols.
pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let Some(root_value) = tree::root_value(&doc.tree) else {
        return Vec::new();
    };

    let kinds = doc.kind_ids();
    let fields = doc.field_ids();
    let source = doc.source();
    let root_kind = root_value.kind_id();

    if root_kind == kinds.object {
        collect_object(doc, source, root_value, kinds, fields)
    } else if root_kind == kinds.array {
        collect_array(doc, source, root_value, kinds, fields)
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Object children
// ---------------------------------------------------------------------------

fn collect_object(
    doc: &Document,
    source: &[u8],
    object: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Vec<DocumentSymbol> {
    let mut cursor = object.walk();
    if !cursor.goto_first_child() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(object.named_child_count());

    loop {
        let pair = cursor.node();
        if pair.kind_id() == kinds.pair {
            if let Some(sym) = pair_symbol(doc, source, pair, kinds, fields) {
                result.push(sym);
            }
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    result
}

#[inline]
fn pair_symbol(
    doc: &Document,
    source: &[u8],
    pair: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Option<DocumentSymbol> {
    let key_node = pair.child_by_field_id(fields.key)?;
    let name = string_content_fast(key_node, source)?.to_string();

    let value_node = pair.child_by_field_id(fields.value);
    let (detail, kind, children) = match value_node {
        Some(v) => {
            let vk = v.kind_id();
            let children = if vk == kinds.object {
                Some(collect_object(doc, source, v, kinds, fields))
            } else if vk == kinds.array {
                Some(collect_array(doc, source, v, kinds, fields))
            } else {
                None
            };
            (
                value_detail(source, v, kinds),
                symbol_kind(vk, kinds),
                children,
            )
        }
        None => (None, SymbolKind::NULL, None),
    };

    let range = doc.node_range(&pair);
    let selection_range = doc.node_range(&key_node);

    #[allow(deprecated)]
    Some(DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children,
    })
}

// ---------------------------------------------------------------------------
// Array children
// ---------------------------------------------------------------------------

fn collect_array(
    doc: &Document,
    source: &[u8],
    array: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Vec<DocumentSymbol> {
    let mut cursor = array.walk();
    if !cursor.goto_first_child() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(array.named_child_count());
    let mut index = 0usize;
    let mut itoa_buf = itoa::Buffer::new();

    loop {
        let item = cursor.node();
        let item_kind = item.kind_id();
        if is_value_node_id(item_kind, kinds) {
            let name = array_index_name(index, &mut itoa_buf);
            let detail = value_detail(source, item, kinds);
            let kind = symbol_kind(item_kind, kinds);
            let range = doc.node_range(&item);

            let children = if item_kind == kinds.object {
                Some(collect_object(doc, source, item, kinds, fields))
            } else if item_kind == kinds.array {
                Some(collect_array(doc, source, item, kinds, fields))
            } else {
                None
            };

            #[allow(deprecated)]
            result.push(DocumentSymbol {
                name,
                detail,
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children,
            });
            index += 1;
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Value builders (single-pass serialization)
// ---------------------------------------------------------------------------

fn collect_object_v(
    doc: &Document,
    source: &[u8],
    object: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Vec<Value> {
    let mut cursor = object.walk();
    if !cursor.goto_first_child() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(object.named_child_count());

    loop {
        let pair = cursor.node();
        if pair.kind_id() == kinds.pair {
            if let Some(sym) = pair_symbol_v(doc, source, pair, kinds, fields) {
                result.push(sym);
            }
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    result
}

#[inline]
fn pair_symbol_v(
    doc: &Document,
    source: &[u8],
    pair: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Option<Value> {
    let key_node = pair.child_by_field_id(fields.key)?;
    let name = string_content_fast(key_node, source)?;

    let value_node = pair.child_by_field_id(fields.value);
    let (detail, kind, children) = match value_node {
        Some(v) => {
            let vk = v.kind_id();
            let children = if vk == kinds.object {
                Some(collect_object_v(doc, source, v, kinds, fields))
            } else if vk == kinds.array {
                Some(collect_array_v(doc, source, v, kinds, fields))
            } else {
                None
            };
            (
                value_detail(source, v, kinds),
                symbol_kind_num(vk, kinds),
                children,
            )
        }
        None => (None, 21, None), // NULL = 21
    };

    let range = doc.node_range(&pair);
    let selection_range = doc.node_range(&key_node);

    Some(build_symbol_value(
        name,
        detail.as_deref(),
        kind,
        &range,
        &selection_range,
        children,
    ))
}

fn collect_array_v(
    doc: &Document,
    source: &[u8],
    array: Node<'_>,
    kinds: &KindIds,
    fields: &FieldIds,
) -> Vec<Value> {
    let mut cursor = array.walk();
    if !cursor.goto_first_child() {
        return Vec::new();
    }

    let mut result = Vec::with_capacity(array.named_child_count());
    let mut index = 0usize;
    let mut itoa_buf = itoa::Buffer::new();

    loop {
        let item = cursor.node();
        let item_kind = item.kind_id();
        if is_value_node_id(item_kind, kinds) {
            let name = array_index_name(index, &mut itoa_buf);
            let detail = value_detail(source, item, kinds);
            let kind = symbol_kind_num(item_kind, kinds);
            let range = doc.node_range(&item);

            let children = if item_kind == kinds.object {
                Some(collect_object_v(doc, source, item, kinds, fields))
            } else if item_kind == kinds.array {
                Some(collect_array_v(doc, source, item, kinds, fields))
            } else {
                None
            };

            result.push(build_symbol_value(
                &name,
                detail.as_deref(),
                kind,
                &range,
                &range,
                children,
            ));
            index += 1;
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    result
}

#[inline]
fn range_to_value(range: &Range) -> Value {
    let start = Value::Object(Map::from_iter([
        ("line".into(), Value::Number(Number::from(range.start.line))),
        (
            "character".into(),
            Value::Number(Number::from(range.start.character)),
        ),
    ]));
    let end = Value::Object(Map::from_iter([
        ("line".into(), Value::Number(Number::from(range.end.line))),
        (
            "character".into(),
            Value::Number(Number::from(range.end.character)),
        ),
    ]));
    Value::Object(Map::from_iter([
        ("start".into(), start),
        ("end".into(), end),
    ]))
}

fn build_symbol_value(
    name: &str,
    detail: Option<&str>,
    kind: u32,
    range: &Range,
    selection_range: &Range,
    children: Option<Vec<Value>>,
) -> Value {
    let mut map = Map::with_capacity(6);
    map.insert("name".into(), Value::String(name.into()));
    if let Some(d) = detail {
        map.insert("detail".into(), Value::String(d.into()));
    }
    map.insert("kind".into(), Value::Number(Number::from(kind)));
    map.insert("range".into(), range_to_value(range));
    map.insert("selectionRange".into(), range_to_value(selection_range));
    if let Some(ch) = children {
        map.insert("children".into(), Value::Array(ch));
    }
    Value::Object(map)
}

#[inline]
fn symbol_kind_num(kind_id: u16, kinds: &KindIds) -> u32 {
    if kind_id == kinds.object {
        19
    } else if kind_id == kinds.array {
        18
    } else if kind_id == kinds.string {
        15
    } else if kind_id == kinds.number {
        16
    } else if kind_id == kinds.r#true || kind_id == kinds.r#false {
        17
    } else if kind_id == kinds.null {
        21
    } else {
        20 // KEY
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the unquoted string content directly from source bytes.
/// Avoids the FFI round-trip of `node.utf8_text()` by slicing source directly.
#[inline]
fn string_content_fast<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let start = node.start_byte();
    let end = node.end_byte();
    if end - start >= 2 && source[start] == b'"' && source[end - 1] == b'"' {
        std::str::from_utf8(&source[start + 1..end - 1]).ok()
    } else {
        std::str::from_utf8(&source[start..end]).ok()
    }
}

#[inline]
fn symbol_kind(kind_id: u16, kinds: &KindIds) -> SymbolKind {
    if kind_id == kinds.object {
        SymbolKind::OBJECT
    } else if kind_id == kinds.array {
        SymbolKind::ARRAY
    } else if kind_id == kinds.string {
        SymbolKind::STRING
    } else if kind_id == kinds.number {
        SymbolKind::NUMBER
    } else if kind_id == kinds.r#true || kind_id == kinds.r#false {
        SymbolKind::BOOLEAN
    } else if kind_id == kinds.null {
        SymbolKind::NULL
    } else {
        SymbolKind::KEY
    }
}

#[inline]
fn value_detail(source: &[u8], node: Node<'_>, kinds: &KindIds) -> Option<String> {
    let kid = node.kind_id();
    if kid == kinds.string {
        let s = string_content_fast(node, source)?;
        if s.len() > 60 {
            Some(format!("\"{}...\"", &s[..57]))
        } else {
            Some(format!("\"{s}\""))
        }
    } else if kid == kinds.number
        || kid == kinds.r#true
        || kid == kinds.r#false
        || kid == kinds.null
    {
        let start = node.start_byte();
        let end = node.end_byte();
        Some(std::str::from_utf8(&source[start..end]).ok()?.to_string())
    } else {
        None
    }
}

/// Format an array index name like `[0]`, `[1]`, etc. using itoa.
#[inline]
fn array_index_name(index: usize, buf: &mut itoa::Buffer) -> String {
    let formatted = buf.format(index);
    let mut s = String::with_capacity(formatted.len() + 2);
    s.push('[');
    s.push_str(formatted);
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn empty_object_no_symbols() {
        let doc = Document::new("{}".into(), 0);
        let syms = document_symbols(&doc);
        assert!(syms.is_empty());
    }

    #[test]
    fn flat_object_symbols() {
        let doc = Document::new(r#"{"name": "Alice", "age": 30}"#.into(), 0);
        let syms = document_symbols(&doc);
        assert_eq!(syms.len(), 2);
        assert_eq!(syms[0].name, "name");
        assert_eq!(syms[1].name, "age");
    }

    #[test]
    fn nested_object_symbols() {
        let doc = Document::new(r#"{"person": {"name": "Bob"}}"#.into(), 0);
        let syms = document_symbols(&doc);
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "person");
        assert!(syms[0].children.is_some());
        let children = syms[0].children.as_ref().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "name");
    }

    #[test]
    fn array_symbols() {
        let doc = Document::new(r#"{"items": [1, 2, 3]}"#.into(), 0);
        let syms = document_symbols(&doc);
        assert_eq!(syms.len(), 1);
        let children = syms[0].children.as_ref().unwrap();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].name, "[0]");
        assert_eq!(children[1].name, "[1]");
        assert_eq!(children[2].name, "[2]");
    }

    #[test]
    fn root_array_no_symbols() {
        let doc = Document::new("[1, 2, 3]".into(), 0);
        let syms = document_symbols(&doc);
        assert_eq!(syms.len(), 3);
    }

    #[test]
    fn symbol_kinds() {
        let doc = Document::new(
            r#"{"str": "hi", "num": 42, "bool": true, "nil": null, "obj": {}, "arr": []}"#.into(),
            0,
        );
        let syms = document_symbols(&doc);
        assert_eq!(syms.len(), 6);
        assert_eq!(syms[0].kind, SymbolKind::STRING);
        assert_eq!(syms[1].kind, SymbolKind::NUMBER);
        assert_eq!(syms[2].kind, SymbolKind::BOOLEAN);
        assert_eq!(syms[3].kind, SymbolKind::NULL);
        assert_eq!(syms[4].kind, SymbolKind::OBJECT);
        assert_eq!(syms[5].kind, SymbolKind::ARRAY);
    }

    #[test]
    fn empty_document_no_symbols() {
        let doc = Document::new("".into(), 0);
        let syms = document_symbols(&doc);
        assert!(syms.is_empty());
    }

    /// Verify that `document_symbols_value` produces identical JSON to
    /// `serde_json::to_value(document_symbols(...))`.
    #[test]
    fn value_parity_flat() {
        let doc = Document::new(r#"{"name": "Alice", "age": 30}"#.into(), 0);
        let via_typed = serde_json::to_value(document_symbols(&doc)).unwrap();
        let via_direct = document_symbols_value(&doc);
        assert_eq!(via_typed, via_direct);
    }

    #[test]
    fn value_parity_nested() {
        let doc = Document::new(
            r#"{"person": {"name": "Bob"}, "scores": [100, 200]}"#.into(),
            0,
        );
        let via_typed = serde_json::to_value(document_symbols(&doc)).unwrap();
        let via_direct = document_symbols_value(&doc);
        assert_eq!(via_typed, via_direct);
    }

    #[test]
    fn value_parity_root_array() {
        let doc = Document::new(r#"[1, "hello", true, null, {}, []]"#.into(), 0);
        let via_typed = serde_json::to_value(document_symbols(&doc)).unwrap();
        let via_direct = document_symbols_value(&doc);
        assert_eq!(via_typed, via_direct);
    }

    #[test]
    fn value_parity_empty() {
        let doc = Document::new("{}".into(), 0);
        let via_typed = serde_json::to_value(document_symbols(&doc)).unwrap();
        let via_direct = document_symbols_value(&doc);
        assert_eq!(via_typed, via_direct);
    }
}
