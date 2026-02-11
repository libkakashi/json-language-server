/// Schema-driven hover provider.
///
/// Shows schema description, type, default, enum values, and JSON path
/// when hovering over a key or value.
use std::sync::Arc;

use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::schema::types::*;
use crate::tree::{self, kinds};

/// Produce hover information at a byte offset.
pub fn hover(doc: &Document, offset: usize, schema: Option<&Arc<JsonSchema>>) -> Option<Hover> {
    let node = tree::node_at_offset(&doc.tree, offset)?;

    let mut sections: Vec<String> = Vec::new();

    // JSON path.
    let path = tree::json_pointer(node, doc.source());
    if !path.is_empty() {
        sections.push(format!("`{path}`"));
    }

    // Schema info.
    if let Some(root_schema) = schema
        && let Some(sub) = resolve_schema_for_hover(doc, node, root_schema)
    {
        if let Some(desc) = sub
            .markdown_description
            .as_deref()
            .or(sub.description.as_deref())
        {
            sections.push(desc.to_string());
        }

        if !sub.types.is_empty() {
            let types: Vec<&str> = sub.types.iter().map(|t| t.as_str()).collect();
            sections.push(format!("Type: `{}`", types.join(" | ")));
        }

        if let Some(ref def) = sub.default {
            sections.push(format!("Default: `{def}`"));
        }

        if !sub.enum_values.is_empty() && sub.enum_values.len() <= 20 {
            let vals: Vec<String> = sub.enum_values.iter().map(|v| format!("`{v}`")).collect();
            sections.push(format!("Allowed values: {}", vals.join(", ")));
        }

        if sub.deprecated {
            let msg = sub.deprecation_message.as_deref().unwrap_or("Deprecated");
            sections.push(format!("**Deprecated:** {msg}"));
        }
    }

    // Current value.
    match node.kind() {
        kinds::STRING => {
            if let Some(s) = tree::string_value(node, doc.source())
                && s.len() < 200
            {
                sections.push(format!("Value: `\"{s}\"`"));
            }
        }
        kinds::NUMBER | kinds::TRUE | kinds::FALSE | kinds::NULL => {
            if let Ok(text) = node.utf8_text(doc.source()) {
                sections.push(format!("Value: `{text}`"));
            }
        }
        _ => {}
    }

    if sections.is_empty() {
        return None;
    }

    let range = doc.range_of(node.start_byte(), node.end_byte());

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: sections.join("\n\n"),
        }),
        range: Some(range),
    })
}

fn resolve_schema_for_hover(
    doc: &Document,
    node: Node<'_>,
    root_schema: &Arc<JsonSchema>,
) -> Option<Arc<JsonSchema>> {
    let path = tree::json_path(node, doc.source());
    let mut current = root_schema.clone();
    for seg in &path {
        current = current.resolve_path_segment(seg)?;
    }
    Some(current)
}
