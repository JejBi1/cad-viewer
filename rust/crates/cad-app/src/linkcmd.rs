//! `--register-link` and `--unregister-link`: add or remove the `cadviewer` scheme for the current user, on explicit
//! user action (also in portable mode). `--open-link <url>` is the registered command and runs in the window.
use std::path::Path;

use crate::strings::APP_NAME;

pub const REGISTERED: &str = "Odkazy cadviewer:// (otevření souboru z Google Drive) jsou zaregistrované pro tohoto uživatele.";
pub const UNREGISTERED: &str = "Odkazy cadviewer:// byly pro tohoto uživatele odregistrované.";

/// Message box of a process that has no window; the text is plain.
pub fn message(text: &str, error: bool) {
    let level = if error { rfd::MessageLevel::Error } else { rfd::MessageLevel::Info };
    rfd::MessageDialog::new().set_title(APP_NAME).set_description(text).set_level(level).show();
}

/// Exit code and message of a registration result.
pub fn outcome(result: Result<(), cad_drive::DriveError>, ok_text: &str) -> (i32, String) {
    match result {
        Ok(()) => (0, ok_text.to_string()),
        Err(e) => (1, e.to_string()),
    }
}

pub fn register(exe: &Path) -> i32 {
    let (code, text) = outcome(cad_drive::register_link(exe), REGISTERED);
    message(&text, code != 0);
    code
}

pub fn unregister() -> i32 {
    let (code, text) = outcome(cad_drive::unregister_link(), UNREGISTERED);
    message(&text, code != 0);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_carries_the_czech_error() {
        assert_eq!(outcome(Ok(()), "hotovo"), (0, "hotovo".to_string()));
        let (code, text) = outcome(Err(cad_drive::DriveError::Unsupported), "x");
        assert_eq!(code, 1);
        assert!(text.contains("není na tomto operačním systému podporovaná"));
    }
}
