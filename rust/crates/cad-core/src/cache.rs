//! Cache of converted models (`.xbf`) in the per-user cache folder. Port of main.py: cache_path, part_path,
//! store_cache, prune_cache, clear_cache (lines 22-25, 100-119, 339-390).
use std::fmt::Display;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::paths;

/// Cache key version suffix (was "1.9" in the Python version).
pub const VERSION: &str = "2.0";
/// Files from this size get a cached copy (`CACHE_MIN`, 1 MiB).
pub const CACHE_MIN: u64 = 1024 * 1024;
/// Total size of cached models kept on disk (`CACHE_LIMIT`, 3 GiB).
pub const CACHE_LIMIT: u64 = 3 * 1024 * 1024 * 1024;
/// Leftovers (temp, log, job files) older than this are removed.
pub const MAX_TMP_AGE: Duration = Duration::from_secs(6 * 3600);
/// Folder of the files downloaded from Google Drive, below the cache folder. It counts in the limit and in "clear".
pub const DRIVE_DIR: &str = "drive";
/// Folder of the local copies of portable-mode sources (`paths::sources_dir`), below the cache folder.
const SOURCES_DIR: &str = "sources";
/// The folder of the model that is being loaded: not in the cache, kept by "clear".
const MATERIALS_DIR: &str = "materials";

/// The cache folder of this user.
pub fn cache_dir() -> PathBuf {
    paths::cache_root()
}

/// Creates a folder. On Unix the leaf is private to the user (0700), as in `ensure_cache_dir`.
pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// The key text: `"<normcase abspath>|<size>|<mtime_ns>|2.0"`.
pub fn key_string(path_norm: &str, size: u64, mtime_ns: i128) -> String {
    format!("{path_norm}|{size}|{mtime_ns}|{VERSION}")
}

/// Cache key text for a source file (its original path, size and mtime).
pub fn cache_key(src: &Path) -> io::Result<String> {
    let meta = fs::metadata(src)?;
    Ok(key_string(
        &paths::normcase_abspath(src)?,
        meta.len(),
        paths::mtime_ns(&meta)?,
    ))
}

/// `<cache dir>/<sha1(key)>.xbf` for a source file.
pub fn cache_path(src: &Path) -> io::Result<PathBuf> {
    cache_path_in(&cache_dir(), src)
}

/// `cache_path` with the folder given.
pub fn cache_path_in(dir: &Path, src: &Path) -> io::Result<PathBuf> {
    Ok(dir.join(format!("{}.xbf", paths::sha1_hex(&cache_key(src)?))))
}

/// Working name of a conversion in progress: `<name>.<pid>.tmp.xbf` (OCCT appends `.xbf` when missing). The process
/// number keeps two running instances apart: there is no single-instance rule, and two windows that open the same file
/// would otherwise write the same partial model and the same `.log`.
pub fn part_path(cache: &Path) -> PathBuf {
    let s = cache.to_string_lossy();
    let base = s.strip_suffix(".xbf").unwrap_or(&s);
    PathBuf::from(format!("{base}.{}.tmp.xbf", std::process::id()))
}

/// Deletes the leftovers and keeps the cache under `CACHE_LIMIT` (`prune_cache`), using the current time.
pub fn prune(dir: &Path) {
    prune_at(dir, SystemTime::now());
}

/// `prune` with an explicit "now" (for tests). Stale temp files, logs and job files are removed, old DWG
/// conversion folders too. Then the newest models are kept until their running total exceeds the limit.
/// Local source copies in `dir/sources` (a folder per copy, see `paths::local_source_copy`) take part in the same
/// limit.
pub fn prune_at(dir: &Path, now: SystemTime) {
    prune_with_limit(dir, now, CACHE_LIMIT);
}

/// True for a name of a file that is still being written or was left behind by an interrupted write: a copy or a
/// conversion (`.tmp.`, `.tmp`) or a Google Drive download (`.part`). Such files are never counted in the limit.
pub fn is_temp_leftover(name: &str) -> bool {
    name.contains(".tmp.") || name.ends_with(".tmp") || name.ends_with(".part")
}

/// True for a symbolic link, a junction or any other reparse point. The pruning and "clear" never follow one.
pub fn is_link(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// A folder that is a folder and not a link: the only kind of folder the cache housekeeping walks into.
fn real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !is_link(&m))
}

/// True when the entry was last changed before `old` (the 6 h rule); `None` means no rule (nothing is stale).
fn older_than(meta: &Metadata, old: Option<SystemTime>) -> bool {
    match (old, meta.modified()) {
        (Some(o), Ok(t)) => t < o,
        _ => false,
    }
}

/// Removes a folder tree without following links: a link is removed as itself, what it points to stays.
fn remove_tree(p: &Path) {
    let Ok(meta) = fs::symlink_metadata(p) else { return };
    if is_link(&meta) {
        let _ = fs::remove_dir(p).or_else(|_| fs::remove_file(p));
    } else if meta.is_dir() {
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                remove_tree(&e.path());
            }
        }
        let _ = fs::remove_dir(p);
    } else {
        let _ = fs::remove_file(p);
    }
}

/// Folders below `dir/sources`: one per local copy of a source model. Links are not folders of the cache.
fn copy_folders(sources: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(sources) else { return Vec::new() };
    rd.flatten().map(|e| e.path()).filter(|p| real_dir(p)).collect()
}

fn prune_with_limit(dir: &Path, now: SystemTime, limit: u64) {
    let old = now.checked_sub(MAX_TMP_AGE);
    let sources = dir.join(SOURCES_DIR);
    let drive = dir.join(DRIVE_DIR);
    // a link below the cache may point anywhere: such a folder is not walked, and nothing is removed through it
    let sources_real = real_dir(&sources);
    let drive_real = real_dir(&drive);
    let folders = if sources_real { copy_folders(&sources) } else { Vec::new() };
    let mut places: Vec<PathBuf> = vec![dir.to_path_buf()];
    if sources_real {
        places.push(sources.clone());
    }
    places.extend(folders.iter().cloned());
    if drive_real {
        places.push(drive.clone());
    }
    for d in &places {
        let Ok(rd) = fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(meta) = fs::symlink_metadata(&p) else { continue };
            if is_link(&meta) {
                continue;
            }
            let stale = older_than(&meta, old);
            if meta.is_file() && stale && (is_temp_leftover(&name) || name.ends_with(".log") || name.ends_with(".w")) {
                let _ = fs::remove_file(&p);
            } else if d.as_path() == dir && name.starts_with("dwg_") && meta.is_dir() && stale {
                remove_tree(&p);
            }
        }
    }
    let mut files: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    collect_lru(dir, &mut files, |n| n.ends_with(".xbf") || n.ends_with(".dxf"));
    // copies of an older layout lie loose in `sources`, current ones in a folder each; the Drive downloads count too
    if sources_real {
        collect_lru(&sources, &mut files, |_| true);
    }
    for f in &folders {
        collect_lru(f, &mut files, |_| true);
    }
    if drive_real {
        collect_lru(&drive, &mut files, |_| true);
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut total = 0u64;
    for (_, p, size) in files {
        total += size;
        if total > limit {
            let _ = fs::remove_file(&p);
        }
    }
    // a folder whose copy is gone is removed when it is empty and older than the temp rule: a folder that was made
    // just now may be the home of a copy that is being written at this moment
    for f in &folders {
        if fs::symlink_metadata(f).is_ok_and(|m| older_than(&m, old)) {
            let _ = fs::remove_dir(f);
        }
    }
}

fn collect_lru(d: &Path, out: &mut Vec<(SystemTime, PathBuf, u64)>, keep: impl Fn(&str) -> bool) {
    let Ok(rd) = fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if is_temp_leftover(&name) || !keep(&name) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if !meta.is_file() || is_link(&meta) {
            continue;
        }
        if let Ok(m) = meta.modified() {
            out.push((m, p, meta.len()));
        }
    }
}

/// Removes everything in the cache folder except the `materials` folder (`clear_cache`). Model copies, DWG
/// conversion folders, local source copies and the Google Drive downloads go; a link is removed as a link and never
/// followed. Errors are ignored like the Python version.
pub fn clear(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name == MATERIALS_DIR {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if meta.is_dir() && !is_link(&meta) {
            if name.starts_with("dwg_") || name == SOURCES_DIR || name == DRIVE_DIR {
                remove_tree(&p);
            }
        } else if is_link(&meta) {
            remove_tree(&p);
        } else {
            let _ = fs::remove_file(&p);
        }
    }
}

/// Stores a finished conversion as `cache` (`store_cache`): `save` writes to the part path, which is renamed into
/// place, then the cache is pruned. On failure the part file is removed and the error returned.
pub fn store<E: Display>(cache: &Path, save: impl FnOnce(&Path) -> Result<(), E>) -> io::Result<()> {
    let dir = cache.parent().unwrap_or_else(|| Path::new("."));
    ensure_dir(dir)?;
    let part = part_path(cache);
    let result = save(&part)
        .map_err(|e| io::Error::other(e.to_string()))
        .and_then(|_| fs::rename(&part, cache));
    match result {
        Ok(()) => {
            prune(dir);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&part);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_has_version_suffix() {
        assert_eq!(
            key_string("c:\\data\\model.step", 1234, 1_700_000_000_123_456_789),
            "c:\\data\\model.step|1234|1700000000123456789|2.0"
        );
    }

    #[test]
    fn key_hash_matches_python_sha1() {
        // hashlib.sha1("c:\\data\\model.step|1234|1700000000123456789|2.0".encode()).hexdigest()
        let key = key_string("c:\\data\\model.step", 1234, 1_700_000_000_123_456_789);
        assert_eq!(
            paths::sha1_hex(&key),
            "9433f4b1987bededa66949903ea84388bbed757b"
        );
    }

    #[test]
    fn part_path_replaces_xbf_suffix_and_is_private_to_the_process() {
        let c = Path::new("/cache/abc.xbf");
        let p = part_path(c);
        assert_eq!(p, PathBuf::from(format!("/cache/abc.{}.tmp.xbf", std::process::id())));
        // the pruning takes it for a leftover, the model limit does not count it, a job to a worker accepts it
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.contains(".tmp.") && name.ends_with(".xbf"));
        // another instance has another name, so two windows opening one file never share the partial model or its log
        let other = format!("/cache/abc.{}.tmp.xbf", std::process::id().wrapping_add(1));
        assert_ne!(p, PathBuf::from(other));
    }

    #[test]
    fn prune_walks_the_folders_of_local_source_copies() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(7 * 3600);
        let sources = dir.path().join("sources");
        let (a, b, c) = (sources.join("aaaa"), sources.join("bbbb"), sources.join("cccc"));
        for f in [&a, &b, &c] {
            fs::create_dir_all(f).unwrap();
        }
        // a copy of the newest use, an older one, and a stale leftover of a copy in progress
        touch_at(&a.join("Díl.stl"), 600, now - Duration::from_secs(10));
        touch_at(&b.join("Starý.step"), 600, now - Duration::from_secs(3000));
        touch_at(&b.join("Starý.step.tmp.123"), 5, old);
        touch_at(&c.join("Hotový.step.tmp.77"), 5, old);
        // a copy of the older, flat layout still takes part
        touch_at(&sources.join("0123.step"), 600, now - Duration::from_secs(5000));
        prune_with_limit(dir.path(), now, 1000);
        assert!(a.join("Díl.stl").exists(), "the newest copy stays");
        assert!(!b.join("Starý.step").exists(), "the second one goes over the limit");
        assert!(!sources.join("0123.step").exists());
        assert!(!b.join("Starý.step.tmp.123").exists(), "a stale leftover goes");
        // the folders were changed just now: they are younger than the temp rule and stay until it allows
        assert!(a.exists() && b.exists() && c.exists(), "young folders stay");
        prune_with_limit(dir.path(), now + Duration::from_secs(7 * 3600), 1000);
        assert!(!b.exists() && !c.exists(), "folders that are empty and old enough now go");
        assert!(a.exists());
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
    fn prune_never_follows_a_link_below_the_cache() {
        // a junction in the sources folder points at a folder outside the cache: its files are not the cache's
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        touch_at(&outside.join("keep.xbf"), 900, now - Duration::from_secs(10 * 3600));
        touch_at(&outside.join("old.tmp.9"), 5, now - Duration::from_secs(10 * 3600));
        let cache = root.path().join("cache");
        let sources = cache.join("sources");
        fs::create_dir_all(&sources).unwrap();
        if !make_dir_link(&sources.join("link"), &outside) {
            eprintln!("SKIP: no junction / symbolic link can be made here");
            return;
        }
        prune_with_limit(&cache, now, 10);
        assert!(outside.join("keep.xbf").exists(), "nothing is removed through a link");
        assert!(outside.join("old.tmp.9").exists(), "a leftover behind a link is not the cache's either");
        // the link itself is not a folder of the cache: clear removes it as a link and leaves the target
        clear(&cache);
        assert!(outside.join("keep.xbf").exists() && outside.join("old.tmp.9").exists());
        assert!(!sources.exists(), "the sources folder went with clear");
    }

    #[test]
    fn a_link_as_the_sources_folder_is_not_walked_or_removed_through() {
        let root = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let outside = root.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        touch_at(&outside.join("a.xbf"), 900, now - Duration::from_secs(10 * 3600));
        let cache = root.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        if !make_dir_link(&cache.join("sources"), &outside) {
            eprintln!("SKIP: no junction / symbolic link can be made here");
            return;
        }
        prune_with_limit(&cache, now, 10);
        clear(&cache);
        assert!(outside.join("a.xbf").exists(), "the target of the link stays");
    }

    #[test]
    fn a_folder_made_for_a_copy_in_progress_is_not_removed_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let sources = dir.path().join("sources");
        fs::create_dir_all(sources.join("being-copied")).unwrap();
        // the folder is empty for the moment: the copy writes its temp file a moment later
        prune_with_limit(dir.path(), now, 1000);
        assert!(sources.join("being-copied").exists(), "a young empty folder stays");
        prune_with_limit(dir.path(), now + Duration::from_secs(7 * 3600), 1000);
        assert!(!sources.join("being-copied").exists(), "an old empty folder goes");
    }

    #[test]
    fn drive_downloads_count_in_the_limit_and_their_partial_files_go() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let drive = dir.path().join(DRIVE_DIR);
        fs::create_dir_all(&drive).unwrap();
        touch_at(&drive.join("old.step"), 800, now - Duration::from_secs(1000));
        touch_at(&drive.join(".x.abc.part"), 5, now - Duration::from_secs(7 * 3600));
        touch_at(&drive.join(".y.def.part"), 5, now);
        touch_at(&dir.path().join("model.xbf"), 800, now);
        prune_with_limit(dir.path(), now, 1000);
        assert!(dir.path().join("model.xbf").exists(), "the newest model stays");
        assert!(!drive.join("old.step").exists(), "the download is over the limit with the models");
        assert!(!drive.join(".x.abc.part").exists(), "an abandoned partial download goes");
        assert!(drive.join(".y.def.part").exists(), "a partial download in progress stays");
    }

    #[test]
    fn clear_removes_drive_downloads_and_keeps_materials() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(DRIVE_DIR)).unwrap();
        fs::write(dir.path().join(DRIVE_DIR).join("model.step"), b"x").unwrap();
        fs::create_dir_all(dir.path().join(MATERIALS_DIR)).unwrap();
        fs::write(dir.path().join(MATERIALS_DIR).join("m.json"), b"{}").unwrap();
        clear(dir.path());
        assert!(!dir.path().join(DRIVE_DIR).exists(), "the Drive downloads are cleared");
        assert!(dir.path().join(MATERIALS_DIR).join("m.json").exists());
    }

    #[test]
    fn a_fresh_copy_of_an_old_source_is_not_pruned_first() {
        // the copy has the time it was made, so the least recently used one is the one that goes
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let sources = dir.path().join("sources");
        fs::create_dir_all(sources.join("new")).unwrap();
        fs::create_dir_all(sources.join("used-long-ago")).unwrap();
        touch_at(&sources.join("new").join("a.step"), 600, now);
        touch_at(&sources.join("used-long-ago").join("b.step"), 600, now - Duration::from_secs(86_400));
        prune_with_limit(dir.path(), now, 700);
        assert!(sources.join("new").join("a.step").exists());
        assert!(!sources.join("used-long-ago").join("b.step").exists());
    }

    #[test]
    fn cache_path_follows_source_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("m.step");
        fs::write(&src, b"abc").unwrap();
        let cache = dir.path().join("cache");
        let p1 = cache_path_in(&cache, &src).unwrap();
        assert!(p1.to_string_lossy().ends_with(".xbf"));
        assert_eq!(p1.parent().unwrap(), cache);
        // a changed size changes the key
        fs::write(&src, b"abcdef").unwrap();
        let p2 = cache_path_in(&cache, &src).unwrap();
        assert_ne!(p1, p2);
    }

    fn touch_at(p: &Path, bytes: usize, at: SystemTime) {
        fs::write(p, vec![0u8; bytes]).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    #[test]
    fn prune_removes_old_temp_files_and_keeps_fresh_ones() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(7 * 3600);
        let fresh = now - Duration::from_secs(60);
        touch_at(&dir.path().join("a.tmp.xbf"), 10, old);
        touch_at(&dir.path().join("a.xbf.log"), 10, old);
        touch_at(&dir.path().join("job_1_1.tmp.w"), 10, old);
        touch_at(&dir.path().join("job_1_2.tmp"), 10, old);
        touch_at(&dir.path().join("b.tmp.xbf"), 10, fresh);
        touch_at(&dir.path().join("c.log"), 10, fresh);
        fs::create_dir(dir.path().join("dwg_old")).unwrap();
        fs::write(dir.path().join("dwg_old").join("x.dxf"), b"0").unwrap();
        prune_at(dir.path(), now);
        assert!(!dir.path().join("a.tmp.xbf").exists());
        assert!(!dir.path().join("a.xbf.log").exists());
        assert!(!dir.path().join("job_1_1.tmp.w").exists());
        assert!(!dir.path().join("job_1_2.tmp").exists());
        assert!(dir.path().join("b.tmp.xbf").exists());
        assert!(dir.path().join("c.log").exists());
        // the DWG folder was created just now: kept at `now`, removed once it is older than 6 h
        assert!(dir.path().join("dwg_old").exists());
        prune_at(dir.path(), now + Duration::from_secs(7 * 3600));
        assert!(!dir.path().join("dwg_old").exists());
    }

    #[test]
    fn prune_limits_total_size_by_mtime_order() {
        // The limit is 3 GiB; use sparse files so the test stays small.
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let big = 2u64 * 1024 * 1024 * 1024; // 2 GiB each
        let mk = |name: &str, at: SystemTime| {
            let p = dir.path().join(name);
            let f = fs::File::create(&p).unwrap();
            f.set_len(big).unwrap();
            f.set_modified(at).unwrap();
        };
        mk("newest.xbf", now);
        mk("middle.xbf", now - Duration::from_secs(100));
        mk("oldest.dxf", now - Duration::from_secs(200));
        prune_at(dir.path(), now);
        // newest (2 GiB) kept; middle brings the total to 4 GiB > 3 GiB and is removed;
        // oldest is also over the limit by the running total and is removed
        assert!(dir.path().join("newest.xbf").exists());
        assert!(!dir.path().join("middle.xbf").exists());
        assert!(!dir.path().join("oldest.dxf").exists());
        assert!(CACHE_LIMIT < 2 * big);
    }

    #[test]
    fn prune_ignores_tmp_xbf_in_limit() {
        let dir = tempfile::tempdir().unwrap();
        let big = 2u64 * 1024 * 1024 * 1024;
        let f = fs::File::create(dir.path().join("in_progress.tmp.xbf")).unwrap();
        f.set_len(big).unwrap();
        let g = fs::File::create(dir.path().join("keep.xbf")).unwrap();
        g.set_len(big).unwrap();
        prune_at(dir.path(), SystemTime::now());
        assert!(dir.path().join("keep.xbf").exists());
    }

    #[test]
    fn clear_keeps_materials_folder() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.xbf"), b"x").unwrap();
        fs::create_dir_all(dir.path().join("materials")).unwrap();
        fs::write(dir.path().join("materials").join("m.json"), b"{}").unwrap();
        fs::create_dir_all(dir.path().join("dwg_1")).unwrap();
        clear(dir.path());
        assert!(!dir.path().join("a.xbf").exists());
        assert!(!dir.path().join("dwg_1").exists());
        assert!(dir.path().join("materials").join("m.json").exists());
    }

    #[test]
    fn store_renames_into_place_and_cleans_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("k.xbf");
        store(&cache, |p| -> Result<(), String> {
            fs::write(p, b"model").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(fs::read(&cache).unwrap(), b"model");
        assert!(!part_path(&cache).exists());

        let cache2 = dir.path().join("j.xbf");
        let err = store(&cache2, |p| -> Result<(), String> {
            fs::write(p, b"partial").map_err(|e| e.to_string())?;
            Err("OCCT failed".into())
        });
        assert!(err.is_err());
        assert!(!part_path(&cache2).exists());
        assert!(!cache2.exists());
    }
}
