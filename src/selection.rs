/// Selection range provider: walk up the tree-sitter node hierarchy.
/// Each parent becomes a wider selection range.
use tower_lsp::lsp_types::*;

use crate::document::Document;
use crate::tree;

/// Produce selection ranges for the given positions.
pub fn selection_ranges(doc: &Document, positions: &[Position]) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|pos| {
            let offset = doc.offset_of(*pos);
            build_selection_range(doc, offset)
        })
        .collect()
}

fn build_selection_range(doc: &Document, offset: usize) -> SelectionRange {
    let node = match tree::node_at_offset(&doc.tree, offset) {
        Some(n) => n,
        None => {
            return SelectionRange {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end: doc.position_of(doc.text.len()),
                },
                parent: None,
            };
        }
    };

    build_chain(doc, node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn selection_range_single_position() {
        let doc = Document::new(r#"{"a": "hello"}"#.into(), 0);
        let pos = Position {
            line: 0,
            character: 8,
        }; // Inside "hello"
        let ranges = selection_ranges(&doc, &[pos]);
        assert_eq!(ranges.len(), 1);
        // Should have a chain: string -> pair -> object -> document
        let r = &ranges[0];
        assert!(r.parent.is_some());
    }

    #[test]
    fn selection_range_nested() {
        let doc = Document::new(r#"{"a": {"b": 42}}"#.into(), 0);
        let pos = Position {
            line: 0,
            character: 12,
        }; // At "42"
        let ranges = selection_ranges(&doc, &[pos]);
        assert_eq!(ranges.len(), 1);
        // Count chain depth.
        let mut depth = 1;
        let mut current = &ranges[0];
        while let Some(ref parent) = current.parent {
            depth += 1;
            current = parent;
        }
        assert!(depth >= 3); // number -> pair -> object -> pair -> object -> document
    }

    #[test]
    fn selection_range_multiple_positions() {
        let doc = Document::new(r#"{"a": 1, "b": 2}"#.into(), 0);
        let positions = vec![
            Position {
                line: 0,
                character: 6,
            }, // At 1
            Position {
                line: 0,
                character: 14,
            }, // At 2
        ];
        let ranges = selection_ranges(&doc, &positions);
        assert_eq!(ranges.len(), 2);
    }

    #[test]
    fn selection_range_empty_document() {
        let doc = Document::new("".into(), 0);
        let pos = Position {
            line: 0,
            character: 0,
        };
        let ranges = selection_ranges(&doc, &[pos]);
        assert_eq!(ranges.len(), 1);
    }
}

fn build_chain(doc: &Document, node: tree_sitter::Node<'_>) -> SelectionRange {
    let range = doc.range_of(node.start_byte(), node.end_byte());

    let parent = node.parent().map(|p| Box::new(build_chain(doc, p)));

    SelectionRange { range, parent }
}
