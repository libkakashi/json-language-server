/// Document symbols provider: produces a hierarchical outline of all keys
/// and values in a JSON document.
use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::{self, kinds};

/// Produce hierarchical document symbols.
pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let Some(root_value) = tree::root_value(&doc.tree) else {
        return Vec::new();
    };
    children_symbols(doc, root_value)
}

fn children_symbols(doc: &Document, node: Node<'_>) -> Vec<DocumentSymbol> {
    match node.kind() {
        kinds::OBJECT => object_symbols(doc, node),
        kinds::ARRAY => array_symbols(doc, node),
        _ => Vec::new(),
    }
}

fn object_symbols(doc: &Document, object: Node<'_>) -> Vec<DocumentSymbol> {
    let mut cursor = object.walk();
    let pairs = tree::object_pairs(object, &mut cursor);

    pairs
        .iter()
        .filter_map(|pair| {
            let key_node = pair.child_by_field_name("key")?;
            let name = tree::string_content(key_node, doc.source())?.to_string();

            let value_node = tree::pair_value(*pair);
            let (detail, kind) = match &value_node {
                Some(v) => (value_detail(doc, *v), node_symbol_kind(*v)),
                None => (None, SymbolKind::NULL),
            };

            let range = doc.range_of(pair.start_byte(), pair.end_byte());
            let selection_range = doc.range_of(key_node.start_byte(), key_node.end_byte());

            let children = value_node.and_then(|v| match v.kind() {
                kinds::OBJECT | kinds::ARRAY => Some(children_symbols(doc, v)),
                _ => None,
            });

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
        })
        .collect()
}

fn array_symbols(doc: &Document, array: Node<'_>) -> Vec<DocumentSymbol> {
    let mut cursor = array.walk();
    let items = tree::array_items(array, &mut cursor);

    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let name = format!("[{i}]");
            let detail = value_detail(doc, *item);
            let kind = node_symbol_kind(*item);
            let range = doc.range_of(item.start_byte(), item.end_byte());

            let children = match item.kind() {
                kinds::OBJECT | kinds::ARRAY => Some(children_symbols(doc, *item)),
                _ => None,
            };

            #[allow(deprecated)]
            DocumentSymbol {
                name,
                detail,
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children,
            }
        })
        .collect()
}

fn node_symbol_kind(node: Node<'_>) -> SymbolKind {
    match node.kind() {
        kinds::OBJECT => SymbolKind::OBJECT,
        kinds::ARRAY => SymbolKind::ARRAY,
        kinds::STRING => SymbolKind::STRING,
        kinds::NUMBER => SymbolKind::NUMBER,
        kinds::TRUE | kinds::FALSE => SymbolKind::BOOLEAN,
        kinds::NULL => SymbolKind::NULL,
        _ => SymbolKind::KEY,
    }
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
        // Root array items get symbols with [0], [1], etc.
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
}

fn value_detail(doc: &Document, node: Node<'_>) -> Option<String> {
    match node.kind() {
        kinds::STRING => {
            let s = tree::string_content(node, doc.source())?;
            if s.len() > 60 {
                Some(format!("\"{}...\"", &s[..57]))
            } else {
                Some(format!("\"{s}\""))
            }
        }
        kinds::NUMBER | kinds::TRUE | kinds::FALSE | kinds::NULL => {
            Some(node.utf8_text(doc.source()).ok()?.to_string())
        }
        kinds::OBJECT => {
            let mut cursor = node.walk();
            let count = tree::object_pairs(node, &mut cursor).len();
            Some(format!("{{{count} properties}}"))
        }
        kinds::ARRAY => {
            let mut cursor = node.walk();
            let count = tree::array_items(node, &mut cursor).len();
            Some(format!("[{count} items]"))
        }
        _ => None,
    }
}
