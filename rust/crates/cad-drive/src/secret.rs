//! A string that must never end up in a log or an error message.

use std::fmt;

/// Holds a token or a client secret. `Debug` prints a placeholder, there is no `Display`
/// and no serialisation, so a value cannot leak by accident through `{:?}` or `{}`.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The only way to get at the text. Call it where the value is really sent or stored.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_value() {
        let s = Secret::new("tajne-heslo");
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(s.clone())), "Some(Secret(***))");
        assert_eq!(s.expose(), "tajne-heslo");
    }
}
