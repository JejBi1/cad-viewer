//! Build-time and run-time configuration. No client id and no secret is committed anywhere:
//! the caller passes a [`DriveConfig`], typically made with [`config_from_build_env!`].

use std::time::Duration;

use crate::error::DriveError;
use crate::secret::Secret;

/// The one and only OAuth scope. It gives access to files the user opened with this app
/// (through "Open with" on Drive) and nothing else.
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";

/// Build-time environment variable names.
pub const ENV_CLIENT_ID: &str = "CADVIEWER_DRIVE_CLIENT_ID";
pub const ENV_CLIENT_SECRET: &str = "CADVIEWER_DRIVE_CLIENT_SECRET";
pub const ENV_LAUNCHER_URL: &str = "CADVIEWER_LAUNCHER_URL";

/// URL of the static launcher page (cad-drive-web), baked in at build time if provided.
pub const LAUNCHER_URL: Option<&str> = option_env!("CADVIEWER_LAUNCHER_URL");

/// Service name under which the refresh token is stored in the OS credential store.
pub const KEYRING_SERVICE: &str = "cadviewer-drive";

/// Endpoints. The defaults are Google's. Tests replace them with a local mock server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// Authorization endpoint opened in the system browser.
    pub auth_url: String,
    /// Token endpoint (code exchange and refresh).
    pub token_url: String,
    /// Base of the Drive API v3, without a trailing slash.
    pub api_base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            api_base: "https://www.googleapis.com/drive/v3".into(),
        }
    }
}

impl Endpoints {
    /// Every endpoint must be HTTPS. Plain HTTP is only accepted for a loopback address
    /// (127.0.0.1, ::1, localhost), which is what the tests with a mock server use.
    pub fn validate(&self) -> Result<(), DriveError> {
        for (name, url) in [
            ("auth_url", &self.auth_url),
            ("token_url", &self.token_url),
            ("api_base", &self.api_base),
        ] {
            let parts = crate::urlutil::split_url(url)
                .ok_or_else(|| DriveError::BadConfig(format!("adresa {name} není platná")))?;
            let secure = parts.scheme == "https";
            let loopback_http = parts.scheme == "http" && crate::urlutil::is_loopback_host(&parts.host);
            if !secure && !loopback_http {
                return Err(DriveError::BadConfig(format!(
                    "adresa {name} musí používat HTTPS"
                )));
            }
        }
        if self.api_base.ends_with('/') {
            return Err(DriveError::BadConfig("api_base nesmí končit lomítkem".into()));
        }
        Ok(())
    }

    pub(crate) fn all_loopback(&self) -> bool {
        [&self.auth_url, &self.token_url, &self.api_base].iter().all(|u| {
            crate::urlutil::split_url(u)
                .map(|p| crate::urlutil::is_loopback_host(&p.host))
                .unwrap_or(false)
        })
    }
}

/// Everything the Drive code needs to know from the application.
#[derive(Debug, Clone)]
pub struct DriveConfig {
    pub(crate) client_id: String,
    pub(crate) client_secret: Option<Secret>,
    pub(crate) endpoints: Endpoints,
    pub(crate) sign_in_timeout: Duration,
}

impl DriveConfig {
    /// Default time to wait for the user to finish signing in in the browser.
    pub const DEFAULT_SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

    /// `client_id` is the OAuth client of type "Desktop app" created by the distributor.
    pub fn new(client_id: &str) -> Result<Self, DriveError> {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            return Err(DriveError::NotConfigured);
        }
        if client_id.len() > 256 || !client_id.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
            return Err(DriveError::BadConfig("identifikátor klienta OAuth není platný".into()));
        }
        Ok(DriveConfig {
            client_id: client_id.to_owned(),
            client_secret: None,
            endpoints: Endpoints::default(),
            sign_in_timeout: Self::DEFAULT_SIGN_IN_TIMEOUT,
        })
    }

    /// Builds the config from values that may be missing (for example `option_env!` results).
    /// A missing or empty client id gives [`DriveError::NotConfigured`].
    pub fn from_optional(client_id: Option<&str>, client_secret: Option<&str>) -> Result<Self, DriveError> {
        let mut cfg = Self::new(client_id.ok_or(DriveError::NotConfigured)?)?;
        if let Some(secret) = client_secret.map(str::trim).filter(|s| !s.is_empty()) {
            cfg.client_secret = Some(Secret::new(secret));
        }
        Ok(cfg)
    }

    /// Google's "Desktop app" clients come with a client secret that Google itself documents as
    /// not confidential. Set it only if the token endpoint asks for it.
    pub fn with_client_secret(mut self, secret: &str) -> Self {
        self.client_secret = Some(Secret::new(secret));
        self
    }

    pub fn with_endpoints(mut self, endpoints: Endpoints) -> Result<Self, DriveError> {
        endpoints.validate()?;
        self.endpoints = endpoints;
        Ok(self)
    }

    pub fn with_sign_in_timeout(mut self, timeout: Duration) -> Self {
        self.sign_in_timeout = timeout;
        self
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn sign_in_timeout(&self) -> Duration {
        self.sign_in_timeout
    }

    /// Reads the id and secret that were present in the environment when *this crate* was
    /// compiled. Use [`config_from_build_env!`] to read them when the application is compiled.
    pub fn from_build_env() -> Result<Self, DriveError> {
        Self::from_optional(
            option_env!("CADVIEWER_DRIVE_CLIENT_ID"),
            option_env!("CADVIEWER_DRIVE_CLIENT_SECRET"),
        )
    }
}

/// Expands to `Result<DriveConfig, DriveError>` using the build-time variables
/// `CADVIEWER_DRIVE_CLIENT_ID` and `CADVIEWER_DRIVE_CLIENT_SECRET` of the crate where it is used.
#[macro_export]
macro_rules! config_from_build_env {
    () => {
        $crate::DriveConfig::from_optional(
            ::core::option_env!("CADVIEWER_DRIVE_CLIENT_ID"),
            ::core::option_env!("CADVIEWER_DRIVE_CLIENT_SECRET"),
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_client_id_is_not_configured() {
        assert_eq!(DriveConfig::from_optional(None, None).unwrap_err(), DriveError::NotConfigured);
        assert_eq!(DriveConfig::from_optional(Some("  "), None).unwrap_err(), DriveError::NotConfigured);
        assert_eq!(DriveConfig::new("").unwrap_err(), DriveError::NotConfigured);
    }

    #[test]
    fn rejects_odd_client_id() {
        assert!(matches!(DriveConfig::new("a b").unwrap_err(), DriveError::BadConfig(_)));
        assert!(matches!(DriveConfig::new("č").unwrap_err(), DriveError::BadConfig(_)));
        assert!(matches!(DriveConfig::new(&"x".repeat(300)).unwrap_err(), DriveError::BadConfig(_)));
    }

    #[test]
    fn debug_does_not_show_secret() {
        let cfg = DriveConfig::from_optional(Some("id.apps.example"), Some("GOCSPX-tajemstvi")).unwrap();
        let text = format!("{cfg:?}");
        assert!(!text.contains("GOCSPX"), "{text}");
        assert!(text.contains("id.apps.example"));
    }

    #[test]
    fn default_endpoints_are_https_and_valid() {
        let e = Endpoints::default();
        e.validate().unwrap();
        assert!(e.auth_url.starts_with("https://accounts.google.com/"));
        assert!(!e.all_loopback());
    }

    #[test]
    fn endpoints_need_https_except_loopback() {
        let mut e = Endpoints {
            token_url: "http://oauth2.googleapis.com/token".into(),
            ..Endpoints::default()
        };
        assert!(e.validate().is_err());
        e.token_url = "http://127.0.0.1:8080/token".into();
        e.validate().unwrap(); // each address is checked on its own: https remote and http loopback
        let local = Endpoints {
            auth_url: "http://127.0.0.1:1/auth".into(),
            token_url: "http://127.0.0.1:1/token".into(),
            api_base: "http://localhost:1/drive/v3".into(),
        };
        local.validate().unwrap();
        assert!(local.all_loopback());
        let bad = Endpoints { api_base: "ftp://127.0.0.1/x".into(), ..local.clone() };
        assert!(bad.validate().is_err());
        let slash = Endpoints { api_base: "http://127.0.0.1:1/drive/v3/".into(), ..local };
        assert!(slash.validate().is_err());
    }

    #[test]
    fn scope_is_drive_file_only() {
        assert_eq!(SCOPE, "https://www.googleapis.com/auth/drive.file");
    }
}
