//! Opening a Google Drive file (`--open-link <url>`): parse the link, download with `cad-drive` off the window thread
//! with progress and cancel, then open the local copy through the normal loading path.
use std::path::{Path, PathBuf};

use cad_core::paths;
use cad_drive::{CancelToken, DriveError, DriveFile, DriveSession, FetchRequest, Progress, SystemBrowser};

use crate::app::App;
use crate::flow::Event;
use crate::strings;
use crate::util::spawn_big;

/// Largest file taken from the Drive (the cache keeps 3 GiB in total).
pub const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;

/// Folder of downloaded copies: a per-user local folder, never the application folder.
pub fn download_dir() -> PathBuf {
    paths::cache_root().join(cad_core::cache::DRIVE_DIR)
}

pub struct DriveRun {
    pub cancel: CancelToken,
}

pub enum DriveEvent {
    Progress { token: u64, progress: Progress },
    Done { token: u64, result: Result<DriveFile, DriveError> },
}

/// Message for a build without an OAuth client, and for a failed download.
pub fn not_configured_text() -> String {
    strings::DRIVE_NOT_CONFIGURED.to_string()
}

pub fn failure_text(e: &DriveError) -> String {
    match e {
        DriveError::NotConfigured => not_configured_text(),
        other => format!("Soubor z Google Drive se nepodařilo otevřít:\n{other}"),
    }
}

/// The message for a link that was refused (also the command line of a link that has more arguments than the link).
pub fn link_error_text(e: &dyn std::fmt::Display) -> String {
    format!("Odkaz se nepodařilo otevřít:\n{e}")
}

/// What the entries "Zaregistrovat / Odregistrovat odkazy Google Drive" and "Odhlásit se z Google Drive" of the menu
/// call. The system does the real work (the registry of the current user or the XDG files, the credential store of
/// the system); tests put in a fake, so that no key is written and no sign-in is touched.
pub trait LinkHost {
    fn register(&self, exe: &Path) -> Result<(), DriveError>;
    fn unregister(&self) -> Result<(), DriveError>;
    fn sign_out(&self) -> Result<(), DriveError>;
}

/// The real thing. Every call is the result of a click on a menu entry, never of anything else.
pub struct SystemHost;

impl LinkHost for SystemHost {
    fn register(&self, exe: &Path) -> Result<(), DriveError> {
        cad_drive::register_link(exe)
    }

    fn unregister(&self) -> Result<(), DriveError> {
        cad_drive::unregister_link()
    }

    fn sign_out(&self) -> Result<(), DriveError> {
        DriveSession::new(cad_drive::config_from_build_env!()?)?.sign_out()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    Register,
    Unregister,
    SignOut,
}

/// Runs one menu action; `(true, text)` for a message of success, `(false, text)` for the Czech reason of a failure.
pub fn run_link_action(host: &dyn LinkHost, exe: &Path, action: LinkAction) -> (bool, String) {
    let (code, text) = match action {
        LinkAction::Register => crate::linkcmd::outcome(host.register(exe), crate::linkcmd::REGISTERED),
        LinkAction::Unregister => crate::linkcmd::outcome(host.unregister(), crate::linkcmd::UNREGISTERED),
        LinkAction::SignOut => match host.sign_out() {
            Ok(()) => (0, strings::SIGNED_OUT.to_string()),
            // a build without an OAuth client never signed anybody in
            Err(DriveError::NotConfigured) => (0, strings::NOTHING_TO_SIGN_OUT.to_string()),
            Err(e) => (1, e.to_string()),
        },
    };
    (code == 0, text)
}

/// Removes the oldest downloaded files until the folder fits in `limit` bytes. Files are only ever read by the
/// viewer, so deleting a copy only means it is downloaded again.
///
/// The folder is shared with other running instances (there is no single-instance rule). A symbolic link or any other
/// reparse point (a placeholder of a synced folder) is not looked into, counted or removed, as `cache::prune` does not
/// either; a download that is still being written (`.part`, `.tmp`) belongs to an instance that is running now, so it is
/// left alone and does not count towards the limit, and only the leftover of a download that stopped long ago goes.
pub fn prune_downloads(dir: &std::path::Path, limit: u64, keep: &std::path::Path) {
    prune_downloads_at(dir, limit, keep, std::time::SystemTime::now());
}

/// `prune_downloads` with an explicit "now" (for tests).
pub fn prune_downloads_at(dir: &std::path::Path, limit: u64, keep: &std::path::Path, now: std::time::SystemTime) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let stale_before = now.checked_sub(cad_core::cache::MAX_TMP_AGE);
    let mut files: Vec<(std::time::SystemTime, PathBuf, u64)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        // `symlink_metadata`: what the entry itself is, never what it points to
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if !m.is_file() || cad_core::cache::is_link(&m) {
            continue;
        }
        if cad_core::cache::is_temp_leftover(&e.file_name().to_string_lossy()) {
            // an unreadable time is no proof that the download stopped
            let stale = matches!((stale_before, m.modified()), (Some(old), Ok(t)) if t < old);
            if stale && p != keep {
                let _ = std::fs::remove_file(&p);
            }
            continue;
        }
        files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), p, m.len()));
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut total = 0u64;
    for (_, p, size) in files {
        total += size;
        if total > limit && p != keep {
            let _ = std::fs::remove_file(&p);
        }
    }
}

impl App {
    /// A menu entry for the link or the sign-in was clicked.
    pub fn link_action(&mut self, action: LinkAction) {
        let (ok, text) = run_link_action(self.link_host.as_ref(), &self.exe, action);
        if ok {
            self.info(text);
        } else {
            self.critical(text);
        }
    }

    /// `--open-link`: starts the download, or says why it cannot be done.
    pub fn start_drive_link(&mut self, url: &str) {
        let link = match cad_drive::parse_link(url) {
            Ok(l) => l,
            Err(e) => {
                self.warn(link_error_text(&e));
                return;
            }
        };
        let config = match cad_drive::config_from_build_env!() {
            Ok(c) => c,
            Err(e) => {
                self.warn(failure_text(&e));
                return;
            }
        };
        if self.flow.active() {
            self.cancel_load();
        }
        self.flow.token += 1;
        let token = self.flow.token;
        self.set_busy(true, "Otevírám soubor z Google Drive…");
        let cancel = CancelToken::new();
        self.drive = Some(DriveRun { cancel: cancel.clone() });
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.flow.thread_busy = true;
        let spawned = spawn_big("drive", move || {
            let result = (|| -> Result<DriveFile, DriveError> {
                let session = DriveSession::new(config)?;
                let dir = download_dir();
                std::fs::create_dir_all(&dir)?;
                let request = FetchRequest { cache_dir: &dir, max_size: MAX_DOWNLOAD, cancel };
                let t2 = tx.clone();
                let c2 = ctx.clone();
                session.fetch(&link, &request, &SystemBrowser, &mut |p| {
                    let _ = t2.send(Event::Drive(DriveEvent::Progress { token, progress: p }));
                    c2.request_repaint();
                })
            })();
            App::send(&tx, &ctx, Event::Drive(DriveEvent::Done { token, result }));
        });
        if spawned.is_err() {
            self.flow.thread_busy = false;
            self.drive = None;
            self.set_busy(false, "");
            self.critical("Stahování z Google Drive se nepodařilo spustit.");
        }
    }

    pub fn on_drive_event(&mut self, ev: DriveEvent) {
        match ev {
            DriveEvent::Progress { token, progress } => {
                if token == self.flow.token && self.flow.busy {
                    self.status.model = progress.to_string();
                }
            }
            DriveEvent::Done { token, result } => {
                if token != self.flow.token {
                    return;
                }
                self.flow.thread_busy = false;
                self.drive = None;
                match result {
                    Ok(file) => {
                        prune_downloads(&download_dir(), cad_core::cache::CACHE_LIMIT / 2, &file.local_path);
                        self.set_busy(false, "");
                        self.open_file(file.local_path, Some(file.display_name));
                    }
                    Err(DriveError::Cancelled | DriveError::DownloadCancelled) => {
                        self.set_busy(false, "");
                        self.update_status();
                    }
                    Err(e) => {
                        self.set_busy(false, "");
                        self.update_status();
                        self.critical(failure_text(&e));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what the menu asked for; nothing is written anywhere.
    #[derive(Default)]
    struct Fake {
        calls: std::cell::RefCell<Vec<String>>,
        fail_with: Option<DriveError>,
        sign_out_error: Option<DriveError>,
    }

    impl LinkHost for Fake {
        fn register(&self, exe: &Path) -> Result<(), DriveError> {
            self.calls.borrow_mut().push(format!("register {}", exe.display()));
            self.fail_with.clone().map_or(Ok(()), Err)
        }
        fn unregister(&self) -> Result<(), DriveError> {
            self.calls.borrow_mut().push("unregister".into());
            self.fail_with.clone().map_or(Ok(()), Err)
        }
        fn sign_out(&self) -> Result<(), DriveError> {
            self.calls.borrow_mut().push("sign_out".into());
            self.sign_out_error.clone().map_or(Ok(()), Err)
        }
    }

    #[test]
    fn the_menu_entries_call_the_host_only_when_run() {
        let host = Fake::default();
        assert!(host.calls.borrow().is_empty(), "nothing happens by itself");
        let exe = Path::new(r"C:\Programy\CAD\StepViewer.exe");
        let (ok, text) = run_link_action(&host, exe, LinkAction::Register);
        assert!(ok);
        assert_eq!(text, crate::linkcmd::REGISTERED);
        let (ok, text) = run_link_action(&host, exe, LinkAction::Unregister);
        assert!(ok);
        assert_eq!(text, crate::linkcmd::UNREGISTERED);
        let (ok, text) = run_link_action(&host, exe, LinkAction::SignOut);
        assert!(ok);
        assert_eq!(text, strings::SIGNED_OUT);
        assert_eq!(*host.calls.borrow(), vec![format!("register {}", exe.display()), "unregister".to_string(), "sign_out".to_string()]);
    }

    #[test]
    fn a_failed_menu_action_gives_the_czech_reason() {
        let exe = Path::new("StepViewer.exe");
        let host = Fake { fail_with: Some(DriveError::Registration("zápis se nezdařil".into())), ..Default::default() };
        let (ok, text) = run_link_action(&host, exe, LinkAction::Register);
        assert!(!ok);
        assert!(text.starts_with("Registrace odkazů cadviewer:// se nezdařila"), "{text}");
        let (ok, text) = run_link_action(&Fake { fail_with: Some(DriveError::Unsupported), ..Default::default() }, exe, LinkAction::Unregister);
        assert!(!ok);
        assert!(text.contains("není na tomto operačním systému podporovaná"), "{text}");
        // the credential store may refuse: that is an error with its reason
        let host = Fake { sign_out_error: Some(DriveError::CredentialStore("zamčeno".into())), ..Default::default() };
        let (ok, text) = run_link_action(&host, exe, LinkAction::SignOut);
        assert!(!ok);
        assert!(text.contains("zamčeno"));
        // a build without an OAuth client has no sign-in to forget: said so, not an error
        let host = Fake { sign_out_error: Some(DriveError::NotConfigured), ..Default::default() };
        let (ok, text) = run_link_action(&host, exe, LinkAction::SignOut);
        assert!(ok);
        assert_eq!(text, strings::NOTHING_TO_SIGN_OUT);
    }

    #[test]
    fn the_help_page_of_the_launcher_names_these_menu_entries() {
        let page = include_str!("../../cad-drive-web/index.html");
        for entry in [strings::MENU_REGISTER_LINK, strings::MENU_UNREGISTER_LINK, strings::MENU_DRIVE_SIGN_OUT] {
            assert!(page.contains(entry), "the page does not name the menu entry {entry:?}");
        }
        assert!(page.contains(&format!("→ {}", strings::MENU_REGISTER_LINK)));
        assert!(!page.contains("Nastavení"));
    }

    #[test]
    fn a_rejected_link_has_the_link_error_text() {
        assert_eq!(
            link_error_text(&cad_drive::LinkError::BadCharacters),
            "Odkaz se nepodařilo otevřít:\nOdkaz obsahuje nepovolené znaky."
        );
    }

    #[test]
    fn download_folder_is_local_and_below_the_cache() {
        assert!(download_dir().starts_with(paths::cache_root()));
        let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        assert!(!download_dir().starts_with(exe_dir));
    }

    #[test]
    fn missing_oauth_client_gives_a_clear_czech_text() {
        let t = failure_text(&DriveError::NotConfigured);
        assert!(t.contains("Google Drive"));
        assert!(t.contains("CADVIEWER_DRIVE_CLIENT_ID"));
        assert!(t.contains("Ctrl+O"));
        let other = failure_text(&DriveError::Forbidden);
        assert!(other.starts_with("Soubor z Google Drive se nepodařilo otevřít:"));
    }

    #[test]
    fn prune_removes_the_oldest_files_but_never_the_one_just_opened() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.step");
        let newer = dir.path().join("new.step");
        std::fs::write(&old, vec![0u8; 600]).unwrap();
        std::fs::write(&newer, vec![0u8; 600]).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
        prune_downloads(dir.path(), 1000, &newer);
        assert!(!old.exists());
        assert!(newer.exists());
        // the file in use stays even when it alone is over the limit
        prune_downloads(dir.path(), 100, &newer);
        assert!(newer.exists());
    }

    fn set_age(path: &Path, age: std::time::Duration) {
        let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(std::time::SystemTime::now() - age).unwrap();
    }

    #[test]
    fn prune_leaves_a_download_in_progress_alone_and_removes_only_a_stale_one() {
        let dir = tempfile::tempdir().unwrap();
        let hours = |n: u64| std::time::Duration::from_secs(n * 3600);
        // another instance is writing this file now: the name `cad-drive` gives a download in progress
        let active = dir.path().join(".model.step.a1b2c3.part");
        let stale = dir.path().join(".old.step.d4e5f6.part");
        let done = dir.path().join("done.step");
        let older = dir.path().join("older.step");
        std::fs::write(&active, vec![0u8; 900]).unwrap();
        std::fs::write(&stale, vec![0u8; 900]).unwrap();
        std::fs::write(&done, vec![0u8; 600]).unwrap();
        std::fs::write(&older, vec![0u8; 600]).unwrap();
        set_age(&active, hours(1));
        set_age(&stale, hours(7));
        set_age(&older, hours(2));
        // far over the limit: the finished files are pruned, the download that is running is not even counted
        prune_downloads(dir.path(), 700, &done);
        assert!(active.exists(), "the download of another instance was removed");
        assert!(!stale.exists(), "the leftover of a download that stopped hours ago stays");
        assert!(done.exists());
        assert!(!older.exists());
        // the download in progress does not push a finished file over the limit either
        let again = dir.path().join("again.step");
        std::fs::write(&again, vec![0u8; 300]).unwrap();
        prune_downloads(dir.path(), 1000, &done);
        assert!(again.exists() && done.exists() && active.exists());
    }

    #[test]
    fn prune_does_not_follow_or_remove_links() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("kept.step");
        std::fs::write(&target, vec![0u8; 5000]).unwrap();
        let link = dir.path().join("link.step");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        if made.is_err() {
            eprintln!("SKIP: no symbolic link can be made here");
            return;
        }
        let real = dir.path().join("real.step");
        std::fs::write(&real, vec![0u8; 600]).unwrap();
        // the link is not counted (5000 bytes of its target would push the real file over the limit) and is not removed
        prune_downloads(dir.path(), 1000, &real);
        assert!(std::fs::symlink_metadata(&link).is_ok(), "the link was removed");
        assert!(target.exists(), "the file the link points to was removed");
        assert!(real.exists());
        prune_downloads(dir.path(), 100, &target);
        assert!(target.exists() && std::fs::symlink_metadata(&link).is_ok());
    }
}
