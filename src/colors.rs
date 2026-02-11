/// Document colors: detects CSS hex color values in JSON strings
/// and provides color presentation conversions.
use lsp_types::*;
use tree_sitter::Node;

use crate::document::Document;
use crate::tree::{self, kinds};

/// Find all hex color values in the document.
pub fn document_colors(doc: &Document) -> Vec<ColorInformation> {
    let mut colors = Vec::new();
    collect_colors(doc, doc.tree.root_node(), &mut colors);
    colors
}

/// Generate color presentation alternatives for a given color.
pub fn color_presentations(color: Color) -> Vec<ColorPresentation> {
    let r = (color.red * 255.0) as u8;
    let g = (color.green * 255.0) as u8;
    let b = (color.blue * 255.0) as u8;
    let a = color.alpha;

    let mut presentations = Vec::new();

    if (a - 1.0).abs() < f32::EPSILON {
        presentations.push(make_presentation(format!("#{r:02x}{g:02x}{b:02x}")));
        presentations.push(make_presentation(format!("rgb({r}, {g}, {b})")));
    } else {
        let a8 = (a * 255.0) as u8;
        presentations.push(make_presentation(format!("#{r:02x}{g:02x}{b:02x}{a8:02x}")));
        presentations.push(make_presentation(format!("rgba({r}, {g}, {b}, {a:.2})")));
    }

    let (h, s, l) = rgb_to_hsl(color.red, color.green, color.blue);
    if (a - 1.0).abs() < f32::EPSILON {
        presentations.push(make_presentation(format!("hsl({h:.0}, {s:.0}%, {l:.0}%)")));
    } else {
        presentations.push(make_presentation(format!(
            "hsla({h:.0}, {s:.0}%, {l:.0}%, {a:.2})"
        )));
    }

    presentations
}

fn make_presentation(label: String) -> ColorPresentation {
    ColorPresentation {
        label,
        text_edit: None,
        additional_text_edits: None,
    }
}

fn collect_colors(doc: &Document, node: Node<'_>, colors: &mut Vec<ColorInformation>) {
    if node.kind() == kinds::STRING {
        if let Some(raw) = tree::string_content(node, doc.source())
            && let Some(color) = parse_hex_color(raw)
        {
            let range = doc.range_of(node.start_byte(), node.end_byte());
            colors.push(ColorInformation { range, color });
        }
        return; // No children to recurse into.
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_colors(doc, child, colors);
    }
}

fn parse_hex_color(s: &str) -> Option<Color> {
    if !s.starts_with('#') {
        return None;
    }
    let hex = &s[1..];
    let bytes = hex.as_bytes();

    match hex.len() {
        3 => {
            let r = hex_digit(bytes[0])? * 17;
            let g = hex_digit(bytes[1])? * 17;
            let b = hex_digit(bytes[2])? * 17;
            Some(color(r, g, b, 255))
        }
        4 => {
            let r = hex_digit(bytes[0])? * 17;
            let g = hex_digit(bytes[1])? * 17;
            let b = hex_digit(bytes[2])? * 17;
            let a = hex_digit(bytes[3])? * 17;
            Some(color(r, g, b, a))
        }
        6 => {
            let r = hex_byte(bytes[0], bytes[1])?;
            let g = hex_byte(bytes[2], bytes[3])?;
            let b = hex_byte(bytes[4], bytes[5])?;
            Some(color(r, g, b, 255))
        }
        8 => {
            let r = hex_byte(bytes[0], bytes[1])?;
            let g = hex_byte(bytes[2], bytes[3])?;
            let b = hex_byte(bytes[4], bytes[5])?;
            let a = hex_byte(bytes[6], bytes[7])?;
            Some(color(r, g, b, a))
        }
        _ => None,
    }
}

fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color {
        red: r as f32 / 255.0,
        green: g as f32 / 255.0,
        blue: b as f32 / 255.0,
        alpha: a as f32 / 255.0,
    }
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some(hex_digit(hi)? * 16 + hex_digit(lo)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    #[test]
    fn detect_hex_6_color() {
        let doc = Document::new(r##"{"color": "#ff0000"}"##.into(), 0);
        let colors = document_colors(&doc);
        assert_eq!(colors.len(), 1);
        let c = &colors[0].color;
        assert!((c.red - 1.0).abs() < 0.01);
        assert!(c.green.abs() < 0.01);
        assert!(c.blue.abs() < 0.01);
        assert!((c.alpha - 1.0).abs() < 0.01);
    }

    #[test]
    fn detect_hex_3_color() {
        let doc = Document::new(r##"{"color": "#f00"}"##.into(), 0);
        let colors = document_colors(&doc);
        assert_eq!(colors.len(), 1);
        let c = &colors[0].color;
        assert!((c.red - 1.0).abs() < 0.01);
    }

    #[test]
    fn detect_hex_8_color_with_alpha() {
        let doc = Document::new(r##"{"color": "#ff000080"}"##.into(), 0);
        let colors = document_colors(&doc);
        assert_eq!(colors.len(), 1);
        let c = &colors[0].color;
        assert!((c.alpha - 128.0 / 255.0).abs() < 0.01);
    }

    #[test]
    fn detect_hex_4_color_with_alpha() {
        let doc = Document::new(r##"{"color": "#f008"}"##.into(), 0);
        let colors = document_colors(&doc);
        assert_eq!(colors.len(), 1);
    }

    #[test]
    fn no_color_in_non_hex_string() {
        let doc = Document::new(r#"{"name": "hello"}"#.into(), 0);
        let colors = document_colors(&doc);
        assert!(colors.is_empty());
    }

    #[test]
    fn no_color_for_invalid_hex() {
        let doc = Document::new(r##"{"color": "#xyz"}"##.into(), 0);
        let colors = document_colors(&doc);
        assert!(colors.is_empty());
    }

    #[test]
    fn multiple_colors() {
        let doc = Document::new(
            r##"{"bg": "#ffffff", "fg": "#000000", "accent": "#abcdef"}"##.into(),
            0,
        );
        let colors = document_colors(&doc);
        assert_eq!(colors.len(), 3);
    }

    #[test]
    fn color_presentations_opaque() {
        let presentations = color_presentations(Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        });
        assert!(presentations.len() >= 2);
        assert!(presentations.iter().any(|p| p.label.contains("ff0000")));
        assert!(presentations.iter().any(|p| p.label.contains("rgb")));
        assert!(presentations.iter().any(|p| p.label.contains("hsl")));
    }

    #[test]
    fn color_presentations_with_alpha() {
        let presentations = color_presentations(Color {
            red: 1.0,
            green: 0.0,
            blue: 0.0,
            alpha: 0.5,
        });
        assert!(presentations.iter().any(|p| p.label.contains("rgba")));
        assert!(presentations.iter().any(|p| p.label.contains("hsla")));
    }

    #[test]
    fn parse_hex_color_edge_cases() {
        // Too short.
        assert!(parse_hex_color("#ab").is_none());
        // Too long.
        assert!(parse_hex_color("#abcdefghi").is_none());
        // No hash.
        assert!(parse_hex_color("ff0000").is_none());
        // Valid lengths.
        assert!(parse_hex_color("#fff").is_some());
        assert!(parse_hex_color("#ffff").is_some());
        assert!(parse_hex_color("#ffffff").is_some());
        assert!(parse_hex_color("#ffffffff").is_some());
    }

    #[test]
    fn rgb_to_hsl_pure_red() {
        let (h, s, l) = rgb_to_hsl(1.0, 0.0, 0.0);
        assert!((h - 0.0).abs() < 1.0);
        assert!((s - 100.0).abs() < 1.0);
        assert!((l - 50.0).abs() < 1.0);
    }

    #[test]
    fn rgb_to_hsl_white() {
        let (_, s, l) = rgb_to_hsl(1.0, 1.0, 1.0);
        assert!(s.abs() < 1.0);
        assert!((l - 100.0).abs() < 1.0);
    }

    #[test]
    fn rgb_to_hsl_black() {
        let (_, s, l) = rgb_to_hsl(0.0, 0.0, 0.0);
        assert!(s.abs() < 1.0);
        assert!(l.abs() < 1.0);
    }
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;

    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l * 100.0);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };

    let h = if (max - r).abs() < f32::EPSILON {
        let mut h = (g - b) / d;
        if g < b {
            h += 6.0;
        }
        h
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };

    (h * 60.0, s * 100.0, l * 100.0)
}
