//! Locations and portable-mode rules. Every cache, settings, log, worker job and temp path of the application is
//! resolved here. The locations are always local per-user folders, never next to the executable, also in portable
//! mode (ARCHITECTURE.md, "Portable variant").
use std::fs::{self, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Counter behind `temp_seq`.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A number that no other call in this process gets. Together with the process number it names the temp file of one
/// attempt: a cancelled load can still be copying when the same file is opened again, and the two attempts must not
/// write the same temp file (Windows answers the second one with "used by another process", os error 32).
pub fn temp_seq() -> u64 {
    TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
}

/// A file of this name next to the executable switches on portable mode.
pub const PORTABLE_FLAG: &str = "portable.flag";

/// True when `exe_dir` holds the portable flag file.
pub fn is_portable_in(exe_dir: &Path) -> bool {
    exe_dir.join(PORTABLE_FLAG).is_file()
}

/// True when the running executable lies in a folder with the portable flag.
pub fn is_portable() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(is_portable_in))
        .unwrap_or(false)
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    env_dir(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_else(std::env::temp_dir)
}

/// Per-user folder for the model cache. Also holds job files, worker logs, temp files and local source copies.
/// Windows: `%LOCALAPPDATA%\StepViewer\cache` (the same folder as the Python version); elsewhere
/// `$XDG_CACHE_HOME/cad-viewer`.
pub fn cache_root() -> PathBuf {
    if cfg!(windows) {
        env_dir("LOCALAPPDATA")
            .unwrap_or_else(|| home_dir().join("AppData").join("Local"))
            .join("StepViewer")
            .join("cache")
    } else {
        env_dir("XDG_CACHE_HOME")
            .unwrap_or_else(|| home_dir().join(".cache"))
            .join("cad-viewer")
    }
}

/// Per-user folder for the settings file. Windows: `%APPDATA%\StepViewer`; elsewhere `$XDG_CONFIG_HOME/cad-viewer`.
pub fn config_root() -> PathBuf {
    if cfg!(windows) {
        env_dir("APPDATA")
            .unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
            .join("StepViewer")
    } else {
        env_dir("XDG_CONFIG_HOME")
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("cad-viewer")
    }
}

/// The settings file (JSON).
pub fn settings_file() -> PathBuf {
    config_root().join("settings.json")
}

/// Local copies of source models (portable mode), a subfolder of the cache.
pub fn sources_dir() -> PathBuf {
    cache_root().join("sources")
}

/// The source model to load. In portable mode the file is copied into the local `sources` folder first, because
/// files on a synced Drive may be online-only or change while they are read. Outside portable mode `src` itself.
pub fn local_source_copy(src: &Path) -> io::Result<PathBuf> {
    source_copy_if(is_portable(), src, &sources_dir())
}

/// `local_source_copy` with the mode and the target folder given explicitly (for tests).
pub fn source_copy_if(portable: bool, src: &Path, dir: &Path) -> io::Result<PathBuf> {
    if !portable {
        return Ok(src.to_path_buf());
    }
    copy_into(src, dir)
}

/// Longest original file name that is kept for a local copy; with the folder of the copy (40 characters) and the
/// cache folder in front, a longer name would come near the 260 characters that many Windows programs stop at. A
/// longer name gives the copy the plain name `<key><ext>` (an STL then shows that name in the tree).
const MAX_COPY_NAME: usize = 100;

/// Name of the local copy inside its own folder. The copy keeps the file name of the source (the 3D reader names an
/// STL after its file, and that name is the tree name and the key of the material assignments), unless the name is
/// long or odd: then it is `<key><ext>`.
fn copy_leaf(src: &Path, key: &str, ext: &str) -> String {
    match src.file_name().and_then(|n| n.to_str()) {
        Some(n)
            if !n.is_empty()
                && n.len() <= MAX_COPY_NAME
                && !n.contains(['/', '\\', ':', '\0'])
                && !n.ends_with(['.', ' '])
                // the cache pruning takes `.tmp.` for a leftover of a copy in progress
                && !n.contains(".tmp.") =>
        {
            n.to_string()
        }
        _ => format!("{key}{ext}"),
    }
}

/// Copies `src` into `dir/<key>/<original name>`. The key is made of the original path, size and mtime, so that an
/// unchanged source is reused (the reuse check looks at the key and the size, never at the mtime of the copy).
/// The copy is written under a temp name and renamed; a source that changes during the copy is refused. The copy gets
/// a fresh mtime: the cache pruning removes the least recently used files first, and a copy made of an old source
/// would otherwise look old.
fn copy_into(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    let meta = fs::metadata(src)?;
    if !meta.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "source is not a file"));
    }
    let size = meta.len();
    let mtime = mtime_ns(&meta)?;
    let ext = src
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let key = sha1_hex(&format!("{}|{}|{}", normcase_abspath(src)?, size, mtime));
    let leaf = copy_leaf(src, &key, &ext);
    let folder = dir.join(&key);
    let dst = folder.join(&leaf);
    if let Ok(m) = fs::metadata(&dst) {
        if m.is_file() && m.len() == size {
            touch(&dst);
            return Ok(dst);
        }
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(&folder)?;
    let tmp = copy_temp_path(&folder, &leaf);
    let result = fs::copy(src, &tmp).and_then(|_| {
        // a read-only source gives a read-only copy, which could neither be touched nor pruned
        if let Ok(m) = fs::metadata(&tmp) {
            let mut perm = m.permissions();
            if perm.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perm.set_readonly(false);
                let _ = fs::set_permissions(&tmp, perm);
            }
        }
        touch(&tmp);
        let after = fs::metadata(src)?;
        if after.len() != size || mtime_ns(&after)? != mtime {
            Err(io::Error::other("source changed during copy"))
        } else {
            fs::rename(&tmp, &dst)
        }
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map(|_| dst)
}

/// The temp name of one copy attempt: `<leaf>.tmp.<pid>.<seq>`. The `.tmp.` part is what the cache pruning recognises
/// as a leftover; the sequence number keeps two attempts in one process apart.
pub fn copy_temp_path(folder: &Path, leaf: &str) -> PathBuf {
    folder.join(format!("{leaf}.tmp.{}.{}", process::id(), temp_seq()))
}

fn touch(p: &Path) {
    if let Ok(f) = fs::OpenOptions::new().write(true).open(p) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// Modification time in nanoseconds since the Unix epoch (Python `st_mtime_ns`).
pub fn mtime_ns(meta: &Metadata) -> io::Result<i128> {
    let t = meta.modified()?;
    Ok(match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    })
}

/// Python `os.path.normcase(os.path.abspath(p))`: absolute, lexically normalised; on Windows with `\` separators
/// and lower case.
pub fn normcase_abspath(p: &Path) -> io::Result<String> {
    let abs = std::path::absolute(p)?;
    let s = lexical_normalize(&abs).to_string_lossy().into_owned();
    Ok(normcase_str(&s))
}

/// Python `os.path.normcase` for a string.
pub fn normcase_str(s: &str) -> String {
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s.to_string()
    }
}

/// Removes `.` components and resolves `..` lexically (Python `normpath`).
pub fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Python `os.path.realpath` for a path that may not exist yet: the longest existing ancestor is canonicalised (so a
/// junction or symbolic link anywhere above the missing part is resolved), the missing rest is appended as it is.
pub fn real_path(p: &Path) -> PathBuf {
    if let Ok(c) = fs::canonicalize(p) {
        return strip_verbatim(c);
    }
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let abs = lexical_normalize(&abs);
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut here: &Path = &abs;
    loop {
        if let Ok(c) = fs::canonicalize(here) {
            let mut out = strip_verbatim(c);
            for name in missing.iter().rev() {
                out.push(name);
            }
            return out;
        }
        match (here.parent(), here.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                here = parent;
            }
            // nothing above exists (or the path has no parent): the lexical form is all there is
            _ => return abs,
        }
    }
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    if cfg!(windows) {
        let s = p.to_string_lossy().into_owned();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    p
}

/// SHA-1 of a string as lower-case hex.
pub fn sha1_hex(s: &str) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_flag_next_to_exe() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_portable_in(dir.path()));
        fs::write(dir.path().join(PORTABLE_FLAG), b"").unwrap();
        assert!(is_portable_in(dir.path()));
    }

    #[test]
    fn running_test_binary_is_not_portable() {
        // The test binary lives in target\...\deps without a portable.flag.
        assert!(!is_portable());
    }

    #[test]
    fn local_folders_never_inside_exe_folder() {
        let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        for p in [cache_root(), config_root(), settings_file(), sources_dir()] {
            assert!(!p.starts_with(&exe_dir), "{} is inside the executable folder", p.display());
        }
    }

    #[test]
    fn settings_file_is_json_in_config_root() {
        assert_eq!(settings_file().file_name().unwrap(), "settings.json");
        assert!(settings_file().starts_with(config_root()));
    }

    #[test]
    fn sources_copy_not_used_outside_portable_mode() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("model.step");
        fs::write(&src, b"ISO-10303").unwrap();
        let out_dir = dir.path().join("local");
        let got = source_copy_if(false, &src, &out_dir).unwrap();
        assert_eq!(got, src);
        assert!(!out_dir.exists());
    }

    #[test]
    fn portable_copy_goes_to_local_folder_and_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("drive").join("model.step");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"ISO-10303-21;DATA;ENDSEC;").unwrap();
        let local = dir.path().join("local");
        let copy = source_copy_if(true, &src, &local).unwrap();
        assert!(copy.starts_with(&local));
        assert_eq!(copy.extension().unwrap(), "step");
        assert_eq!(fs::read(&copy).unwrap(), fs::read(&src).unwrap());
        // second call with unchanged size and mtime reuses the same copy
        let again = source_copy_if(true, &src, &local).unwrap();
        assert_eq!(again, copy);
        // no temp file left behind
        let names: Vec<_> = fs::read_dir(&local)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn portable_copy_changes_when_source_changes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("model.stl");
        fs::write(&src, b"solid a\nendsolid a\n").unwrap();
        let local = dir.path().join("local");
        let first = source_copy_if(true, &src, &local).unwrap();
        fs::write(&src, b"solid bb\nendsolid bb\n").unwrap();
        let second = source_copy_if(true, &src, &local).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&second).unwrap(), b"solid bb\nendsolid bb\n");
    }

    #[test]
    fn the_copy_keeps_the_original_name_in_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("drive").join("Díl 1.stl");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"solid a\nendsolid a\n").unwrap();
        let local = dir.path().join("local");
        let copy = source_copy_if(true, &src, &local).unwrap();
        // the 3D reader names an STL after its file: that is the tree name and the key of the material assignments
        assert_eq!(copy.file_stem().unwrap(), "Díl 1");
        assert_eq!(copy.file_name().unwrap(), "Díl 1.stl");
        // the folder of the copy is below the local folder and is named after the key, not after the source folder
        assert_eq!(copy.parent().unwrap().parent().unwrap(), local);
        assert_eq!(copy.parent().unwrap().file_name().unwrap().len(), 40);
        // a second source with the same name lands in a folder of its own
        let other = dir.path().join("other").join("Díl 1.stl");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, b"solid b\nendsolid b\n").unwrap();
        let copy2 = source_copy_if(true, &other, &local).unwrap();
        assert_ne!(copy2.parent(), copy.parent());
        assert_eq!(copy2.file_name(), copy.file_name());
    }

    #[test]
    fn odd_or_long_names_fall_back_to_the_key() {
        let key = "ab".repeat(20);
        let long = Path::new("x").join(format!("{}.step", "n".repeat(MAX_COPY_NAME)));
        for (src, expect_key) in [
            (Path::new("a/model.step"), false),
            (Path::new("a/model.tmp.step"), true),
            (Path::new("a/trailing."), true),
            (long.as_path(), true),
        ] {
            let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let leaf = copy_leaf(src, &key, &ext);
            assert_eq!(leaf.starts_with(&key), expect_key, "{src:?} -> {leaf}");
        }
    }

    #[test]
    fn the_copy_gets_a_fresh_mtime_while_the_reuse_check_follows_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("old.step");
        fs::write(&src, b"ISO-10303-21;DATA;ENDSEC;").unwrap();
        let long_ago = SystemTime::now() - std::time::Duration::from_secs(400 * 24 * 3600);
        fs::OpenOptions::new().write(true).open(&src).unwrap().set_modified(long_ago).unwrap();
        let local = dir.path().join("local");
        let copy = source_copy_if(true, &src, &local).unwrap();
        let age = |p: &Path| SystemTime::now().duration_since(fs::metadata(p).unwrap().modified().unwrap()).unwrap_or_default();
        // the pruning of the cache sorts by mtime: a copy that looked 400 days old would be removed first
        assert!(age(&copy) < std::time::Duration::from_secs(120), "copy is {:?} old", age(&copy));
        // the source is not touched
        assert!(age(&src) > std::time::Duration::from_secs(300 * 24 * 3600));
        // the copy is reused although its own mtime differs from the source's: the key and the size decide
        let again = source_copy_if(true, &src, &local).unwrap();
        assert_eq!(again, copy);
        assert_eq!(fs::read_dir(copy.parent().unwrap()).unwrap().count(), 1, "no second copy and no temp file");
        // a source with another mtime is another copy
        fs::OpenOptions::new().write(true).open(&src).unwrap().set_modified(long_ago + std::time::Duration::from_secs(5)).unwrap();
        assert_ne!(source_copy_if(true, &src, &local).unwrap(), copy);
    }

    #[test]
    fn a_read_only_source_gives_a_copy_that_can_be_touched_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("ro.step");
        fs::write(&src, b"x").unwrap();
        let mut perm = fs::metadata(&src).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(&src, perm).unwrap();
        let copy = source_copy_if(true, &src, &dir.path().join("local")).unwrap();
        assert!(!fs::metadata(&copy).unwrap().permissions().readonly());
        fs::remove_file(&copy).unwrap();
        let mut perm = fs::metadata(&src).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(&src, perm).unwrap();
    }

    /// A junction (Windows, no rights needed) or a symbolic link to a folder; false when the system refuses.
    fn make_dir_link(link: &Path, target: &Path) -> bool {
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = (link, target);
            false
        }
    }

    #[test]
    fn real_path_resolves_a_junction_above_several_missing_folders() {
        let dir = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(dir.path()).unwrap().join("outside");
        fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("inside");
        if !make_dir_link(&link, &real) {
            eprintln!("SKIP: no junction / symbolic link can be made here");
            return;
        }
        let expected = strip_verbatim(fs::canonicalize(&real).unwrap());
        // one missing level (the old code resolved this) and three (it did not)
        assert_eq!(real_path(&link.join("a.xbf")), expected.join("a.xbf"));
        assert_eq!(real_path(&link.join("x").join("y").join("a.xbf")), expected.join("x").join("y").join("a.xbf"));
        // the link itself and an existing file below it
        assert_eq!(real_path(&link), expected);
        fs::write(real.join("f.xbf"), b"x").unwrap();
        assert_eq!(real_path(&link.join("f.xbf")), expected.join("f.xbf"));
        // `..` in the missing part is resolved lexically
        assert_eq!(real_path(&link.join("x").join("..").join("b.xbf")), expected.join("b.xbf"));
    }

    #[test]
    fn real_path_of_an_unreachable_root_is_the_lexical_form() {
        // no ancestor exists on a drive that is not there; the path comes back normalised, never a panic
        let p = if cfg!(windows) { Path::new(r"Q:\neexistuje\a\..\b.xbf") } else { Path::new("/neexistuje-ani-koren/a/../b.xbf") };
        let r = real_path(p);
        assert!(r.ends_with("b.xbf"));
        assert!(!r.to_string_lossy().contains(".."));
    }

    #[test]
    fn normcase_abspath_resolves_dots() {
        let a = normcase_abspath(Path::new("/x/y/../z/./m.step")).unwrap();
        let b = normcase_abspath(Path::new("/x/z/m.step")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_copy_attempt_gets_its_own_temp_name() {
        // a cancelled load can still be copying when the file is opened again: the attempts must not share a temp file
        let folder = Path::new("cache").join("key");
        let (a, b) = (copy_temp_path(&folder, "m.step"), copy_temp_path(&folder, "m.step"));
        assert_ne!(a, b);
        assert_eq!(a.parent(), Some(folder.as_path()));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("m.step.tmp.") && name.contains(&process::id().to_string()), "{name}");
        // and the copy itself leaves no temp file behind, whichever attempt ran
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("drive").join("m.step");
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, b"ISO-10303-21;DATA;ENDSEC;").unwrap();
        let local = dir.path().join("local");
        let first = source_copy_if(true, &src, &local).unwrap();
        fs::remove_file(&first).unwrap();
        let second = source_copy_if(true, &src, &local).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(second.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn lexical_normalize_keeps_leading_parent() {
        assert_eq!(lexical_normalize(Path::new("a/../../b")), PathBuf::from("../b"));
        assert_eq!(lexical_normalize(Path::new("/../b")), PathBuf::from("/b"));
    }
}
