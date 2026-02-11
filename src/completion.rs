/// Schema-driven completion provider.
///
/// Determines completion context from the tree-sitter node at the cursor,
/// walks the schema to find the relevant sub-schema, and produces completion
/// items for property names, values, enum members, and snippets.
use std::collections::HashSet;
use std::sync::Arc;

use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::schema::types::*;
use crate::tree::{self, kinds};

/// Produce completion items at the given byte offset.
pub fn completions(
    doc: &Document,
    offset: usize,
    schema: Option<&Arc<JsonSchema>>,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    let ctx = determine_context(doc, offset);

    match ctx {
        Context::PropertyName { object } => {
            if let Some(schema) = schema {
                let sub = resolve_schema_for_node(doc, object, schema);
                complete_property_names(doc, object, &sub, &mut items);
            }
        }
        Context::PropertyValue { object, key } => {
            if let Some(schema) = schema {
                let sub = resolve_schema_for_node(doc, object, schema);
                if let Some(prop_schema) = sub.properties.get(&key) {
                    complete_value(prop_schema, &mut items);
                }
            }
            // Always offer structural completions.
            items.push(snippet_item("{ }", "Empty object", "{$1}"));
            items.push(snippet_item("[ ]", "Empty array", "[$1]"));
        }
        Context::ArrayItem { array, index } => {
            if let Some(schema) = schema {
                let sub = resolve_schema_for_node(doc, array, schema);
                if let Some(item_schema) = array_item_schema(&sub, index) {
                    complete_value(&item_schema, &mut items);
                }
            }
        }
        Context::None => {}
    }

    items
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

enum Context<'a> {
    PropertyName { object: Node<'a> },
    PropertyValue { object: Node<'a>, key: String },
    ArrayItem { array: Node<'a>, index: usize },
    None,
}

/// Figure out what kind of completion the cursor position needs.
/// The tree-sitter node at the cursor tells us the structural context:
/// - Inside an object -> property name or value (determined by colon position)
/// - Inside an array -> array item
/// - On a string/pair/leaf -> walk up to find the containing structure
fn determine_context<'a>(doc: &'a Document, offset: usize) -> Context<'a> {
    let node = match tree::node_at_offset(&doc.tree, offset) {
        Some(n) => n,
        None => return Context::None,
    };

    match node.kind() {
        kinds::OBJECT => {
            // Cursor is on whitespace inside an object. Use text scanning to
            // determine if we're before or after a colon (key vs value position).
            let text_before = &doc.text[..offset];
            if let Some(colon_pos) = text_before.rfind(':') {
                let last_sep = text_before
                    .rfind(',')
                    .max(text_before.rfind('{'))
                    .unwrap_or(0);
                if colon_pos > last_sep
                    && let Some(key) = find_key_at_colon(doc, node, colon_pos)
                {
                    return Context::PropertyValue { object: node, key };
                }
            }
            Context::PropertyName { object: node }
        }
        kinds::ARRAY => {
            let mut cursor = node.walk();
            let count = tree::array_items(node, &mut cursor).len();
            Context::ArrayItem {
                array: node,
                index: count,
            }
        }
        kinds::STRING => {
            // Could be a key or a value.
            if let Some(pair) = node.parent().filter(|p| p.kind() == kinds::PAIR) {
                let is_key = pair
                    .child_by_field_name("key")
                    .is_some_and(|k| k.id() == node.id());
                if is_key {
                    if let Some(object) = pair.parent().filter(|p| p.kind() == kinds::OBJECT) {
                        return Context::PropertyName { object };
                    }
                } else {
                    // Value string.
                    if let Some(object) = pair.parent().filter(|p| p.kind() == kinds::OBJECT) {
                        let key = tree::pair_key_unescaped(pair, doc.source()).unwrap_or_default();
                        return Context::PropertyValue { object, key };
                    }
                }
            }
            Context::None
        }
        kinds::PAIR => {
            // Cursor might be between key and value.
            if let Some(object) = node.parent().filter(|p| p.kind() == kinds::OBJECT) {
                let key = tree::pair_key_unescaped(node, doc.source()).unwrap_or_default();
                return Context::PropertyValue { object, key };
            }
            Context::None
        }
        _ => {
            // Leaf value — check parent.
            if let Some(pair) = node.parent().filter(|p| p.kind() == kinds::PAIR)
                && let Some(object) = pair.parent().filter(|p| p.kind() == kinds::OBJECT)
            {
                let key = tree::pair_key_unescaped(pair, doc.source()).unwrap_or_default();
                return Context::PropertyValue { object, key };
            }
            if let Some(array) = node.parent().filter(|p| p.kind() == kinds::ARRAY) {
                let mut cursor = array.walk();
                let items = tree::array_items(array, &mut cursor);
                let index = items.iter().position(|n| n.id() == node.id()).unwrap_or(0);
                return Context::ArrayItem { array, index };
            }
            Context::None
        }
    }
}

/// Given a colon byte position inside an object, find which key it belongs to.
/// Scans all pairs in the object to find the colon node closest to `colon_byte`.
fn find_key_at_colon(doc: &Document, object: Node<'_>, colon_byte: usize) -> Option<String> {
    let mut cursor = object.walk();
    let pairs = tree::object_pairs(object, &mut cursor);
    let mut best: Option<(usize, String)> = None;
    for pair in pairs {
        let mut pc = pair.walk();
        for child in pair.children(&mut pc) {
            if child.kind() == ":" && child.start_byte() <= colon_byte {
                let dist = colon_byte - child.start_byte();
                if best.as_ref().is_none_or(|(d, _)| dist < *d)
                    && let Some(k) = tree::pair_key_unescaped(pair, doc.source())
                {
                    best = Some((dist, k));
                }
            }
        }
    }
    best.map(|(_, k)| k)
}

// ---------------------------------------------------------------------------
// Schema resolution for a node
// ---------------------------------------------------------------------------

fn resolve_schema_for_node(
    doc: &Document,
    node: Node<'_>,
    root_schema: &Arc<JsonSchema>,
) -> Arc<JsonSchema> {
    let path = tree::json_path(node, doc.source());
    let mut current = root_schema.clone();
    for seg in &path {
        current = current
            .resolve_path_segment(seg)
            .unwrap_or_else(|| Arc::new(JsonSchema::default()));
    }
    current
}

fn array_item_schema(schema: &JsonSchema, index: usize) -> Option<Arc<JsonSchema>> {
    if let Some(ps) = schema.prefix_items.get(index) {
        return Some(ps.clone());
    }
    schema.items.as_ref()?.as_schema().cloned()
}

// ---------------------------------------------------------------------------
// Completion generation
// ---------------------------------------------------------------------------

fn complete_property_names(
    doc: &Document,
    object: Node<'_>,
    schema: &JsonSchema,
    items: &mut Vec<CompletionItem>,
) {
    // Collect existing keys.
    let mut cursor = object.walk();
    let pairs = tree::object_pairs(object, &mut cursor);
    let existing: HashSet<String> = pairs
        .iter()
        .filter_map(|p| tree::pair_key_unescaped(*p, doc.source()))
        .collect();

    for (key, prop_schema) in &schema.properties {
        if existing.contains(key) || prop_schema.do_not_suggest {
            continue;
        }

        let detail = prop_schema
            .types
            .first()
            .map(|t| format!("{t:?}").to_lowercase());

        let documentation = prop_schema
            .markdown_description
            .as_deref()
            .or(prop_schema.description.as_deref())
            .map(|d| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: d.to_string(),
                })
            });

        let is_required = schema.required.contains(key);
        let default_value = default_value_snippet(prop_schema);
        let insert_text = format!("\"{key}\": {default_value}");

        let mut item = CompletionItem {
            label: key.clone(),
            kind: Some(CompletionItemKind::PROPERTY),
            detail,
            documentation,
            insert_text: Some(insert_text),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(if is_required {
                format!("0_{key}")
            } else {
                format!("1_{key}")
            }),
            ..CompletionItem::default()
        };

        if prop_schema.deprecated {
            item.deprecated = Some(true);
            item.tags = Some(vec![CompletionItemTag::DEPRECATED]);
        }

        items.push(item);
    }

    // allOf: merge properties from all subschemas.
    for sub in &schema.all_of {
        complete_property_names(doc, object, sub, items);
    }

    // anyOf/oneOf: show union of all branch completions.
    for sub in schema.any_of.iter().chain(schema.one_of.iter()) {
        complete_property_names(doc, object, sub, items);
    }

    // if/then/else: offer from relevant branch.
    if let Some(ref then_schema) = schema.then_schema {
        complete_property_names(doc, object, then_schema, items);
    }
    if let Some(ref else_schema) = schema.else_schema {
        complete_property_names(doc, object, else_schema, items);
    }

    // Default snippets from schema.
    for snip in &schema.default_snippets {
        if let Some(ref body) = snip.body {
            let label = snip.label.as_deref().unwrap_or("snippet");
            let insert = serde_json::to_string_pretty(body).unwrap_or_default();
            items.push(CompletionItem {
                label: label.to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: snip.description.clone(),
                insert_text: Some(insert),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..CompletionItem::default()
            });
        }
    }
}

fn complete_value(schema: &JsonSchema, items: &mut Vec<CompletionItem>) {
    // Enum values with optional descriptions.
    for (i, val) in schema.enum_values.iter().enumerate() {
        let label = format_json_value(val);
        let doc = schema
            .markdown_enum_descriptions
            .get(i)
            .or_else(|| schema.enum_descriptions.get(i))
            .map(|d| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: d.clone(),
                })
            });
        items.push(CompletionItem {
            label: label.clone(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            documentation: doc,
            insert_text: Some(label),
            ..CompletionItem::default()
        });
    }

    if let Some(ref c) = schema.const_value {
        let label = format_json_value(c);
        items.push(value_item(&label));
    }

    if schema.types.contains(&SchemaType::Boolean) && schema.enum_values.is_empty() {
        items.push(value_item("true"));
        items.push(value_item("false"));
    }
    if schema.types.contains(&SchemaType::Null) && schema.enum_values.is_empty() {
        items.push(value_item("null"));
    }
    if let Some(ref def) = schema.default {
        let label = format_json_value(def);
        items.push(CompletionItem {
            label: format!("{label} (default)"),
            kind: Some(CompletionItemKind::VALUE),
            insert_text: Some(label),
            preselect: Some(true),
            ..CompletionItem::default()
        });
    }

    for snip in &schema.default_snippets {
        if let Some(ref body) = snip.body {
            let label = snip.label.as_deref().unwrap_or("snippet");
            let insert = serde_json::to_string_pretty(body).unwrap_or_default();
            items.push(CompletionItem {
                label: label.to_string(),
                kind: Some(CompletionItemKind::SNIPPET),
                detail: snip.description.clone(),
                insert_text: Some(insert),
                insert_text_format: Some(InsertTextFormat::SNIPPET),
                ..CompletionItem::default()
            });
        }
    }
}

fn default_value_snippet(schema: &JsonSchema) -> String {
    if let Some(ref c) = schema.const_value {
        return format_json_value(c);
    }
    if schema.enum_values.len() == 1 {
        return format_json_value(&schema.enum_values[0]);
    }
    if let Some(ref def) = schema.default {
        return format_json_value(def);
    }
    match schema.types.first() {
        Some(SchemaType::String) => "\"$1\"".into(),
        Some(SchemaType::Number | SchemaType::Integer) => "${1:0}".into(),
        Some(SchemaType::Boolean) => "${1:false}".into(),
        Some(SchemaType::Null) => "null".into(),
        Some(SchemaType::Array) => "[$1]".into(),
        Some(SchemaType::Object) => "{$1}".into(),
        None => "$1".into(),
    }
}

fn format_json_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

fn value_item(label: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::VALUE),
        insert_text: Some(label.to_string()),
        ..CompletionItem::default()
    }
}

fn snippet_item(label: &str, detail: &str, insert: &str) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(CompletionItemKind::STRUCT),
        detail: Some(detail.to_string()),
        insert_text: Some(insert.to_string()),
        insert_text_format: Some(InsertTextFormat::SNIPPET),
        ..CompletionItem::default()
    }
}
