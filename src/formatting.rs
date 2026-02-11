/// JSON formatting and sorting.
///
/// Formatting walks the tree-sitter CST directly, avoiding a redundant
/// serde_json round-trip. Sorting still uses serde_json since tree-sitter
/// doesn't provide sorted iteration.
use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::kinds;

/// Format the entire document.
pub fn format_document(doc: &Document, options: &FormattingOptions) -> Vec<TextEdit> {
    // Only format if the document has no syntax errors.
    if doc.tree.root_node().has_error() {
        return Vec::new();
    }

    let formatted = match reformat(doc, options) {
        Some(f) => f,
        None => return Vec::new(),
    };

    if formatted == doc.text {
        return Vec::new();
    }

    let end = doc.position_of(doc.text.len());
    vec![TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end,
        },
        new_text: formatted,
    }]
}

/// Format a range within the document.
/// For simplicity, we format the whole document — most editors handle this fine.
pub fn format_range(doc: &Document, _range: Range, options: &FormattingOptions) -> Vec<TextEdit> {
    format_document(doc, options)
}

/// Sort all object properties alphabetically (recursive), preserving indent style.
/// Round-trips through serde_json to sort, then re-serializes with the document's
/// detected indentation.
pub fn sort_document(doc: &Document) -> Vec<TextEdit> {
    if doc.tree.root_node().has_error() {
        return Vec::new();
    }

    let val: serde_json::Value = match serde_json::from_str(&doc.text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let sorted = sort_value(&val);

    // Detect the current indent style from the document.
    let indent = detect_indent(&doc.text);
    let mut new_text = String::with_capacity(doc.text.len());
    format_serde_value(&sorted, &indent, 0, &mut new_text);
    new_text.push('\n');

    if new_text == doc.text {
        return Vec::new();
    }

    let end = doc.position_of(doc.text.len());
    vec![TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end,
        },
        new_text,
    }]
}

/// Detect the indentation style used in the document.
fn detect_indent(text: &str) -> String {
    for line in text.lines().skip(1) {
        if line.starts_with('\t') {
            return "\t".to_string();
        }
        let spaces = line.len() - line.trim_start_matches(' ').len();
        if spaces > 0 {
            return " ".repeat(spaces);
        }
    }
    "  ".to_string() // Default: 2 spaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn make_options(tab_size: u32, insert_spaces: bool) -> FormattingOptions {
        FormattingOptions {
            tab_size,
            insert_spaces,
            insert_final_newline: Some(true),
            ..FormattingOptions::default()
        }
    }

    #[test]
    fn format_compact_json() {
        let doc = Document::new(r#"{"a":1,"b":2}"#.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert_eq!(edits.len(), 1);
        let formatted = &edits[0].new_text;
        assert!(formatted.contains('\n'));
        assert!(formatted.contains("  \"a\": 1"));
    }

    #[test]
    fn format_with_tabs() {
        let doc = Document::new(r#"{"a":1}"#.into(), 0);
        let edits = format_document(&doc, &make_options(1, false));
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.contains('\t'));
    }

    #[test]
    fn format_with_4_spaces() {
        let doc = Document::new(r#"{"a":1}"#.into(), 0);
        let edits = format_document(&doc, &make_options(4, true));
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.contains("    \"a\""));
    }

    #[test]
    fn format_already_formatted() {
        let src = "{\n  \"a\": 1\n}\n";
        let doc = Document::new(src.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert!(edits.is_empty()); // No changes needed.
    }

    #[test]
    fn format_skips_syntax_errors() {
        let doc = Document::new(r#"{"a": }"#.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert!(edits.is_empty());
    }

    #[test]
    fn format_empty_object() {
        let doc = Document::new(r#"{}"#.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "{}\n");
    }

    #[test]
    fn format_empty_array() {
        let doc = Document::new(r#"[]"#.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "[]\n");
    }

    #[test]
    fn sort_document_alphabetical() {
        let doc = Document::new(r#"{"b": 2, "a": 1}"#.into(), 0);
        let edits = sort_document(&doc);
        assert_eq!(edits.len(), 1);
        let sorted = &edits[0].new_text;
        let a_pos = sorted.find("\"a\"").unwrap();
        let b_pos = sorted.find("\"b\"").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn sort_nested_objects() {
        let doc = Document::new(r#"{"z": {"b": 2, "a": 1}, "a": 1}"#.into(), 0);
        let edits = sort_document(&doc);
        assert_eq!(edits.len(), 1);
        let sorted = &edits[0].new_text;
        // Top level: "a" before "z".
        let a_pos = sorted.find("\"a\": 1").unwrap();
        let z_pos = sorted.find("\"z\"").unwrap();
        assert!(a_pos < z_pos);
    }

    #[test]
    fn sort_already_sorted() {
        let doc = Document::new("{\n  \"a\": 1,\n  \"b\": 2\n}\n".into(), 0);
        let edits = sort_document(&doc);
        assert!(edits.is_empty());
    }

    #[test]
    fn sort_skips_syntax_errors() {
        let doc = Document::new(r#"{"b": , "a": 1}"#.into(), 0);
        let edits = sort_document(&doc);
        assert!(edits.is_empty());
    }

    #[test]
    fn format_preserves_string_escapes() {
        let doc = Document::new(r#"{"msg":"hello\nworld"}"#.into(), 0);
        let edits = format_document(&doc, &make_options(2, true));
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.contains(r#"\n"#));
    }

    #[test]
    fn detect_indent_2_spaces() {
        assert_eq!(detect_indent("{\n  \"a\": 1\n}"), "  ");
    }

    #[test]
    fn detect_indent_4_spaces() {
        assert_eq!(detect_indent("{\n    \"a\": 1\n}"), "    ");
    }

    #[test]
    fn detect_indent_tabs() {
        assert_eq!(detect_indent("{\n\t\"a\": 1\n}"), "\t");
    }

    #[test]
    fn detect_indent_default() {
        assert_eq!(detect_indent("{}"), "  ");
    }
}

fn reformat(doc: &Document, options: &FormattingOptions) -> Option<String> {
    let root = doc.tree.root_node();
    let value_node = root
        .named_children(&mut root.walk())
        .find(|n| crate::tree::is_value_node(n))?;

    let insert_spaces = options.insert_spaces;
    let tab_size = options.tab_size as usize;
    let insert_final_newline = options.insert_final_newline.unwrap_or(true);

    let indent = if insert_spaces {
        " ".repeat(tab_size)
    } else {
        "\t".to_string()
    };

    let source = doc.source();
    let mut out = String::with_capacity(doc.text.len());
    format_node(value_node, source, &indent, 0, &mut out);

    if insert_final_newline && !out.ends_with('\n') {
        out.push('\n');
    }

    Some(out)
}

/// Recursively format a tree-sitter node, reading leaf text directly from source.
fn format_node(node: Node<'_>, source: &[u8], indent: &str, depth: usize, out: &mut String) {
    match node.kind() {
        kinds::OBJECT => {
            let mut cursor = node.walk();
            let pairs: Vec<Node<'_>> = node
                .named_children(&mut cursor)
                .filter(|n| n.kind() == kinds::PAIR)
                .collect();
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, pair) in pairs.iter().enumerate() {
                write_indent(out, indent, depth + 1);
                // Key: copy verbatim from source (preserves escapes).
                if let Some(key_node) = pair.child_by_field_name("key") {
                    let key_text = &source[key_node.start_byte()..key_node.end_byte()];
                    out.push_str(std::str::from_utf8(key_text).unwrap_or("\"\""));
                }
                out.push_str(": ");
                // Value.
                if let Some(val_node) = pair.child_by_field_name("value") {
                    format_node(val_node, source, indent, depth + 1, out);
                }
                if i < pairs.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent, depth);
            out.push('}');
        }
        kinds::ARRAY => {
            let mut cursor = node.walk();
            let items: Vec<Node<'_>> = node
                .named_children(&mut cursor)
                .filter(|n| crate::tree::is_value_node(n))
                .collect();
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                write_indent(out, indent, depth + 1);
                format_node(*item, source, indent, depth + 1, out);
                if i < items.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent, depth);
            out.push(']');
        }
        // Leaf nodes: copy verbatim from source.
        // This preserves string escapes, number formats, etc. exactly.
        kinds::STRING | kinds::NUMBER | kinds::TRUE | kinds::FALSE | kinds::NULL => {
            let text = &source[node.start_byte()..node.end_byte()];
            out.push_str(std::str::from_utf8(text).unwrap_or("null"));
        }
        _ => {
            // Fallback: copy raw text for unexpected node kinds.
            let text = &source[node.start_byte()..node.end_byte()];
            out.push_str(std::str::from_utf8(text).unwrap_or(""));
        }
    }
}

fn write_indent(out: &mut String, indent: &str, depth: usize) {
    for _ in 0..depth {
        out.push_str(indent);
    }
}

fn escape_json_string(s: &str, out: &mut String) {
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\u{0020}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

/// serde_json-based formatter used only by sort_document (which needs
/// reordered keys that tree-sitter can't provide).
fn format_serde_value(val: &serde_json::Value, indent: &str, depth: usize, out: &mut String) {
    match val {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => out.push_str(&n.to_string()),
        serde_json::Value::String(s) => {
            out.push('"');
            escape_json_string(s, out);
            out.push('"');
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in arr.iter().enumerate() {
                write_indent(out, indent, depth + 1);
                format_serde_value(item, indent, depth + 1, out);
                if i < arr.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent, depth);
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let entries: Vec<_> = map.iter().collect();
            for (i, (key, value)) in entries.iter().enumerate() {
                write_indent(out, indent, depth + 1);
                out.push('"');
                escape_json_string(key, out);
                out.push_str("\": ");
                format_serde_value(value, indent, depth + 1, out);
                if i < entries.len() - 1 {
                    out.push(',');
                }
                out.push('\n');
            }
            write_indent(out, indent, depth);
            out.push('}');
        }
    }
}

fn sort_value(val: &serde_json::Value) -> serde_json::Value {
    match val {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(String, serde_json::Value)> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_value(v)))
                .collect();
            sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_value).collect())
        }
        other => other.clone(),
    }
}
