/// Diagnostics engine: produces LSP Diagnostics from tree-sitter ERROR/MISSING
/// nodes and detects duplicate keys in objects.
use std::collections::HashSet;

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::{self, kinds};

/// Produce all syntax diagnostics for a document.
pub fn syntax_diagnostics(doc: &Document) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let root = doc.tree.root_node();

    if root.has_error() {
        collect_errors(doc, root, &mut diags);
    }
    collect_duplicate_keys(doc, root, &mut diags);

    diags
}

/// Recursively collect ERROR and MISSING nodes, skipping trailing commas.
/// tree-sitter-json treats trailing commas as errors, but they are widely
/// accepted in practice (JSONC, tsconfig, VS Code settings), so we tolerate
/// them silently.
fn collect_errors(doc: &Document, node: Node<'_>, diags: &mut Vec<Diagnostic>) {
    if node.is_error() {
        if is_trailing_comma_error(node, doc.source()) {
            return;
        }
        let range = doc.range_of(node.start_byte(), node.end_byte());
        let message = describe_error(node, doc.source());
        diags.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("json".into()),
            message,
            ..Diagnostic::default()
        });
        return; // Don't recurse into ERROR nodes — they're already reported.
    }

    if node.is_missing() {
        if is_trailing_comma_missing(node) {
            return;
        }
        let range = doc.range_of(node.start_byte(), node.start_byte());
        let message = describe_missing(node);
        diags.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("json".into()),
            message,
            ..Diagnostic::default()
        });
        return;
    }

    // Recurse into children.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error() || child.is_error() || child.is_missing() {
            collect_errors(doc, child, diags);
        }
    }
}

// ---------------------------------------------------------------------------
// Trailing comma detection
// ---------------------------------------------------------------------------

/// Detect trailing comma as an ERROR node: the node text must be exactly ","
/// (plus whitespace), it must be followed by a closing bracket, and it must
/// be preceded by an actual element (not by an opening bracket, which would
/// make it a leading comma like `{,}`).
fn is_trailing_comma_error(node: Node<'_>, source: &[u8]) -> bool {
    let text = &source[node.start_byte()..node.end_byte()];
    // Must be exactly one comma (possibly surrounded by whitespace).
    let comma_count = text.iter().filter(|&&b| b == b',').count();
    if comma_count != 1 || !text.iter().all(|&b| b == b',' || b.is_ascii_whitespace()) {
        return false;
    }
    // Must be followed by a closing bracket.
    let next_is_close = node
        .next_sibling()
        .is_some_and(|s| s.kind() == "}" || s.kind() == "]");
    // Must be preceded by an actual element, not an opening bracket.
    let prev_is_element = node.prev_sibling().is_some_and(|p| {
        matches!(
            p.kind(),
            kinds::PAIR
                | ","
                | kinds::OBJECT
                | kinds::ARRAY
                | kinds::STRING
                | kinds::NUMBER
                | kinds::TRUE
                | kinds::FALSE
                | kinds::NULL
        ) && p.kind() != "{"
            && p.kind() != "["
            && p.kind() != "{"
    });
    next_is_close && prev_is_element
}

/// Detect trailing comma in arrays: tree-sitter sometimes represents this as
/// a regular "," followed by a MISSING value node before "]". Only suppress
/// when the previous sibling is a genuine comma and the one before that is a
/// real value (not another comma or opening bracket).
fn is_trailing_comma_missing(node: Node<'_>) -> bool {
    let Some(comma) = node.prev_sibling() else {
        return false;
    };
    if comma.kind() != "," {
        return false;
    }
    let Some(closing) = node.next_sibling() else {
        return false;
    };
    if closing.kind() != "]" {
        return false;
    }
    // The element before the comma must be a real value, not another comma or "[".
    comma
        .prev_sibling()
        .is_some_and(|p| tree::is_value_node(&p))
}

// ---------------------------------------------------------------------------
// Descriptive error messages
// ---------------------------------------------------------------------------

/// Produce a human-readable message for an ERROR node by inspecting its
/// children and surrounding context.
fn describe_error(node: Node<'_>, source: &[u8]) -> String {
    let text = node_text(node, source);

    // Check what children the ERROR node contains.
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    let child_kinds: Vec<&str> = children.iter().map(|c| c.kind()).collect();

    let has_colon = child_kinds.contains(&":");
    let has_comma = child_kinds.contains(&",");

    // Patterns involving colon — distinguish "missing value" from "missing key".
    if has_colon {
        let string_pos = children.iter().position(|c| c.kind() == kinds::STRING);
        let colon_pos = children.iter().position(|c| c.kind() == ":").unwrap();

        if let Some(sp) = string_pos {
            if sp < colon_pos {
                // "key": , or "key": → string before colon means value is missing.
                return "Expected a value.".into();
            }
            // : "value" → string after colon means key is missing.
            return "Expected a property name.".into();
        }

        // Colon present but no string at all — check for unquoted/single-quoted key.
        if let Some(err_child) = children.iter().find(|c| c.is_error()) {
            let err_text = node_text(*err_child, source);
            if err_text.starts_with('\'') {
                return "Single-quoted strings are not allowed in JSON. Use double quotes.".into();
            }
        }
        // Colon with no recognizable key or value.
        return "Expected a property name before \":\".".into();
    }

    // ERROR at document root containing "{" or "[" → broken object/array start,
    // typically unquoted or single-quoted keys like `{a:` or `{'a':`.
    if (child_kinds.contains(&"{") || child_kinds.contains(&"["))
        && let Some(err_child) = children.iter().find(|c| c.is_error())
    {
        let err_text = node_text(*err_child, source);
        if err_text.starts_with('\'') {
            return "Single-quoted strings are not allowed in JSON. Use double quotes.".into();
        }
        if err_text
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            return format!(
                "Unexpected token \"{err_text}\". Property keys must be double-quoted."
            );
        }
    }

    // ERROR containing a pair node → missing comma between properties
    if child_kinds.contains(&kinds::PAIR) {
        return "Expected comma before this property.".into();
    }

    // ERROR that is just a closing bracket → extra closing bracket
    if text == "}" || text == "]" {
        return format!("Unexpected \"{text}\".");
    }

    // ERROR with only commas (not trailing — those are filtered earlier)
    if child_kinds.iter().all(|&k| k == ",") && !child_kinds.is_empty() {
        if has_comma && child_kinds.len() > 1 {
            return "Unexpected comma.".into();
        }
        // Single comma in an unexpected position
        return "Unexpected comma.".into();
    }

    // Fallback: show the unexpected text (truncated).
    if text.len() <= 20 {
        format!("Unexpected token \"{text}\".")
    } else {
        format!("Unexpected token \"{}\"...", &text[..20])
    }
}

/// Produce a human-readable message for a MISSING node.
fn describe_missing(node: Node<'_>) -> String {
    match node.kind() {
        "}" => "Expected closing \"}\".".into(),
        "]" => "Expected closing \"]\".".into(),
        ":" => "Expected \":\" after property key.".into(),
        "\"" => "Expected closing quote.".into(),
        kinds::STRING => "Expected a property name.".into(),
        _ => {
            // MISSING value inside a pair → "Expected a value" (e.g. `"key": }`)
            if node.parent().is_some_and(|p| p.kind() == kinds::PAIR) {
                return "Expected a value.".into();
            }
            // Inside an object → expected property or closing brace.
            if node.parent().is_some_and(|p| p.kind() == kinds::OBJECT) {
                return "Expected a property name or \"}\".".into();
            }
            // Inside an array → expected value or closing bracket.
            if node.parent().is_some_and(|p| p.kind() == kinds::ARRAY) {
                return "Expected a value or \"]\".".into();
            }
            format!("Expected {}.", node.kind())
        }
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.start_byte()..node.end_byte()]).unwrap_or("<invalid>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use tower_lsp::lsp_types::DiagnosticSeverity;

    fn errors(src: &str) -> Vec<Diagnostic> {
        let doc = Document::new(src.into(), 0);
        syntax_diagnostics(&doc)
    }

    fn error_messages(src: &str) -> Vec<String> {
        errors(src).into_iter().map(|d| d.message).collect()
    }

    // --- Valid JSON: no diagnostics ---

    #[test]
    fn valid_json_no_diagnostics() {
        assert!(errors(r#"{"a": 1, "b": [1, 2, 3]}"#).is_empty());
    }

    #[test]
    fn empty_object_no_diagnostics() {
        assert!(errors("{}").is_empty());
    }

    #[test]
    fn empty_array_no_diagnostics() {
        assert!(errors("[]").is_empty());
    }

    // --- Trailing commas: tolerated ---

    #[test]
    fn trailing_comma_object() {
        assert!(errors(r#"{"a": 1,}"#).is_empty());
    }

    #[test]
    fn trailing_comma_object_multiline() {
        assert!(errors("{\n  \"a\": 1,\n  \"b\": 2,\n}").is_empty());
    }

    #[test]
    fn trailing_comma_array_single() {
        assert!(errors("[1,]").is_empty());
    }

    #[test]
    fn trailing_comma_array_multi() {
        assert!(errors("[1, 2, 3,]").is_empty());
    }

    #[test]
    fn trailing_comma_array_multiline() {
        assert!(errors("[\n  1,\n  2,\n]").is_empty());
    }

    #[test]
    fn trailing_comma_nested() {
        assert!(errors(r#"{"a": [1,], "b": {"c": 2,},}"#).is_empty());
    }

    #[test]
    fn trailing_comma_tsconfig_style() {
        assert!(errors("{\n  \"compilerOptions\": {\n    \"strict\": true,\n  },\n}").is_empty());
    }

    #[test]
    fn trailing_comma_with_comment() {
        assert!(errors("{\n  // comment\n  \"a\": 1,\n}").is_empty());
    }

    // --- Comments: tolerated ---

    #[test]
    fn line_comment_tolerated() {
        assert!(errors("{\n  // line comment\n  \"a\": 1\n}").is_empty());
    }

    #[test]
    fn block_comment_tolerated() {
        assert!(errors("{ /* block */ \"a\": 1 }").is_empty());
    }

    // --- Double/leading commas: NOT tolerated ---

    #[test]
    fn double_comma_not_tolerated() {
        let msgs = error_messages(r#"{"a": 1,,}"#);
        assert!(!msgs.is_empty());
        assert!(msgs.iter().any(|m| m.contains("comma")));
    }

    #[test]
    fn leading_comma_not_tolerated() {
        let msgs = error_messages("{,}");
        assert!(!msgs.is_empty());
    }

    // --- Descriptive error messages ---

    #[test]
    fn error_missing_value_with_space() {
        let msgs = error_messages(r#"{"a": }"#);
        assert!(msgs.iter().any(|m| m.contains("Expected a value")));
    }

    #[test]
    fn error_missing_value_no_space() {
        let msgs = error_messages(r#"{"a":}"#);
        assert!(msgs.iter().any(|m| m.contains("Expected a value")));
    }

    #[test]
    fn error_missing_value_with_comma() {
        let msgs = error_messages(r#"{"arguments": ,}"#);
        assert!(msgs.iter().any(|m| m.contains("Expected a value")));
    }

    #[test]
    fn error_missing_value_then_next_prop() {
        let msgs = error_messages(r#"{"a": , "b": 2}"#);
        assert!(msgs.iter().any(|m| m.contains("Expected a value")));
    }

    #[test]
    fn error_missing_key() {
        let msgs = error_messages(r#"{: "value"}"#);
        assert!(msgs.iter().any(|m| m.contains("property name")));
    }

    #[test]
    fn error_missing_close_brace() {
        let msgs = error_messages(r#"{"a": 1"#);
        assert!(msgs.iter().any(|m| m.contains("}")));
    }

    #[test]
    fn error_missing_close_bracket() {
        let msgs = error_messages(r#"[1, 2"#);
        assert!(msgs.iter().any(|m| m.contains("]")));
    }

    #[test]
    fn error_extra_close_brace() {
        let msgs = error_messages(r#"{"a": 1}}"#);
        assert!(msgs.iter().any(|m| m.contains("Unexpected")));
    }

    #[test]
    fn error_unquoted_key() {
        let msgs = error_messages(r#"{a: 1}"#);
        assert!(
            msgs.iter()
                .any(|m| m.contains("double-quoted") || m.contains("Unexpected"))
        );
    }

    #[test]
    fn error_single_quoted_key() {
        let msgs = error_messages(r#"{'a': 1}"#);
        assert!(
            msgs.iter()
                .any(|m| m.contains("Single-quoted") || m.contains("double quotes"))
        );
    }

    #[test]
    fn error_missing_comma_between_properties() {
        let msgs = error_messages(r#"{"a": 1 "b": 2}"#);
        assert!(msgs.iter().any(|m| m.contains("comma")));
    }

    // --- Duplicate keys ---

    #[test]
    fn duplicate_keys_detected() {
        let msgs = error_messages(r#"{"a": 1, "a": 2}"#);
        assert!(msgs.iter().any(|m| m.contains("Duplicate key")));
    }

    #[test]
    fn duplicate_keys_nested() {
        let diags = errors(r#"{"outer": {"a": 1, "a": 2}}"#);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Duplicate key"))
            .collect();
        assert_eq!(dups.len(), 1);
    }

    #[test]
    fn no_duplicate_keys_different_objects() {
        let diags = errors(r#"{"a": {"a": 1}}"#);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Duplicate key"))
            .collect();
        assert_eq!(dups.len(), 0);
    }

    // --- Severity checks ---

    #[test]
    fn syntax_errors_are_error_severity() {
        let diags = errors(r#"{a: 1}"#);
        assert!(
            diags
                .iter()
                .all(|d| d.severity == Some(DiagnosticSeverity::ERROR))
        );
    }

    #[test]
    fn duplicate_keys_are_warning_severity() {
        let diags = errors(r#"{"a": 1, "a": 2}"#);
        let dups: Vec<_> = diags
            .iter()
            .filter(|d| d.message.contains("Duplicate key"))
            .collect();
        assert!(
            dups.iter()
                .all(|d| d.severity == Some(DiagnosticSeverity::WARNING))
        );
    }
}

/// Detect duplicate keys within the same object.
pub fn collect_duplicate_keys(doc: &Document, node: Node<'_>, diags: &mut Vec<Diagnostic>) {
    if node.kind() == kinds::OBJECT {
        let mut seen = HashSet::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if child.kind() != kinds::PAIR {
                continue;
            }
            if let Some(key_node) = child.child_by_field_name("key")
                && let Some(key) = tree::string_content(key_node, doc.source())
                && !seen.insert(key.to_string())
            {
                let range = doc.range_of(key_node.start_byte(), key_node.end_byte());
                diags.push(Diagnostic {
                    range,
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some("json".into()),
                    message: format!("Duplicate key \"{key}\""),
                    ..Diagnostic::default()
                });
            }
        }
    }

    // Recurse into all children that could contain objects.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            kinds::OBJECT | kinds::ARRAY | kinds::PAIR => {
                collect_duplicate_keys(doc, child, diags);
            }
            _ => {}
        }
    }
}
