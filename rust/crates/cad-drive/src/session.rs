//! The high-level entry point: sign in once, then fetch Drive files into the local cache.

use std::cell::RefCell;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::DriveConfig;
use crate::drive::{self, FileMetadata};
use crate::error::DriveError;
use crate::http::Http;
use crate::link::DriveLink;
use crate::oauth::{self, Browser, CancelToken, Tokens};
use crate::secret::Secret;
use crate::store::{KeyringStore, TokenStore};

/// What the session is doing, for a status line. `Display` gives the Czech text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Waiting for the user in the browser.
    SigningIn,
    ReadingMetadata,
    /// An up-to-date copy is already in the cache.
    UsingCachedCopy,
    Downloading { done: u64, total: u64 },
    Finished,
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Progress::SigningIn => f.write_str("Přihlášení ke Google Drive: dokončete ho v okně prohlížeče…"),
            Progress::ReadingMetadata => f.write_str("Zjišťuji informace o souboru na Google Drive…"),
            Progress::UsingCachedCopy => f.write_str("Používám již stažený soubor."),
            Progress::Downloading { done, total } => write!(
                f,
                "Stahuji soubor z Google Drive: {} z {}",
                crate::error::format_size(*done),
                crate::error::format_size(*total)
            ),
            Progress::Finished => f.write_str("Soubor je připravený."),
        }
    }
}

/// Parameters of one download.
#[derive(Debug, Clone)]
pub struct FetchRequest<'a> {
    /// Local folder for downloaded copies (created if missing). Choose a per-user folder,
    /// never the synchronised application folder.
    pub cache_dir: &'a Path,
    /// Largest file accepted, in bytes.
    pub max_size: u64,
    /// Lets another thread stop the sign-in or the download.
    pub cancel: CancelToken,
}

/// A file made available locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveFile {
    /// Path of the local copy. Its name comes from the file id and the modified time.
    pub local_path: PathBuf,
    /// The name the file has on the Drive, for display only.
    pub display_name: String,
    pub file_id: String,
    pub size: u64,
    /// Drive's modified time (RFC 3339, UTC).
    pub modified_time: String,
    /// True if no download was needed.
    pub from_cache: bool,
}

struct CachedAccess {
    token: Secret,
    valid_until: Instant,
}

struct Ui<'a> {
    browser: &'a dyn Browser,
    cancel: &'a CancelToken,
    progress: RefCell<&'a mut dyn FnMut(Progress)>,
}

impl Ui<'_> {
    fn report(&self, p: Progress) {
        (self.progress.borrow_mut())(p);
    }
}

pub struct DriveSession {
    config: DriveConfig,
    http: Http,
    store: Box<dyn TokenStore>,
    access: Mutex<Option<CachedAccess>>,
}

impl DriveSession {
    /// Session whose refresh token lives in the operating system credential store.
    pub fn new(config: DriveConfig) -> Result<Self, DriveError> {
        let store = KeyringStore::native(&config)?;
        Self::with_store(config, Box::new(store))
    }

    /// Session with another token store (for example [`crate::MemoryStore`]).
    pub fn with_store(config: DriveConfig, store: Box<dyn TokenStore>) -> Result<Self, DriveError> {
        config.endpoints.validate()?;
        let http = Http::new(config.endpoints.all_loopback());
        Ok(DriveSession { config, http, store, access: Mutex::new(None) })
    }

    pub fn config(&self) -> &DriveConfig {
        &self.config
    }

    /// True if a refresh token is stored, so no browser is needed for the next fetch.
    pub fn has_stored_sign_in(&self) -> bool {
        matches!(self.store.load_refresh_token(), Ok(Some(_)))
    }

    /// Interactive sign-in in the system browser. Stores the refresh token.
    pub fn sign_in(&self, browser: &dyn Browser, cancel: &CancelToken) -> Result<(), DriveError> {
        let tokens = oauth::sign_in(&self.config, &self.http, browser, cancel)?;
        if let Some(rt) = &tokens.refresh {
            self.store.save_refresh_token(rt)?;
        }
        self.remember(&tokens);
        Ok(())
    }

    /// Forgets the stored sign-in on this computer. Access that Google has granted can be
    /// withdrawn by the user in the Google account settings.
    pub fn sign_out(&self) -> Result<(), DriveError> {
        self.invalidate_access();
        self.store.clear()
    }

    /// Reads the metadata of the file (signing in first if necessary).
    pub fn metadata(
        &self,
        link: &DriveLink,
        browser: &dyn Browser,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<FileMetadata, DriveError> {
        let ui = Ui { browser, cancel, progress: RefCell::new(progress) };
        self.read_metadata(&ui, link)
    }

    /// Makes the file available in `request.cache_dir` and returns where it is.
    ///
    /// Signs in if needed, reads the metadata, checks the extension and the size limit, reuses
    /// an up-to-date copy or downloads the file atomically.
    pub fn fetch(
        &self,
        link: &DriveLink,
        request: &FetchRequest<'_>,
        browser: &dyn Browser,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<DriveFile, DriveError> {
        let ui = Ui { browser, cancel: &request.cancel, progress: RefCell::new(progress) };
        let meta = self.read_metadata(&ui, link)?;
        let (ext, size) = drive::check_fetchable(&meta, request.max_size)?;
        let file_name = drive::cache_file_name(&meta.id, &meta.modified_time, ext)?;
        let local_path = request.cache_dir.join(&file_name);

        let result = |local_path: PathBuf, from_cache: bool| DriveFile {
            local_path,
            display_name: meta.name.clone(),
            file_id: meta.id.clone(),
            size,
            modified_time: meta.modified_time.clone(),
            from_cache,
        };

        if drive::is_up_to_date(&local_path, size) {
            ui.report(Progress::UsingCachedCopy);
            return Ok(result(local_path, true));
        }

        let api_base = &self.config.endpoints.api_base;
        let path = self.authorized(&ui, |token| {
            drive::download(
                &self.http,
                api_base,
                token,
                &meta,
                size,
                request.cache_dir,
                &file_name,
                &request.cancel,
                &mut |done, total| ui.report(Progress::Downloading { done, total }),
            )
        })?;
        ui.report(Progress::Finished);
        Ok(result(path, false))
    }

    fn read_metadata(&self, ui: &Ui<'_>, link: &DriveLink) -> Result<FileMetadata, DriveError> {
        ui.report(Progress::ReadingMetadata);
        let api_base = &self.config.endpoints.api_base;
        self.authorized(ui, |token| drive::get_metadata(&self.http, api_base, token, link.file_id()))
    }

    /// Runs `op` with a valid access token. If Drive answers 401 the token is renewed once
    /// and the call repeated.
    fn authorized<T>(
        &self,
        ui: &Ui<'_>,
        mut op: impl FnMut(&Secret) -> Result<T, DriveError>,
    ) -> Result<T, DriveError> {
        let token = self.access_token(ui, false)?;
        match op(&token) {
            Err(DriveError::Unauthorized) => {
                self.invalidate_access();
                let token = self.access_token(ui, true)?;
                op(&token)
            }
            other => other,
        }
    }

    fn access_token(&self, ui: &Ui<'_>, renew: bool) -> Result<Secret, DriveError> {
        if !renew && let Some(token) = self.cached_access() {
            return Ok(token);
        }
        // A credential store that cannot be read counts as "not signed in" so the file can
        // still be opened (the sign-in then just is not remembered).
        if let Ok(Some(refresh_token)) = self.store.load_refresh_token() {
            match oauth::refresh(&self.config, &self.http, &refresh_token) {
                Ok(tokens) => {
                    if let Some(rotated) = &tokens.refresh {
                        let _ = self.store.save_refresh_token(rotated);
                    }
                    self.remember(&tokens);
                    return Ok(tokens.access);
                }
                Err(DriveError::SignInRequired) => {
                    let _ = self.store.clear();
                }
                Err(other) => return Err(other),
            }
        }
        ui.report(Progress::SigningIn);
        let tokens = oauth::sign_in(&self.config, &self.http, ui.browser, ui.cancel)?;
        if let Some(rt) = &tokens.refresh {
            let _ = self.store.save_refresh_token(rt);
        }
        self.remember(&tokens);
        Ok(tokens.access)
    }

    fn remember(&self, tokens: &Tokens) {
        let margin = Duration::from_secs(60);
        let valid_until = Instant::now() + tokens.expires_in.saturating_sub(margin);
        *self.access_lock() = Some(CachedAccess { token: tokens.access.clone(), valid_until });
    }

    fn cached_access(&self) -> Option<Secret> {
        match &*self.access_lock() {
            Some(c) if c.valid_until > Instant::now() => Some(c.token.clone()),
            _ => None,
        }
    }

    fn invalidate_access(&self) {
        *self.access_lock() = None;
    }

    fn access_lock(&self) -> std::sync::MutexGuard<'_, Option<CachedAccess>> {
        self.access.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_texts_are_czech() {
        assert_eq!(Progress::UsingCachedCopy.to_string(), "Používám již stažený soubor.");
        assert_eq!(
            Progress::Downloading { done: 1024 * 1024, total: 2 * 1024 * 1024 }.to_string(),
            "Stahuji soubor z Google Drive: 1,0 MB z 2,0 MB"
        );
    }
}
