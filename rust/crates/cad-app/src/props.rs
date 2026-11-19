//! Content of the properties panel ("QTextBrowser" of the Python window): rows of label and value, where a value is a
//! few styled spans, some of them links that start an action (`mat:<idx>`, `mat:all`, `calc:<idx>`).
use cad_types::NodeId;

/// Target of a link in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// `mat:<idx>`: material dialog for this node.
    Material(NodeId),
    /// `mat:all`: material dialog for the default material.
    MaterialAll,
    /// `calc:<idx>`: compute the mass of this assembly.
    Calc(NodeId),
}

/// A piece of a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    /// Grey hint text (`#8a919c`).
    Muted(String),
    /// Orange warning (`#d08000`).
    Warn(String),
    Link(String, LinkTarget),
    /// Colour swatch followed by its hex name.
    Swatch([u8; 3], String),
}

/// One line of the panel. `label` is empty for continuation lines; the colon is added when there is a value.
#[derive(Clone, Debug, PartialEq)]
pub struct PropRow {
    pub label: String,
    pub bold_label: bool,
    pub value: Vec<Span>,
}

impl PropRow {
    pub fn text(label: impl Into<String>, value: impl Into<String>) -> Self {
        PropRow { label: label.into(), bold_label: false, value: vec![Span::Text(value.into())] }
    }

    pub fn bold(label: impl Into<String>, value: impl Into<String>) -> Self {
        PropRow { label: label.into(), bold_label: false, value: vec![Span::Bold(value.into())] }
    }

    pub fn spans(label: impl Into<String>, value: Vec<Span>) -> Self {
        PropRow { label: label.into(), bold_label: false, value }
    }

    /// A bold heading line without a value (`("<b>Měření</b>", "")`).
    pub fn heading(label: impl Into<String>) -> Self {
        PropRow { label: label.into(), bold_label: true, value: Vec::new() }
    }

    /// A paragraph without a label.
    pub fn note(text: impl Into<String>) -> Self {
        PropRow { label: String::new(), bold_label: false, value: vec![Span::Text(text.into())] }
    }

    /// The plain text of the value.
    #[cfg(test)]
    pub fn value_text(&self) -> String {
        self.value
            .iter()
            .map(|s| match s {
                Span::Text(t) | Span::Bold(t) | Span::Muted(t) | Span::Warn(t) | Span::Link(t, _) => t.clone(),
                Span::Swatch(_, name) => name.clone(),
            })
            .collect()
    }
}

/// What the panel shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Props {
    /// The start screen text.
    Welcome,
    Rows { title: Option<String>, rows: Vec<PropRow> },
}

impl Props {
    pub fn rows(title: Option<String>, rows: Vec<PropRow>) -> Self {
        Props::Rows { title, rows }
    }
}

/// `#rrggbb` as Qt's `QColor.name()`.
pub fn hex_name(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_name_is_lowercase_qt_style() {
        assert_eq!(hex_name([200, 204, 210]), "#c8ccd2");
        assert_eq!(hex_name([0, 0, 0]), "#000000");
    }

    #[test]
    fn value_text_joins_spans() {
        let r = PropRow::spans(
            "Materiál",
            vec![Span::Text("Ocel (".into()), Span::Link("přiřadit…".into(), LinkTarget::Material(3)), Span::Text(")".into())],
        );
        assert_eq!(r.value_text(), "Ocel (přiřadit…)");
        assert_eq!(PropRow::heading("Měření").value_text(), "");
    }
}
