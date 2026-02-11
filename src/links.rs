/// Document links: detects `$ref` values and URLs in JSON strings.
/// Also provides go-to-definition for `$ref` within the same document.
use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::{self, kinds};

/// Find all document links (URLs and $ref values).
pub fn document_links(doc: &Document) -> Vec<DocumentLink> {
    let mut links = Vec::new();
    let root = doc.tree.root_node();
    collect_links(doc, root, &mut links);
    links
}

/// Resolve a `$ref` to a definition location within the same document.
pub fn find_definition(doc: &Document, offset: usize) -> Option<Location> {
    let mut node = tree::node_at_offset(&doc.tree, offset)?;

    // Walk up to the string node if we landed on string_content or escape_sequence.
    if node.kind() == kinds::STRING_CONTENT || node.kind() == kinds::ESCAPE_SEQUENCE {
        node = node.parent().filter(|p| p.kind() == kinds::STRING)?;
    }

    // Must be a string node.
    if node.kind() != kinds::STRING {
        return None;
    }

    // Must be the value of a "$ref" pair.
    let pair = node.parent()?;
    if pair.kind() != kinds::PAIR {
        return None;
    }
    let key_node = pair.child_by_field_name("key")?;
    if tree::string_content(key_node, doc.source())? != "$ref" {
        return None;
    }

    let ref_value = tree::string_value(node, doc.source())?;

    // Only handle internal refs (starting with #).
    let fragment = ref_value.strip_prefix('#')?;
    let target = resolve_pointer(doc, fragment)?;

    let range = doc.range_of(target.start_byte(), target.end_byte());
    // Placeholder URI — the server's goto_definition handler replaces it
    // with the actual document URI before returning to the client.
    Some(Location {
        uri: Url::parse("file:///placeholder").unwrap(),
        range,
    })
}

fn collect_links(doc: &Document, node: Node<'_>, links: &mut Vec<DocumentLink>) {
    if node.kind() == kinds::PAIR
        && let Some(value_node) = tree::pair_value(node)
        && value_node.kind() == kinds::STRING
        && let Some(val) = tree::string_value(value_node, doc.source())
    {
        let key = tree::pair_key(node, doc.source()).unwrap_or("");

        // $ref links.
        if key == "$ref" {
            let range = doc.range_of(value_node.start_byte(), value_node.end_byte());
            let target = if val.starts_with("http://") || val.starts_with("https://") {
                Url::parse(&val).ok()
            } else {
                None
            };
            links.push(DocumentLink {
                range,
                target,
                tooltip: Some("Go to definition".into()),
                data: None,
            });
        }
        // URLs in any string value.
        else if (val.starts_with("http://") || val.starts_with("https://"))
            && let Ok(url) = Url::parse(&val)
        {
            let range = doc.range_of(value_node.start_byte(), value_node.end_byte());
            links.push(DocumentLink {
                range,
                target: Some(url),
                tooltip: None,
                data: None,
            });
        }
    }

    // Recurse.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_links(doc, child, links);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn detect_ref_link() {
        let doc = Document::new(r##"{"$ref": "#/definitions/Foo"}"##.into(), 0);
        let links = document_links(&doc);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].tooltip.as_deref(), Some("Go to definition"));
    }

    #[test]
    fn detect_url_link() {
        let doc = Document::new(r#"{"homepage": "https://example.com"}"#.into(), 0);
        let links = document_links(&doc);
        assert_eq!(links.len(), 1);
        assert!(links[0].target.is_some());
    }

    #[test]
    fn detect_http_ref() {
        let doc = Document::new(r#"{"$ref": "https://example.com/schema.json"}"#.into(), 0);
        let links = document_links(&doc);
        assert_eq!(links.len(), 1);
        assert!(links[0].target.is_some());
    }

    #[test]
    fn no_links_in_plain_json() {
        let doc = Document::new(r#"{"name": "Alice", "age": 30}"#.into(), 0);
        let links = document_links(&doc);
        assert!(links.is_empty());
    }

    #[test]
    fn find_definition_internal_ref() {
        let doc = Document::new(
            r##"{"definitions": {"Foo": {"type": "string"}}, "value": {"$ref": "#/definitions/Foo"}}"##
                .into(),
            0,
        );
        // Find the $ref string node offset.
        // The $ref value starts at the last "#/definitions/Foo".
        let ref_str_start = doc.text.rfind(r##""#/definitions/Foo""##).unwrap();
        let loc = find_definition(&doc, ref_str_start + 1);
        assert!(loc.is_some());
    }

    #[test]
    fn find_definition_not_a_ref() {
        let doc = Document::new(r#"{"name": "Alice"}"#.into(), 0);
        let loc = find_definition(&doc, 10); // Inside "Alice"
        assert!(loc.is_none());
    }

    #[test]
    fn find_definition_external_ref_returns_none() {
        let doc = Document::new(r##"{"$ref": "other.json#/foo"}"##.into(), 0);
        // External refs (no leading #) return None.
        let loc = find_definition(&doc, 10);
        assert!(loc.is_none());
    }

    #[test]
    fn resolve_pointer_root() {
        let doc = Document::new(r#"{"a": 1}"#.into(), 0);
        let node = resolve_pointer(&doc, "");
        assert!(node.is_some());
    }

    #[test]
    fn resolve_pointer_nested() {
        let doc = Document::new(r#"{"a": {"b": 42}}"#.into(), 0);
        let node = resolve_pointer(&doc, "/a/b");
        assert!(node.is_some());
        assert_eq!(node.unwrap().utf8_text(doc.source()).unwrap(), "42");
    }

    #[test]
    fn resolve_pointer_array_index() {
        let doc = Document::new(r#"{"arr": [10, 20, 30]}"#.into(), 0);
        let node = resolve_pointer(&doc, "/arr/1");
        assert!(node.is_some());
        assert_eq!(node.unwrap().utf8_text(doc.source()).unwrap(), "20");
    }

    #[test]
    fn resolve_pointer_invalid_path() {
        let doc = Document::new(r#"{"a": 1}"#.into(), 0);
        let node = resolve_pointer(&doc, "/nonexistent");
        assert!(node.is_none());
    }
}

/// Resolve a JSON Pointer path (e.g. `/definitions/Foo`) within the tree.
fn resolve_pointer<'a>(doc: &'a Document, pointer: &str) -> Option<Node<'a>> {
    let path = pointer.strip_prefix('/').unwrap_or(pointer);
    if path.is_empty() {
        return tree::root_value(&doc.tree);
    }

    let segments: Vec<&str> = path.split('/').collect();
    let mut current = tree::root_value(&doc.tree)?;

    for segment in segments {
        let decoded = segment.replace("~1", "/").replace("~0", "~");

        match current.kind() {
            kinds::OBJECT => {
                let mut cursor = current.walk();
                let pairs = tree::object_pairs(current, &mut cursor);
                let mut found = false;
                for pair in pairs {
                    if let Some(key) = tree::pair_key_unescaped(pair, doc.source())
                        && key == decoded
                    {
                        current = tree::pair_value(pair)?;
                        found = true;
                        break;
                    }
                }
                if !found {
                    return None;
                }
            }
            kinds::ARRAY => {
                let idx: usize = decoded.parse().ok()?;
                let mut cursor = current.walk();
                let items = tree::array_items(current, &mut cursor);
                current = *items.get(idx)?;
            }
            _ => return None,
        }
    }

    Some(current)
}
