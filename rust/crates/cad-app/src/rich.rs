//! Small helpers for text with `**bold**` parts. The default egui fonts have no bold face (and no extra font is
//! shipped: the size budget), so bold text is drawn in the strong text colour.
use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat, Ui};

/// Splits `text` at the `**` markers. The pieces alternate between normal and bold, starting with normal.
pub fn split_bold(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for (i, piece) in text.split("**").enumerate() {
        if !piece.is_empty() {
            out.push((piece.to_string(), i % 2 == 1));
        }
    }
    out
}

/// Layout job of a text with `**bold**` parts.
pub fn job(ui: &Ui, text: &str, size: f32, wrap_width: f32) -> LayoutJob {
    let normal = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    for (piece, bold) in split_bold(text) {
        job.append(
            &piece,
            0.0,
            TextFormat { font_id: FontId::proportional(size), color: if bold { strong } else { normal }, ..Default::default() },
        );
    }
    job
}

/// A wrapped paragraph that fills the available width.
pub fn paragraph(ui: &mut Ui, text: &str) {
    let width = ui.available_width();
    let size = ui.text_style_height(&egui::TextStyle::Body);
    let j = job(ui, text, size, width);
    ui.add(egui::Label::new(j).wrap());
}

/// Heading line (the `<h3>` of the Python panel).
pub fn heading(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(16.0).strong()).wrap());
}

/// The grey of hints (`#8a919c`).
pub const MUTED: Color32 = Color32::from_rgb(0x8a, 0x91, 0x9c);
/// The orange of warnings (`#d08000`).
pub const WARN: Color32 = Color32::from_rgb(0xd0, 0x80, 0x00);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_markers_alternate() {
        assert_eq!(split_bold("a **b** c"), vec![("a ".into(), false), ("b".into(), true), (" c".into(), false)]);
        assert_eq!(split_bold("**x**"), vec![("x".into(), true)]);
        assert_eq!(split_bold("plain"), vec![("plain".into(), false)]);
        assert_eq!(split_bold(""), Vec::<(String, bool)>::new());
        // an unclosed marker keeps the rest bold
        assert_eq!(split_bold("a **b"), vec![("a ".into(), false), ("b".into(), true)]);
    }
}
