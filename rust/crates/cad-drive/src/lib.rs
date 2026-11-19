//! Google Drive launcher of the CAD viewer (native part).
//! See docs/rust-port/ARCHITECTURE.md, section "Google Drive launcher (all Rust)".
//!
//! Typical use from the application:
//!
//! ```no_run
//! use cad_drive::{CancelToken, DriveConfig, DriveSession, FetchRequest, SystemBrowser, parse_link};
//!
//! # fn main() -> Result<(), cad_drive::DriveError> {
//! let link = parse_link("cadviewer://open?fileId=1AbCdEfGhIjKlMnOpQrStUvWxYz")?;
//! let session = DriveSession::new(cad_drive::config_from_build_env!()?)?;
//! let request = FetchRequest {
//!     cache_dir: std::path::Path::new("C:/Users/me/AppData/Local/CadViewer/drive"),
//!     max_size: 500 * 1024 * 1024,
//!     cancel: CancelToken::new(),
//! };
//! let file = session.fetch(&link, &request, &SystemBrowser, &mut |p| println!("{p}"))?;
//! println!("{} -> {}", file.display_name, file.local_path.display());
//! # Ok(()) }
//! ```
//!
//! Security properties, each covered by tests:
//! * the link is untrusted and parsed strictly ([`parse_link`]);
//! * OAuth uses PKCE S256, a random loopback port, a verified `state`, a timeout, and the single
//!   scope `drive.file`;
//! * tokens live only in the OS credential store and in memory, never in files or logs
//!   ([`Secret`] has no `Display` and a redacted `Debug`);
//! * the Drive is only read: `files.get` and `alt=media`, both with GET;
//! * the local file name is derived from the file id and the modified time, never from the
//!   Drive name, and the file appears in the cache atomically.

mod config;
mod drive;
mod error;
mod http;
mod link;
mod oauth;
mod pkce;
mod register;
mod secret;
mod session;
mod store;
mod urlutil;

pub use config::{
    DriveConfig, Endpoints, ENV_CLIENT_ID, ENV_CLIENT_SECRET, ENV_LAUNCHER_URL, KEYRING_SERVICE, LAUNCHER_URL,
    SCOPE,
};
pub use drive::{
    FileMetadata, SUPPORTED_EXTENSIONS, cache_file_name, sanitize_display_name, supported_extension,
};
pub use error::{DriveError, LinkError, format_size};
pub use link::{
    DriveLink, FILE_ID_MAX_LEN, FILE_ID_MIN_LEN, LINK_COMMAND, LINK_PARAM, LINK_SCHEME, MAX_LINK_LEN,
    is_valid_file_id, parse_link,
};
pub use oauth::{Browser, CALLBACK_PATH, CancelToken, SystemBrowser};
pub use pkce::challenge_for;
pub use register::{
    DESKTOP_FILE_NAME, OPEN_LINK_ARG, REGISTRY_ROOT, RegistryBackend, XdgDirs, desktop_entry, desktop_exec_quote,
    is_under_root, mimeapps_remove_default, mimeapps_set_default, register_link, register_link_with,
    register_link_xdg, unregister_link, unregister_link_with, unregister_link_xdg, windows_command,
};
pub use secret::Secret;
pub use session::{DriveFile, DriveSession, FetchRequest, Progress};
pub use store::{KeyringStore, MemoryStore, TokenStore};

#[cfg(test)]
mod source_rules {
    //! Guards that keep the "read only" promise visible in the code.

    const DRIVE_SRC: &str = include_str!("drive.rs");
    const SESSION_SRC: &str = include_str!("session.rs");

    /// Code before the test module, without comment lines.
    fn production_part(src: &str) -> String {
        src.split("#[cfg(test)]")
            .next()
            .unwrap_or(src)
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn drive_api_code_has_no_write_calls() {
        for (name, src) in [("drive.rs", DRIVE_SRC), ("session.rs", SESSION_SRC)] {
            let code = production_part(src);
            for forbidden in [
                "post_form", ".post(", ".put(", ".patch(", ".delete(", "Method::POST", "Method::PUT",
                "Method::PATCH", "Method::DELETE", "send_form", "\"POST\"", "\"PUT\"", "\"PATCH\"",
                "\"DELETE\"", "/upload/", "uploadType", "trash", "permissions", "files/copy", "/copy",
                "emptyTrash",
            ] {
                assert!(!code.contains(forbidden), "{name} contains `{forbidden}`");
            }
        }
        // Drive requests go through `Http::get` only.
        let drive = production_part(DRIVE_SRC);
        assert!(drive.contains("http.get(Kind::Api"));
        assert!(drive.contains("http.get(Kind::Download"));
    }

    #[test]
    fn only_the_drive_file_scope_is_requested() {
        let all = [DRIVE_SRC, SESSION_SRC, include_str!("oauth.rs"), include_str!("config.rs")];
        for src in all {
            let code = production_part(src);
            for other in [
                "auth/drive\"", "auth/drive.readonly", "auth/drive.metadata", "auth/drive.appdata",
                "auth/userinfo", "auth/cloud-platform",
            ] {
                assert!(!code.contains(other), "unexpected scope text `{other}`");
            }
        }
        assert_eq!(super::SCOPE, "https://www.googleapis.com/auth/drive.file");
    }
}
