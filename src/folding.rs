/// Folding range provider: every object, array, and comment block is foldable.
use lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::kinds;

/// Produce folding ranges for all objects, arrays, and comment blocks.
pub fn folding_ranges(doc: &Document) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();
    collect_folds(doc, doc.tree.root_node(), &mut ranges);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn no_folds_single_line() {
        let doc = Document::new(r#"{"a": 1}"#.into(), 0);
        let folds = folding_ranges(&doc);
        assert!(folds.is_empty());
    }

    #[test]
    fn fold_multiline_object() {
        let doc = Document::new("{\n  \"a\": 1\n}".into(), 0);
        let folds = folding_ranges(&doc);
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].start_line, 0);
        assert_eq!(folds[0].end_line, 2);
        assert_eq!(folds[0].kind, Some(FoldingRangeKind::Region));
    }

    #[test]
    fn fold_multiline_array() {
        let doc = Document::new("[\n  1,\n  2\n]".into(), 0);
        let folds = folding_ranges(&doc);
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].kind, Some(FoldingRangeKind::Region));
    }

    #[test]
    fn fold_nested() {
        let doc = Document::new("{\n  \"a\": {\n    \"b\": 1\n  }\n}".into(), 0);
        let folds = folding_ranges(&doc);
        assert_eq!(folds.len(), 2); // Outer object + inner object.
    }

    #[test]
    fn empty_document_no_folds() {
        let doc = Document::new("".into(), 0);
        let folds = folding_ranges(&doc);
        assert!(folds.is_empty());
    }
}

fn collect_folds(doc: &Document, node: Node<'_>, ranges: &mut Vec<FoldingRange>) {
    match node.kind() {
        kinds::OBJECT | kinds::ARRAY => {
            let start = doc.position_of(node.start_byte());
            let end = doc.position_of(node.end_byte());

            // Only fold if it spans multiple lines.
            if end.line > start.line {
                ranges.push(FoldingRange {
                    start_line: start.line,
                    start_character: Some(start.character),
                    end_line: end.line,
                    end_character: Some(end.character),
                    kind: Some(FoldingRangeKind::Region),
                    collapsed_text: None,
                });
            }
        }
        kinds::COMMENT => {
            let start = doc.position_of(node.start_byte());
            let end = doc.position_of(node.end_byte());

            if end.line > start.line {
                ranges.push(FoldingRange {
                    start_line: start.line,
                    start_character: Some(start.character),
                    end_line: end.line,
                    end_character: Some(end.character),
                    kind: Some(FoldingRangeKind::Comment),
                    collapsed_text: None,
                });
            }
            return; // Comments don't have children.
        }
        _ => {}
    }

    // Recurse.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_folds(doc, child, ranges);
    }
}
