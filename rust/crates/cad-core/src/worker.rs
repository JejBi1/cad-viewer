//! Load worker protocol (port of main.py: run_worker, job_valid, serve_worker, spawn_worker, submit_job and the log
//! handling of poll_worker / on_worker_finished).
//!
//! A job is a JSON array `[src, dst]` in a job file. The GUI writes `<job>.w` and renames it to `<job>`, so the
//! worker never reads a half-written job. The worker deletes the job file, validates it, runs the load with
//! progress lines appended to `<dst>.log`, and ends the log with `DONE` or `ERROR <message>`.
//!
//! The load itself is passed in as closures (`read` and `save`), so this crate does not depend on occt-bridge.
use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::cache;
use crate::paths;

/// A pre-started worker waits this long for its job, then quits (`WORKER_IDLE_S`).
pub const WORKER_IDLE: Duration = Duration::from_secs(120);
/// A warm worker is only handed a job while it is younger than this (`_warm_ok`: `WORKER_IDLE_S - 15`).
pub const WARM_MAX_AGE: Duration = Duration::from_secs(105);
/// Command-line flag that starts a pre-started worker (`--worker-serve <job file>`).
pub const SERVE_FLAG: &str = "--worker-serve";
/// Command-line flag that runs one load directly (`--worker <src> <dst>`).
pub const WORKER_FLAG: &str = "--worker";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const HANDOVER_TEXT: &str = "Předávám model…";
const UNKNOWN_ERROR_TEXT: &str = "neznámá chyba";

/// A parsed job: the source model and the `.xbf` destination in the cache folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub src: PathBuf,
    pub dst: PathBuf,
}

/// A worker process started ahead of time, waiting for its job file.
#[derive(Debug)]
pub struct WarmWorker {
    pub child: Child,
    pub job_file: PathBuf,
    pub started: Instant,
}

impl WarmWorker {
    /// Still running and young enough to be handed a job.
    pub fn usable(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None)) && self.started.elapsed() < WARM_MAX_AGE
    }

    /// Kills the worker and removes its job file (`_drop_warm`).
    pub fn discard(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.job_file);
    }
}

/// `<dst>.log`, where progress and the final line are written.
pub fn log_path(dst: &Path) -> PathBuf {
    PathBuf::from(format!("{}.log", dst.display()))
}

/// Non-empty trimmed lines of the log (`_log_lines`). A missing log gives no lines.
pub fn log_lines(dst: &Path) -> Vec<String> {
    match fs::read_to_string(log_path(dst)) {
        Ok(text) => text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// `None` when the worker finished well (exit code 0 and last line `DONE`), otherwise the message to show:
/// the first `ERROR` line, or the exit code (`on_worker_finished`).
pub fn failure_message(lines: &[String], exit_code: i32) -> Option<String> {
    if exit_code == 0 && lines.last().map(String::as_str) == Some("DONE") {
        return None;
    }
    let err = lines
        .iter()
        .find_map(|l| l.strip_prefix("ERROR "))
        .map(str::to_string);
    Some(err.unwrap_or_else(|| format!("načítací proces skončil chybou (kód {exit_code})")))
}

/// Removes the partial model and its log (`_cleanup_tmp`).
pub fn remove_tmp(dst: &Path) {
    let _ = fs::remove_file(dst);
    let _ = fs::remove_file(log_path(dst));
}

fn append_line(log: &Path, msg: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{msg}");
    }
}

/// Error text for the log. Line breaks would split the log protocol, so they become spaces.
fn error_text<E: Display>(e: &E) -> String {
    let s = e.to_string().replace(['\r', '\n'], " ");
    if s.trim().is_empty() {
        UNKNOWN_ERROR_TEXT.to_string()
    } else {
        s
    }
}

/// Runs one load (`run_worker`). `read` loads the source and reports progress through the callback; `save` writes
/// the result to `dst`. Returns the process exit code: 0 on `DONE`, 1 on an error (the error is in the log).
pub fn run_worker<D, E1, E2>(
    src: &Path,
    dst: &Path,
    read: impl FnOnce(&Path, &mut dyn FnMut(&str)) -> Result<D, E1>,
    save: impl FnOnce(&D, &Path) -> Result<(), E2>,
) -> i32
where
    E1: Display,
    E2: Display,
{
    let log = log_path(dst);
    let mut report = |msg: &str| append_line(&log, msg);
    let doc = match read(src, &mut report) {
        Ok(doc) => doc,
        Err(e) => {
            report(&format!("ERROR {}", error_text(&e)));
            return 1;
        }
    };
    report(HANDOVER_TEXT);
    match save(&doc, dst) {
        Ok(()) => {
            report("DONE");
            0
        }
        Err(e) => {
            report(&format!("ERROR {}", error_text(&e)));
            1
        }
    }
}

/// Checks a job against the cache folder `root` (`job_valid`): absolute source path, `.xbf` destination that is
/// absolute and lies inside `root` (not `root` itself), no NUL characters.
pub fn job_valid_in(root: &Path, src: &Path, dst: &Path) -> bool {
    let s = src.to_string_lossy();
    let d = dst.to_string_lossy();
    if s.is_empty() || d.is_empty() || s.contains('\0') || d.contains('\0') {
        return false;
    }
    if !src.is_absolute() || !dst.is_absolute() {
        return false;
    }
    if !d.to_lowercase().ends_with(".xbf") {
        return false;
    }
    is_inside(root, dst)
}

/// True when `p` lies inside the folder `root` and is not `root` itself, after links and `..` are resolved (a path that
/// does not exist yet is judged by its existing part, `paths::real_path`). The rule for the destination of a job and for
/// the job file itself.
fn is_inside(root: &Path, p: &Path) -> bool {
    let root = norm_key(&paths::real_path(root));
    let real = norm_key(&paths::real_path(p));
    real != root && real.starts_with(&root)
}

/// `job_valid` against the per-user cache folder.
pub fn job_valid(src: &Path, dst: &Path) -> bool {
    job_valid_in(&cache::cache_dir(), src, dst)
}

fn norm_key(p: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(p.to_string_lossy().replace('/', "\\").to_lowercase())
    } else {
        p.to_path_buf()
    }
}

/// Serves one job from `job_file` (`serve_worker`). Waits up to `idle` for the file, reads and removes it, then runs
/// it when it is valid. Returns the exit code: 0 after a finished job or an idle timeout, 1 for an invalid job.
/// The job file comes from the command line (`--worker-serve <job file>`), so it is not trusted: a file that does not lie
/// inside the cache folder `root` (after links are resolved) is neither read nor removed, and the answer is 1.
/// The caller warms up the OCCT libraries before calling this, as the Python worker imports `core` first.
pub fn serve_worker_in<D, E1, E2>(
    root: &Path,
    job_file: &Path,
    idle: Duration,
    read: impl FnOnce(&Path, &mut dyn FnMut(&str)) -> Result<D, E1>,
    save: impl FnOnce(&D, &Path) -> Result<(), E2>,
) -> i32
where
    E1: Display,
    E2: Display,
{
    let end = Instant::now() + idle;
    while Instant::now() < end {
        // judged again for every attempt: a link may be put in the place of the job file while the worker waits
        if !is_inside(root, job_file) {
            return 1;
        }
        let raw = match fs::read_to_string(job_file) {
            Ok(raw) => raw,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let _ = fs::remove_file(job_file);
        let pair: Option<Vec<String>> = serde_json::from_str(&raw).ok();
        let job = match pair.as_deref() {
            Some([src, dst]) if job_valid_in(root, Path::new(src), Path::new(dst)) => Job {
                src: PathBuf::from(src),
                dst: PathBuf::from(dst),
            },
            _ => return 1,
        };
        return run_worker(&job.src, &job.dst, read, save);
    }
    0
}

/// `serve_worker` against the per-user cache folder and `WORKER_IDLE`.
pub fn serve_worker<D, E1, E2>(
    job_file: &Path,
    read: impl FnOnce(&Path, &mut dyn FnMut(&str)) -> Result<D, E1>,
    save: impl FnOnce(&D, &Path) -> Result<(), E2>,
) -> i32
where
    E1: Display,
    E2: Display,
{
    serve_worker_in(&cache::cache_dir(), job_file, WORKER_IDLE, read, save)
}

/// Writes a job for a waiting worker (`submit_job`): `<job>.w` first, then renamed over `<job>`.
pub fn submit(job_file: &Path, src: &Path, dst: &Path) -> io::Result<()> {
    let src = src
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source path is not UTF-8"))?;
    let dst = dst
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination path is not UTF-8"))?;
    let text = serde_json::to_string(&[src, dst]).map_err(io::Error::other)?;
    let tmp = PathBuf::from(format!("{}.w", job_file.display()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, job_file)
}

static JOB_SEQ: AtomicU64 = AtomicU64::new(0);

/// Starts an idle worker process `exe --worker-serve <job file>` (`spawn_worker`). The job file name is unique per
/// process and counter and lies in the cache folder.
pub fn spawn(exe: &Path) -> io::Result<WarmWorker> {
    let n = JOB_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let dir = cache::cache_dir();
    cache::ensure_dir(&dir)?;
    let job_file = dir.join(format!("job_{}_{}.tmp", std::process::id(), n));
    let _ = fs::remove_file(&job_file);
    let mut cmd = Command::new(exe);
    cmd.arg(SERVE_FLAG)
        .arg(&job_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd.spawn()?;
    Ok(WarmWorker {
        child,
        job_file,
        started: Instant::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn idle_timeout_is_120_s_and_warm_age_105_s() {
        assert_eq!(WORKER_IDLE, Duration::from_secs(120));
        assert_eq!(WARM_MAX_AGE, Duration::from_secs(105));
    }

    #[test]
    fn job_validation_rules() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let good_dst = root.join("abc.tmp.xbf");
        let src = if cfg!(windows) {
            PathBuf::from(r"C:\models\a.step")
        } else {
            PathBuf::from("/models/a.step")
        };
        assert!(job_valid_in(root, &src, &good_dst));
        // relative source
        assert!(!job_valid_in(root, Path::new("a.step"), &good_dst));
        // destination not ending in .xbf
        assert!(!job_valid_in(root, &src, &root.join("abc.dxf")));
        // destination outside the cache folder
        let other = tempfile::tempdir().unwrap();
        assert!(!job_valid_in(root, &src, &other.path().join("x.xbf")));
        // destination equal to the folder itself
        assert!(!job_valid_in(root, &src, root));
        // relative destination
        assert!(!job_valid_in(root, &src, Path::new("x.xbf")));
        // a sibling folder whose name starts with the cache name is not inside it
        let sibling = PathBuf::from(format!("{}x", root.display())).join("y.xbf");
        assert!(!job_valid_in(root, &src, &sibling));
        // NUL characters
        let nul = PathBuf::from(format!("{}\0.xbf", good_dst.display()));
        assert!(!job_valid_in(root, &src, &nul));
    }

    #[test]
    fn submit_writes_json_pair_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let job = dir.path().join("job_1_1.tmp");
        submit(&job, Path::new("/m/a b.step"), Path::new("/c/x.tmp.xbf")).unwrap();
        assert!(!dir.path().join("job_1_1.tmp.w").exists());
        let text = fs::read_to_string(&job).unwrap();
        let v: Vec<String> = serde_json::from_str(&text).unwrap();
        assert_eq!(v, strings(&["/m/a b.step", "/c/x.tmp.xbf"]));
    }

    #[test]
    fn run_worker_success_writes_handover_and_done() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("k.tmp.xbf");
        let code = run_worker(
            Path::new("/src.step"),
            &dst,
            |_src, report| -> Result<u32, String> {
                report("Čtu model…");
                Ok(7)
            },
            |doc, p| -> Result<(), String> {
                assert_eq!(*doc, 7);
                fs::write(p, b"xbf").map_err(|e| e.to_string())
            },
        );
        assert_eq!(code, 0);
        let lines = log_lines(&dst);
        assert_eq!(lines, strings(&["Čtu model…", "Předávám model…", "DONE"]));
        assert_eq!(failure_message(&lines, 0), None);
    }

    #[test]
    fn run_worker_read_error_is_logged_as_error() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("k.tmp.xbf");
        let saved = Cell::new(false);
        let code = run_worker(
            Path::new("/src.step"),
            &dst,
            |_s, _r| -> Result<(), String> { Err("Chyba v souboru".into()) },
            |_d, _p| -> Result<(), String> {
                saved.set(true);
                Ok(())
            },
        );
        assert_eq!(code, 1);
        assert!(!saved.get());
        let lines = log_lines(&dst);
        assert_eq!(lines, strings(&["ERROR Chyba v souboru"]));
        assert_eq!(
            failure_message(&lines, 1).as_deref(),
            Some("Chyba v souboru")
        );
    }

    #[test]
    fn run_worker_empty_error_gets_placeholder_and_single_line() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("k.tmp.xbf");
        let code = run_worker(
            Path::new("/src.step"),
            &dst,
            |_s, _r| -> Result<(), String> { Err("first\nsecond".into()) },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(code, 1);
        assert_eq!(log_lines(&dst), strings(&["ERROR first second"]));

        let dst2 = dir.path().join("j.tmp.xbf");
        run_worker(
            Path::new("/src.step"),
            &dst2,
            |_s, _r| -> Result<(), String> { Err(String::new()) },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(log_lines(&dst2), strings(&["ERROR neznámá chyba"]));
    }

    #[test]
    fn failure_message_without_error_line_gives_exit_code() {
        assert_eq!(
            failure_message(&[], 3).as_deref(),
            Some("načítací proces skončil chybou (kód 3)")
        );
        // exit 0 but no DONE: still a failure
        assert!(failure_message(&strings(&["Čtu"]), 0).is_some());
        // the first ERROR line wins
        let lines = strings(&["ERROR a", "ERROR b"]);
        assert_eq!(failure_message(&lines, 1).as_deref(), Some("a"));
    }

    #[test]
    fn serve_worker_runs_valid_job_and_removes_file() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("job_9_1.tmp");
        let dst = root.path().join("abc.tmp.xbf");
        let src = root.path().join("a.step");
        fs::write(&src, b"x").unwrap();
        submit(&job, &src, &dst).unwrap();
        let code = serve_worker_in(
            root.path(),
            &job,
            Duration::from_secs(5),
            |_s, _r| -> Result<(), String> { Ok(()) },
            |_d, p| -> Result<(), String> { fs::write(p, b"x").map_err(|e| e.to_string()) },
        );
        assert_eq!(code, 0);
        assert!(!job.exists());
        assert_eq!(log_lines(&dst).last().map(String::as_str), Some("DONE"));
    }

    #[test]
    fn serve_worker_rejects_invalid_job() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("job_9_2.tmp");
        // destination outside the cache folder
        fs::write(&job, r#"["/m/a.step", "/elsewhere/x.xbf"]"#).unwrap();
        let loaded = Cell::new(false);
        let code = serve_worker_in(
            root.path(),
            &job,
            Duration::from_secs(5),
            |_s, _r| -> Result<(), String> {
                loaded.set(true);
                Ok(())
            },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(code, 1);
        assert!(!loaded.get());
        assert!(!job.exists(), "the job file is removed before validation");
    }

    #[test]
    fn serve_worker_leaves_a_job_file_outside_the_cache_alone() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        // the command line names a file that is not a job of ours: it is neither read nor removed
        let precious = elsewhere.path().join("precious.tmp");
        let text = r#"["/m/a.step", "/elsewhere/x.xbf"]"#;
        fs::write(&precious, text).unwrap();
        let touched = Cell::new(false);
        let serve = |job: &Path| {
            serve_worker_in(
                root.path(),
                job,
                Duration::from_secs(5),
                |_s, _r| -> Result<(), String> {
                    touched.set(true);
                    Ok(())
                },
                |_d, _p| -> Result<(), String> { Ok(()) },
            )
        };
        assert_eq!(serve(&precious), 1);
        assert_eq!(fs::read_to_string(&precious).unwrap(), text, "the file was removed or changed");
        // the folder itself and a path that only starts inside the folder and climbs out of it
        assert_eq!(serve(root.path()), 1);
        assert!(root.path().exists());
        let climbing = root.path().join("..").join(elsewhere.path().file_name().unwrap()).join("precious.tmp");
        assert_eq!(serve(&climbing), 1);
        assert!(precious.exists(), "a path with .. went out of the folder");
        // a valid-looking job that lies outside is not run either
        let src = elsewhere.path().join("a.step");
        let dst = root.path().join("abc.tmp.xbf");
        fs::write(&src, b"x").unwrap();
        let outside_job = elsewhere.path().join("job_9_7.tmp");
        submit(&outside_job, &src, &dst).unwrap();
        assert_eq!(serve(&outside_job), 1);
        assert!(outside_job.exists() && !touched.get() && !log_path(&dst).exists());
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
    fn serve_worker_does_not_follow_a_link_out_of_the_cache() {
        let root = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(elsewhere.path()).unwrap();
        let link = root.path().join("jobs");
        if !make_dir_link(&link, &real) {
            eprintln!("SKIP: no junction / symbolic link can be made here");
            return;
        }
        // a job that would be valid if its file lay in the cache folder; it is outside, behind a link in the cache folder
        let src = real.join("a.step");
        fs::write(&src, b"x").unwrap();
        let dst = root.path().join("abc.tmp.xbf");
        submit(&real.join("job_9_8.tmp"), &src, &dst).unwrap();
        let loaded = Cell::new(false);
        let code = serve_worker_in(
            root.path(),
            &link.join("job_9_8.tmp"),
            Duration::from_secs(5),
            |_s, _r| -> Result<(), String> {
                loaded.set(true);
                Ok(())
            },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(code, 1);
        assert!(!loaded.get(), "a job behind a link out of the cache was run");
        assert!(real.join("job_9_8.tmp").exists(), "the file behind the link was removed");
        assert!(!log_path(&dst).exists());
    }

    #[test]
    fn serve_worker_rejects_malformed_json() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("job_9_3.tmp");
        fs::write(&job, "[\"only one\"]").unwrap();
        let code = serve_worker_in(
            root.path(),
            &job,
            Duration::from_secs(5),
            |_s, _r| -> Result<(), String> { Ok(()) },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn serve_worker_times_out_without_job() {
        let root = tempfile::tempdir().unwrap();
        let job = root.path().join("never.tmp");
        let start = Instant::now();
        let code = serve_worker_in(
            root.path(),
            &job,
            Duration::from_millis(200),
            |_s, _r| -> Result<(), String> { Ok(()) },
            |_d, _p| -> Result<(), String> { Ok(()) },
        );
        assert_eq!(code, 0);
        assert!(start.elapsed() >= Duration::from_millis(150));
    }

    #[test]
    fn remove_tmp_deletes_part_and_log() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("k.tmp.xbf");
        fs::write(&dst, b"p").unwrap();
        fs::write(log_path(&dst), b"DONE\n").unwrap();
        remove_tmp(&dst);
        assert!(!dst.exists());
        assert!(!log_path(&dst).exists());
    }
}
