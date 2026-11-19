//! DWG to DXF conversion: ODA File Converter when installed, else the bundled LibreDWG `dwg2dxf`
//! (port of `view2d.dwg_to_dxf` / `load_drawing`).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const TIMEOUT: Duration = Duration::from_secs(600);
/// The converter is not installed. The installer without it and the portable folder offer it as a separate add-on
/// (`StepViewer-dwg-addon-<version>.zip`, see `xtask`); the message says where to get it and where it goes.
pub const MISSING_CONVERTER: &str = "Chybí převodník DWG (dwg2dxf z LibreDWG, případně ODA File Converter). Tato instalace ho neobsahuje: soubory DWG se otevřou po instalaci doplňku pro DWG (StepViewer-dwg-addon-*.zip) – rozbalte ho do složky programu vedle StepViewer.exe (vznikne složka third_party\\libredwg). DXF, STEP, IGES a STL fungují i bez něj.";
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// ODA File Converter (Windows: `ODA\ODAFileConverter*\ODAFileConverter.exe` under Program Files, other systems:
/// `PATH` and `/opt`, `/usr/bin`). The newest installation wins.
pub fn find_oda() -> Option<PathBuf> {
    let mut hits: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(base) = std::env::var_os(var) {
                let oda = PathBuf::from(base).join("ODA");
                if let Ok(rd) = std::fs::read_dir(&oda) {
                    for d in rd.flatten() {
                        let name = d.file_name().to_string_lossy().to_string();
                        if name.starts_with("ODAFileConverter") {
                            let exe = d.path().join("ODAFileConverter.exe");
                            if exe.is_file() {
                                hits.push(exe);
                            }
                        }
                    }
                }
            }
        }
    } else {
        if let Some(p) = which("ODAFileConverter") {
            hits.push(p);
        }
        for dir in ["/opt", "/usr/bin"] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for d in rd.flatten() {
                    let name = d.file_name().to_string_lossy().to_string();
                    if name.starts_with("ODAFileConverter") {
                        let p = d.path();
                        let exe = if p.is_dir() { p.join("ODAFileConverter") } else { p };
                        if exe.is_file() {
                            hits.push(exe);
                        }
                    }
                }
            }
        }
    }
    hits.sort();
    hits.pop()
}

#[cfg(not(windows))]
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}

#[cfg(windows)]
fn which(_name: &str) -> Option<PathBuf> {
    None // Windows never searches PATH or the current folder (executable planting)
}

/// Folders searched for `libredwg/dwg2dxf.exe`: the explicit `third_party` folder of the caller, then
/// `<exe dir>/third_party`. The parent and grandparent folders of the executable are searched only when
/// `allow_parents` is set (a build tree: `target/debug/..`); a released program runs a converter from its own installation
/// folder only, never from a folder somebody else may have write access to (the downloads folder, a shared drive root).
pub fn converter_dirs(third_party: Option<&Path>, exe_dir: Option<&Path>, allow_parents: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(t) = third_party {
        dirs.push(t.to_path_buf());
    }
    if let Some(d) = exe_dir {
        dirs.push(d.join("third_party"));
        if allow_parents {
            dirs.push(d.join("..").join("third_party"));
            dirs.push(d.join("..").join("..").join("third_party"));
        }
    }
    dirs
}

/// Bundled LibreDWG: `<third_party>/libredwg/dwg2dxf.exe` (Windows) or `libredwg-linux/dwg2dxf` (+ PATH on Linux).
pub fn find_dwg2dxf(third_party: Option<&Path>) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok();
    find_dwg2dxf_in(&converter_dirs(third_party, exe.as_deref().and_then(|e| e.parent()), cfg!(debug_assertions)))
}

fn find_dwg2dxf_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    let sub = if cfg!(windows) { "libredwg" } else { "libredwg-linux" };
    let file = if cfg!(windows) { "dwg2dxf.exe" } else { "dwg2dxf" };
    for d in dirs {
        let p = d.join(sub).join(file);
        if p.is_file() {
            return Some(p);
        }
    }
    if cfg!(windows) {
        return None;
    }
    which("dwg2dxf")
}

/// Message of a conversion that was stopped because the load was cancelled.
pub const CANCELLED: &str = "Převod DWG byl zrušen.";

/// A path that a command line parser cannot take for an option: a relative name that starts with `-` (a file called
/// `-v9.dwg` or `--help.dwg` next to the drawing) gets the current folder in front of it.
pub fn safe_arg_path(p: &Path) -> PathBuf {
    if p.is_relative() && p.to_string_lossy().starts_with('-') {
        Path::new(".").join(p)
    } else {
        p.to_path_buf()
    }
}

/// Names of the files LibreDWG works with: the drawing is copied to `STAGED_INPUT` in a private folder of its own, the
/// converter runs in that folder and writes `STAGED_OUTPUT` there. `dwg2dxf.exe` takes its file names in the ANSI code
/// page and in at most 260 characters, so a drawing called `Výkres 图纸.dwg`, one in a folder that is not in the code
/// page of the system, or one 300 characters deep could not be opened. The names it gets are plain and short.
pub const STAGED_INPUT: &str = "in.dwg";
pub const STAGED_OUTPUT: &str = "out.dxf";

/// `dwg2dxf` in the folder `work`: quiet, overwrite, output file, then the input file, both relative to `work`.
fn libredwg_command(exe: &Path, work: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.current_dir(work).args(["-v0", "-y", "-o", STAGED_OUTPUT, STAGED_INPUT]);
    cmd
}

/// Copies `src` into a private folder under its plain name, lets `make` build the converter command for that folder, runs
/// it and copies the DXF it wrote to `dst`. The folder is removed afterwards.
fn convert_staged(src: &Path, dst: &Path, cancel: Option<&AtomicBool>, make: &dyn Fn(&Path) -> Command) -> Result<(), String> {
    let work = create_private_dir(None, PREFIX_DWG_IN).map_err(|e| format!("Převod DWG selhal: {}", e))?;
    std::fs::copy(src, work.path().join(STAGED_INPUT)).map_err(|e| format!("Převod DWG selhal: {}", e))?;
    let (code, err) = run_cancellable(make(work.path()), TIMEOUT, cancel)?;
    let out = work.path().join(STAGED_OUTPUT);
    if code != Some(0) || !out.exists() {
        let msg = String::from_utf8_lossy(&err).trim().lines().last().map(|l| l.to_string());
        return Err(format!("Převod DWG selhal{}", msg.map(|m| format!(": {}", m)).unwrap_or_else(|| ".".into())));
    }
    std::fs::copy(&out, dst).map_err(|e| format!("Převod DWG selhal: {}", e))?;
    Ok(())
}

/// Windows: the converter runs in a job object with kill-on-close. The job is closed when the conversion ends, and the
/// operating system closes it when this process ends, so a converter cannot outlive the program (for example when the
/// window is closed while a DWG is being converted). Declared here directly: this crate has no Windows bindings.
#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    type Handle = *mut c_void;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    /// `JOBOBJECT_BASIC_LIMIT_INFORMATION`.
    #[repr(C)]
    struct BasicLimit {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// `IO_COUNTERS`.
    #[repr(C)]
    struct IoCounters {
        counters: [u64; 6],
    }

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`.
    #[repr(C)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, length: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    /// A job object that holds one process. Dropping it closes the job, and with kill-on-close its processes end.
    pub struct Job(Handle);

    impl Job {
        /// A job that holds `child`; `None` when the system refuses (the converter then runs without one).
        pub fn holding(child: &Child) -> Option<Job> {
            // SAFETY: plain Win32 calls with pointers to local, correctly laid out structs of the given size; the handle
            // is closed by `Drop` on every path (a job that is not set up is closed before it is returned as `None`).
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle);
                let mut info: ExtendedLimit = std::mem::zeroed();
                info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                    (&info as *const ExtendedLimit).cast(),
                    std::mem::size_of::<ExtendedLimit>() as u32,
                );
                if set == 0 || AssignProcessToJobObject(job.0, child.as_raw_handle()) == 0 {
                    return None;
                }
                Some(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from `CreateJobObjectW` and is closed once, here.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// How much of the end of the converter's error output is kept. Callers show only its last line; a converter that
/// floods the pipe must not make the program (the 2 GiB thumbnail job) hold all of it.
pub const STDERR_TAIL: usize = 16 * 1024;
/// How long the reader of the error output may take to see the end of the pipe once the converter and its job are gone.
/// Closing the job ends every process in it, so this only matters when the system refused the job (or on a system
/// without one) and a grandchild of the converter still holds the pipe: the output read so far is then used.
const READER_GRACE: Duration = if cfg!(test) { Duration::from_secs(15) } else { Duration::from_secs(5) };

/// Reads `src` to its end and keeps only the last `cap` bytes of it in `tail` (trimmed when the buffer reaches twice
/// that, so a flood never takes more than `2 * cap` plus one read).
fn drain_tail(src: &mut dyn std::io::Read, cap: usize, tail: &std::sync::Mutex<Vec<u8>>) {
    let mut buf = vec![0u8; 8192];
    loop {
        match src.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
                t.extend_from_slice(&buf[..n]);
                if t.len() > 2 * cap {
                    let cut = t.len() - cap;
                    t.drain(..cut);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Reads the error output of the converter on a thread of its own, keeping the last `STDERR_TAIL` bytes.
struct StderrTail {
    tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

impl StderrTail {
    fn start(mut src: Option<impl std::io::Read + Send + 'static>) -> StderrTail {
        let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, done) = std::sync::mpsc::channel();
        let sink = tail.clone();
        let thread = std::thread::spawn(move || {
            if let Some(s) = src.as_mut() {
                drain_tail(s, STDERR_TAIL, &sink);
            }
            let _ = tx.send(());
        });
        StderrTail { tail, done, thread }
    }

    /// The kept tail. Waits for the end of the pipe for `grace` at the most; a reader that is still blocked after that
    /// (a process outside the job holds the pipe) is left to end on its own and the output so far is returned.
    fn finish(self, grace: Duration) -> Vec<u8> {
        if self.done.recv_timeout(grace).is_ok() {
            let _ = self.thread.join();
        }
        let mut t = self.tail.lock().unwrap_or_else(|e| e.into_inner());
        let keep = t.len().saturating_sub(STDERR_TAIL);
        t.drain(..keep);
        std::mem::take(&mut *t)
    }
}

#[cfg(not(windows))]
mod job {
    /// No job objects here: nothing holds the converter's process tree.
    #[allow(dead_code)]
    pub struct Job;
    impl Job {
        pub fn holding(_child: &std::process::Child) -> Option<Job> {
            None
        }
    }
}

/// Runs the converter, waits for it for at most `timeout` and stops it as soon as `cancel` is set. Of its error output
/// the last `STDERR_TAIL` bytes are returned. On every way out the job is closed first (which ends the converter and
/// every process it started), then the reader of the pipe is waited for: a grandchild that holds the pipe cannot keep
/// this function from returning.
fn run_cancellable(mut cmd: Command, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<(Option<i32>, Vec<u8>), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let is_cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    if is_cancelled() {
        return Err(CANCELLED.into());
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // closing it ends the converter and its children if they are still running: dropped before the reader is joined,
    // and, as a last resort, when this function returns
    let mut job = job::Job::holding(&child);
    let reader = StderrTail::start(child.stderr.take());
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                drop(job.take());
                return Ok((status.code(), reader.finish(READER_GRACE)));
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(job.take());
                    reader.finish(READER_GRACE);
                    return Err("Převod DWG překročil časový limit.".into());
                }
                if is_cancelled() {
                    let _ = child.kill();
                    let _ = child.wait();
                    drop(job.take());
                    reader.finish(READER_GRACE);
                    return Err(CANCELLED.into());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                drop(job.take());
                reader.finish(READER_GRACE);
                return Err(e.to_string());
            }
        }
    }
}

/// Converts `src` to the DXF file `dst`; returns the name of the converter.
pub fn dwg_to_dxf(src: &Path, dst: &Path, third_party: Option<&Path>) -> Result<&'static str, String> {
    dwg_to_dxf_cancellable(src, dst, third_party, None)
}

/// `dwg_to_dxf` that stops the converter process as soon as `cancel` is set (the error is then `CANCELLED`).
pub fn dwg_to_dxf_cancellable(src: &Path, dst: &Path, third_party: Option<&Path>, cancel: Option<&AtomicBool>) -> Result<&'static str, String> {
    if let Some(oda) = find_oda() {
        // ODA works on folders: a private input folder with the file and a private output folder. The copy gets a plain
        // name: the name of the drawing is neither a pattern for the converter's file filter nor an option
        let tmp_in = create_private_dir(None, PREFIX_ODA_IN).map_err(|e| e.to_string())?;
        let tmp_out = create_private_dir(None, PREFIX_ODA_OUT).map_err(|e| e.to_string())?;
        src.file_name().ok_or("neplatný název souboru")?;
        let name = std::ffi::OsStr::new("input.dwg");
        std::fs::copy(src, tmp_in.path().join(name)).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&oda);
        cmd.arg(tmp_in.path()).arg(tmp_out.path()).arg("ACAD2018").arg("DXF").arg("0").arg("1").arg(name);
        // ODA File Converter is a Qt application too: it must not inherit our Qt settings
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().to_ascii_uppercase().starts_with("QT_") {
                cmd.env_remove(&k);
            }
        }
        let (code, err) = run_cancellable(cmd, TIMEOUT, cancel)?;
        let out = tmp_out.path().join(Path::new(name).with_extension("dxf"));
        if code != Some(0) && !out.exists() {
            let msg = String::from_utf8_lossy(&err).trim().lines().last().map(|l| l.to_string());
            return Err(format!("Převod DWG selhal{}", msg.map(|m| format!(": {}", m)).unwrap_or_else(|| ".".into())));
        }
        std::fs::copy(&out, dst).map_err(|e| format!("Převod DWG selhal: {}", e))?;
        return Ok("ODA File Converter");
    }
    let exe = find_dwg2dxf(third_party).ok_or_else(|| MISSING_CONVERTER.to_string())?;
    convert_staged(src, dst, cancel, &|work| libredwg_command(&exe, work))?;
    Ok("LibreDWG")
}

/// Folders that the conversions of a DWG make in the temporary folder: the private folder of `dwg2dxf` (`PREFIX_DWG_IN`), the
/// two of the ODA File Converter, the folder of the converted file that is read afterwards (`PREFIX_DXF`, next to the cache) and
/// the short copy of a 3D model with a long path that a thumbnail is made from (`PREFIX_THUMB_IN`, made by `cad-app`). The
/// program removes them when it is done; a program that was killed in the middle (the task manager, the time limit of the
/// shell, a power failure) leaves them.
///
/// The names are distinctive (`StepViewer-...`) and every such folder holds `MARKER_FILE` from the moment it is made. The
/// temporary folder is shared with every other program, and a name of a few letters is not a proof that a folder is ours
/// (a folder `dwg_backup` of someone else is as long as `dwg_` and six characters): only a folder that has one of these
/// names *and* the marker is ever removed.
pub const STALE_PREFIXES: [&str; 5] = [PREFIX_DWG_IN, PREFIX_ODA_IN, PREFIX_ODA_OUT, PREFIX_DXF, PREFIX_THUMB_IN];
/// Start of the name of the private folder of the `dwg2dxf` conversion.
pub const PREFIX_DWG_IN: &str = "StepViewer-dwg-in-";
/// Start of the names of the input and the output folder of the ODA File Converter.
pub const PREFIX_ODA_IN: &str = "StepViewer-oda-in-";
pub const PREFIX_ODA_OUT: &str = "StepViewer-oda-out-";
/// Start of the name of the folder that receives the converted DXF (next to the cache file, else in the temporary folder).
pub const PREFIX_DXF: &str = "StepViewer-dxf-";
/// Start of the name of the short copy of a 3D model that `cad-app` makes for a thumbnail (make it with `create_private_dir`,
/// or mark a folder it made itself with `mark_private_dir`, else leftovers of it are not recognised and stay).
pub const PREFIX_THUMB_IN: &str = "StepViewer-thumb-in-";
/// The name `dwg_` + six letters or digits that the folder next to the cache file had before it got a distinctive name and
/// a marker. Such a folder is still removed, but only in the cache folder of the program (`remove_stale_conversion_dirs_in`):
/// nothing else is kept there. In the temporary folder the name alone says nothing.
pub const LEGACY_CACHE_PREFIX: &str = "dwg_";
/// The file that marks a folder as made by this program.
pub const MARKER_FILE: &str = ".stepviewer-temp";
/// What the marker says (the cleanup reads it back).
const MARKER_TEXT: &str = "StepViewer temporary folder. The program made it and removes it; a leftover older than six hours is removed at a later start.\n";
/// The start of `MARKER_TEXT` that is checked.
const MARKER_START: &str = "StepViewer temporary folder.";
/// Characters that `tempfile` adds to a prefix.
const RANDOM_CHARS: usize = 6;
/// Folders older than this are leftovers of a program that is no longer running (a conversion takes minutes at the most:
/// `TIMEOUT`).
pub const STALE_AGE: Duration = Duration::from_secs(6 * 3600);

/// Makes a private folder `<prefix><six letters or digits>` in `base` (the temporary folder when `None`) with the marker in
/// it, and gives it back as a `TempDir`, which removes it when it goes. A folder whose marker cannot be written is removed
/// again and the error is returned.
pub fn create_private_dir(base: Option<&Path>, prefix: &str) -> std::io::Result<tempfile::TempDir> {
    let mut b = tempfile::Builder::new();
    b.prefix(prefix).rand_bytes(RANDOM_CHARS);
    let dir = match base {
        Some(base) => b.tempdir_in(base)?,
        None => b.tempdir()?,
    };
    mark_private_dir(dir.path())?; // a failure drops `dir`, which removes it
    Ok(dir)
}

/// Marks a folder that the caller made itself (`std::fs::create_dir`, with a name that starts with one of `STALE_PREFIXES`
/// and goes on with six letters or digits) as one of ours, so that a leftover of it is removed later.
pub fn mark_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join(MARKER_FILE), MARKER_TEXT)
}

/// Whether the folder holds the marker: a plain file (not a link) that says what the marker says.
fn has_marker(dir: &Path) -> bool {
    use std::io::Read;
    let file = dir.join(MARKER_FILE);
    let Ok(meta) = std::fs::symlink_metadata(&file) else { return false };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return false;
    }
    let mut head = Vec::new();
    std::fs::File::open(&file).and_then(|f| f.take(MARKER_START.len() as u64).read_to_end(&mut head)).is_ok() && head == MARKER_START.as_bytes()
}

/// `prefix` and then exactly six letters or digits.
fn has_shape(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|rest| rest.len() == RANDOM_CHARS && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Removes the leftover folders of this program in `dir` that are older than `max_age`, and returns how many. A folder is
/// one of ours when its name is one of `STALE_PREFIXES` and six letters or digits and it holds `MARKER_FILE`; a folder
/// that is not marked is never touched, whatever its name (the temporary folder is shared with other programs). Never a
/// link or a file; a folder that is in use or cannot be removed stays. Nothing here fails: this runs at the start of the
/// program.
pub fn remove_stale_conversion_dirs(dir: &Path, max_age: Duration) -> usize {
    remove_stale_in(dir, false, max_age)
}

/// `remove_stale_conversion_dirs` for several folders at once (the temporary folder, the cache folder, the folder of the
/// thumbnails, `LocalLow\StepViewerThumbs`...); a folder that is given twice is looked through once. `cache` is the cache folder
/// of the program (it is looked through too, when it is not in `dirs`): the only folder in which a folder `dwg_` + six
/// letters or digits without a marker, as the earlier versions made them, is removed as well (`LEGACY_CACHE_PREFIX`).
pub fn remove_stale_conversion_dirs_in(dirs: &[&Path], cache: Option<&Path>, max_age: Duration) -> usize {
    let mut done: Vec<&Path> = Vec::new();
    let mut removed = 0;
    for &d in dirs.iter().chain(cache.iter()) {
        if done.contains(&d) {
            continue;
        }
        done.push(d);
        removed += remove_stale_in(d, cache == Some(d), max_age);
    }
    removed
}

fn remove_stale_in(dir: &Path, legacy_names: bool, max_age: Duration) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = STALE_PREFIXES.iter().any(|p| has_shape(&name, p));
        let legacy = legacy_names && has_shape(&name, LEGACY_CACHE_PREFIX);
        if !ours && !legacy {
            continue;
        }
        let path = entry.path();
        // `symlink_metadata`: a link (or a junction) with such a name is not followed and not removed
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        if !meta.is_dir() || meta.file_type().is_symlink() {
            continue;
        }
        if ours && !has_marker(&path) {
            continue;
        }
        let age = meta.modified().ok().and_then(|m| now.duration_since(m).ok());
        if age.is_some_and(|a| a >= max_age) && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Result of `load_dwg`: the DXF bytes and the converter label.
pub struct Converted {
    pub dxf: PathBuf,
    pub converter: String,
    /// Temporary directory to remove after the DXF was read (None when the cache file is used).
    pub temp: Option<tempfile::TempDir>,
}

/// DWG -> DXF with the cache rules of the viewer: `dxf_cache` (an existing file is used as it is), a private
/// temporary directory next to the cache (else in the system temp folder) that receives the converter output, the
/// finished DXF is moved into the cache in one step.
pub fn convert_cached(src: &Path, dxf_cache: Option<&Path>, third_party: Option<&Path>, report: &mut dyn FnMut(&str)) -> Result<Converted, String> {
    convert_cached_cancellable(src, dxf_cache, third_party, report, None)
}

/// `convert_cached` that stops the converter and removes its temporary folder when `cancel` is set.
pub fn convert_cached_cancellable(
    src: &Path,
    dxf_cache: Option<&Path>,
    third_party: Option<&Path>,
    report: &mut dyn FnMut(&str),
    cancel: Option<&AtomicBool>,
) -> Result<Converted, String> {
    if let Some(c) = dxf_cache {
        if c.exists() {
            return Ok(Converted { dxf: c.to_path_buf(), converter: "mezipaměť".into(), temp: None });
        }
    }
    report("Převádím DWG na DXF…");
    let base = dxf_cache.and_then(|c| c.parent()).map(|p| p.to_path_buf());
    let dir = create_private_dir(base.as_deref(), PREFIX_DXF).map_err(|e| e.to_string())?;
    let tmp = dir.path().join("o.dxf");
    let conv = dwg_to_dxf_cancellable(src, &tmp, third_party, cancel)?;
    let dxf = match dxf_cache {
        Some(c) => {
            std::fs::rename(&tmp, c).map_err(|e| format!("Převod DWG selhal: {}", e))?;
            c.to_path_buf()
        }
        None => tmp,
    };
    Ok(Converted { dxf, converter: conv.to_string(), temp: Some(dir) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_cache_is_used_without_conversion() {
        let d = tempfile::tempdir().unwrap();
        let cache = d.path().join("c.dxf");
        std::fs::write(&cache, "0\nEOF\n").unwrap();
        let mut msgs = Vec::new();
        let c = convert_cached(Path::new("nonexistent.dwg"), Some(&cache), None, &mut |m| msgs.push(m.to_string())).unwrap();
        assert_eq!(c.converter, "mezipaměť");
        assert_eq!(c.dxf, cache);
        assert!(msgs.is_empty());
    }

    #[test]
    fn the_missing_converter_message_tells_about_the_dwg_add_on() {
        assert!(MISSING_CONVERTER.contains("doplňku pro DWG"), "{MISSING_CONVERTER}");
        assert!(MISSING_CONVERTER.contains("StepViewer-dwg-addon"), "{MISSING_CONVERTER}");
        assert!(MISSING_CONVERTER.contains("third_party"), "{MISSING_CONVERTER}");
        // it starts as the message always did
        assert!(MISSING_CONVERTER.starts_with("Chybí převodník DWG (dwg2dxf z LibreDWG, případně ODA File Converter)."));
    }

    fn make_old(path: &Path, age: Duration) {
        let t = std::time::SystemTime::now() - age;
        let mut o = std::fs::OpenOptions::new();
        o.write(true);
        #[cfg(windows)]
        {
            // a folder is opened for its attributes with the backup semantics flag
            use std::os::windows::fs::OpenOptionsExt;
            o.custom_flags(0x0200_0000);
        }
        let f = o.open(path).expect("open the folder");
        f.set_modified(t).expect("set the time");
    }

    /// A folder as the program makes it (`name` and the marker), older than `age`, with a file in it.
    fn ours(root: &Path, name: &str, age: Duration) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("in.dwg"), b"x").unwrap();
        mark_private_dir(&d).unwrap();
        make_old(&d, age);
        d
    }

    /// A folder of somebody else with files in it (no marker), older than `age`.
    fn foreign(root: &Path, name: &str, age: Duration) -> PathBuf {
        let d = root.join(name);
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("in.dwg"), b"x").unwrap();
        std::fs::write(d.join("keep.txt"), b"mine").unwrap();
        make_old(&d, age);
        d
    }

    fn hours(n: u64) -> Duration {
        Duration::from_secs(n * 3600)
    }

    #[test]
    fn leftover_conversion_folders_older_than_six_hours_are_removed_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let t = temp.path();
        let stale_in = ours(t, "StepViewer-dwg-in-aB3x9Z", hours(7));
        let stale_oda = ours(t, "StepViewer-oda-in-000000", hours(30));
        let stale_out = ours(t, "StepViewer-oda-out-zzzzzz", hours(8));
        let stale_cache = ours(t, "StepViewer-dxf-Q1w2E3", hours(100));
        let stale_thumb = ours(t, "StepViewer-thumb-in-00ff7a", hours(9));
        let young = ours(t, "StepViewer-dwg-in-AAAAAA", hours(5));
        // names that are not ours, however old: a longer name, other characters, other prefixes, a name that is not distinctive
        let other = ours(t, "StepViewer-dwg-in-AAAAAAA", hours(100));
        let other2 = ours(t, "my-StepViewer-dwg-in-AAAAAA", hours(100));
        let other3 = ours(t, "StepViewer-dwg-in-AB-3x9", hours(100));
        let other4 = ours(t, "Temp", hours(100));
        // a file with such a name is not a folder
        std::fs::write(t.join("StepViewer-dwg-in-FILE12"), b"x").unwrap();
        make_old(&t.join("StepViewer-dwg-in-FILE12"), hours(100));
        let n = remove_stale_conversion_dirs(t, STALE_AGE);
        assert_eq!(n, 5);
        for gone in [&stale_in, &stale_oda, &stale_out, &stale_cache, &stale_thumb] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        for kept in [&young, &other, &other2, &other3, &other4, &t.join("StepViewer-dwg-in-FILE12")] {
            assert!(kept.exists(), "{}", kept.display());
        }
        // run again: nothing to do; a folder that does not exist is no error
        assert_eq!(remove_stale_conversion_dirs(t, STALE_AGE), 0);
        assert_eq!(remove_stale_conversion_dirs(&t.join("none"), STALE_AGE), 0);
    }

    #[test]
    fn folders_of_other_programs_are_never_removed_from_the_temporary_folder() {
        // 30 hours old folders of somebody else that look like ours to the old rule (a prefix and six characters), with files
        let temp = tempfile::tempdir().unwrap();
        let t = temp.path();
        let others: Vec<PathBuf> = ["dwg_backup", "dwg_export", "dwg_in_backup", "oda_in_export", "oda_out_backup", "thumb_in_abcdef", "dwg_Q1w2E3"]
            .iter()
            .map(|n| foreign(t, n, hours(30)))
            .collect();
        // and folders that have the distinctive name but not the marker: nobody made them but us, and still nothing proves it
        let unmarked: Vec<PathBuf> = ["StepViewer-dwg-in-backup", "StepViewer-dxf-export", "StepViewer-thumb-in-abcdef"].iter().map(|n| foreign(t, n, hours(30))).collect();
        // a marker that says something else, a marker that is a folder
        let wrong = foreign(t, "StepViewer-dwg-in-wrong1", hours(30));
        std::fs::write(wrong.join(MARKER_FILE), b"something else\n").unwrap();
        make_old(&wrong, hours(30));
        let dirmarker = foreign(t, "StepViewer-dwg-in-dirmk1", hours(30));
        std::fs::create_dir(dirmarker.join(MARKER_FILE)).unwrap();
        make_old(&dirmarker, hours(30));
        let cache = t.join("cache");
        std::fs::create_dir(&cache).unwrap();
        assert_eq!(remove_stale_conversion_dirs(t, STALE_AGE), 0);
        // the list form, with another folder as the cache folder: the temporary folder is not the cache folder, so even the
        // `dwg_` names stay there
        assert_eq!(remove_stale_conversion_dirs_in(&[t, &cache], Some(&cache), STALE_AGE), 0);
        for kept in others.iter().chain(&unmarked).chain([&wrong, &dirmarker]) {
            assert!(kept.exists(), "{}", kept.display());
            assert!(kept.join("keep.txt").exists() || kept.join("in.dwg").exists());
        }
    }

    #[test]
    fn the_cache_folder_also_loses_the_dwg_folders_that_earlier_versions_left_there() {
        let root = tempfile::tempdir().unwrap();
        let (temp, cache) = (root.path().join("temp"), root.path().join("cache"));
        std::fs::create_dir(&temp).unwrap();
        std::fs::create_dir(&cache).unwrap();
        let in_cache = foreign(&cache, "dwg_Q1w2E3", hours(100));
        let in_cache_young = foreign(&cache, "dwg_young1", hours(1));
        let in_cache_other = foreign(&cache, "dwg_backups", hours(100)); // seven characters: not the shape
        let in_temp = foreign(&temp, "dwg_Q1w2E3", hours(100));
        let marked_in_cache = ours(&cache, "StepViewer-dxf-abc123", hours(100));
        let marked_in_temp = ours(&temp, "StepViewer-dwg-in-abc123", hours(100));
        // the cache folder is given in the list as well: it is looked through once
        let n = remove_stale_conversion_dirs_in(&[&temp, &cache], Some(&cache), STALE_AGE);
        assert_eq!(n, 3);
        for gone in [&in_cache, &marked_in_cache, &marked_in_temp] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        for kept in [&in_cache_young, &in_cache_other, &in_temp] {
            assert!(kept.exists(), "{}", kept.display());
        }
        // the cache folder is looked through even when the list does not have it, and an empty list is no error
        let again = foreign(&cache, "dwg_Zz9Yy8", hours(100));
        assert_eq!(remove_stale_conversion_dirs_in(&[], Some(&cache), STALE_AGE), 1);
        assert!(!again.exists());
        assert_eq!(remove_stale_conversion_dirs_in(&[], None, STALE_AGE), 0);
        // the plain function knows no cache folder: no `dwg_` name, with or without a marker
        let plain = foreign(&cache, "dwg_Aa1Bb2", hours(100));
        assert_eq!(remove_stale_conversion_dirs(&cache, STALE_AGE), 0);
        assert!(plain.exists());
    }

    #[test]
    fn the_folders_the_program_makes_have_a_distinctive_name_and_the_marker() {
        let root = tempfile::tempdir().unwrap();
        for prefix in STALE_PREFIXES {
            let d = create_private_dir(Some(root.path()), prefix).unwrap();
            let name = d.path().file_name().unwrap().to_string_lossy().into_owned();
            assert!(has_shape(&name, prefix), "{name}");
            assert!(has_marker(d.path()), "{name} has the marker");
            assert!(prefix.starts_with("StepViewer-"), "{prefix}");
            // made old, it is recognised as a leftover by the cleanup
            make_old(d.path(), hours(7));
            let path = d.path().to_path_buf();
            assert_eq!(remove_stale_conversion_dirs(root.path(), STALE_AGE), 1);
            assert!(!path.exists());
            // (the TempDir finds the folder gone when it is dropped: that is no error either)
            drop(d);
        }
        // in the temporary folder by default
        let d = create_private_dir(None, PREFIX_DWG_IN).unwrap();
        assert!(d.path().starts_with(std::env::temp_dir()) && has_marker(d.path()));
    }

    #[test]
    fn a_staged_conversion_works_in_a_marked_folder_and_leaves_none_behind() {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("a.dwg");
        std::fs::write(&src, b"DWGDATA").unwrap();
        let seen = std::sync::Mutex::new((String::new(), false));
        let exe = std::env::current_exe().unwrap();
        let make = |work: &Path| {
            *seen.lock().unwrap() = (work.file_name().unwrap().to_string_lossy().into_owned(), has_marker(work));
            let mut c = Command::new(&exe);
            c.current_dir(work).args(["--exact", "dwg::tests::fake_converter_for_the_staging_test", "--nocapture", "--test-threads=1"]).env("CAD2D_TEST_FAKE_CONVERTER", "ok");
            c
        };
        convert_staged(&src, &root.path().join("o.dxf"), None, &make).expect("conversion");
        let (name, marked) = seen.lock().unwrap().clone();
        assert!(has_shape(&name, PREFIX_DWG_IN), "{name}");
        assert!(marked, "the marker is there while the converter runs");
        // the converted file goes through a marked folder next to the cache file, which is gone when the load is done
        let cache = root.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        let flag = AtomicBool::new(true);
        let e = convert_cached_cancellable(&src, Some(&cache.join("k.dxf")), None, &mut |_| {}, Some(&flag)).err().expect("cancelled");
        // (cancelled, or no converter on this computer: either way the folder made for the conversion is gone again)
        assert!(e == CANCELLED || e.starts_with("Chybí převodník"), "{e}");
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0, "no folder is left in the cache folder");
    }

    #[test]
    fn a_link_with_the_name_of_a_conversion_folder_is_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let t = temp.path();
        let precious = t.join("precious");
        std::fs::create_dir(&precious).unwrap();
        std::fs::write(precious.join("keep.txt"), b"keep").unwrap();
        // even with the marker in the target, a link is not a folder of ours
        mark_private_dir(&precious).unwrap();
        // a directory link (junction / symbolic link): creating one may be refused without the right, then there is nothing to test
        let link = t.join("StepViewer-dwg-in-LINK12");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&precious, &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&precious, &link).is_ok();
        if !made {
            eprintln!("SKIP: no right to make a link");
            return;
        }
        make_old(&precious, Duration::from_secs(100 * 3600));
        remove_stale_conversion_dirs(t, Duration::from_secs(0));
        assert!(precious.join("keep.txt").exists(), "the target of the link is untouched");
    }

    #[test]
    fn missing_converter_message() {
        // no third_party folder given and none next to the test executable: the converter lookup fails (or an
        // installed ODA converter is used, which then fails on the missing input file)
        if find_oda().is_some() || find_dwg2dxf(None).is_some() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let e = dwg_to_dxf(Path::new("x.dwg"), &d.path().join("o.dxf"), None).unwrap_err();
        assert_eq!(e, MISSING_CONVERTER);
    }

    fn plant_converter(root: &Path) -> PathBuf {
        let sub = if cfg!(windows) { "libredwg" } else { "libredwg-linux" };
        let file = if cfg!(windows) { "dwg2dxf.exe" } else { "dwg2dxf" };
        let dir = root.join("third_party").join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(file);
        std::fs::write(&p, b"not a program").unwrap();
        p
    }

    #[test]
    fn release_builds_search_only_the_folder_of_the_executable() {
        let exe_dir = Path::new("app").join("bin");
        let release = converter_dirs(None, Some(&exe_dir), false);
        assert_eq!(release, vec![exe_dir.join("third_party")]);
        // the folder given by the caller comes first
        let given = Path::new("given");
        assert_eq!(converter_dirs(Some(given), Some(&exe_dir), false), vec![given.to_path_buf(), exe_dir.join("third_party")]);
        // a build tree (debug builds) also looks at the parent and the grandparent folder
        let debug = converter_dirs(None, Some(&exe_dir), true);
        assert_eq!(debug.len(), 3);
        assert!(debug.iter().any(|d| d.components().any(|c| c == std::path::Component::ParentDir)));
        assert!(release.iter().all(|d| d.components().all(|c| c != std::path::Component::ParentDir)));
    }

    #[test]
    fn a_converter_planted_in_a_parent_folder_is_not_run_by_release_builds() {
        let root = tempfile::tempdir().unwrap();
        let planted = plant_converter(root.path());
        let exe_dir = root.path().join("bin");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let release = find_dwg2dxf_in(&converter_dirs(None, Some(&exe_dir), false));
        assert_ne!(release.as_deref(), Some(planted.as_path()), "release search must not leave the folder of the executable");
        // the same layout is found when the build tree search is on (debug builds), so the test proves the difference
        let debug = find_dwg2dxf_in(&converter_dirs(None, Some(&exe_dir), true)).expect("debug search finds the planted file");
        assert_eq!(debug.canonicalize().unwrap(), planted.canonicalize().unwrap());
        // and a converter inside <exe dir>/third_party is found by both
        let own = plant_converter(&exe_dir);
        assert_eq!(find_dwg2dxf_in(&converter_dirs(None, Some(&exe_dir), false)).as_deref(), Some(own.as_path()));
    }

    #[test]
    fn file_names_that_look_like_options_are_made_safe() {
        for name in ["-v9.dwg", "--help.dwg", "-o", "-"] {
            let src = Path::new(name);
            let safe = safe_arg_path(src);
            assert!(!safe.to_string_lossy().starts_with('-'), "{name} -> {safe:?}");
            assert!(safe.ends_with(name));
        }
        // paths that are fine stay as they are
        assert_eq!(safe_arg_path(Path::new("a.dwg")), PathBuf::from("a.dwg"));
        let abs = std::env::current_dir().unwrap().join("-v9.dwg");
        assert_eq!(safe_arg_path(&abs), abs);
        assert_eq!(safe_arg_path(&Path::new("dir").join("-x.dwg")), Path::new("dir").join("-x.dwg"));
    }

    #[test]
    fn libredwg_gets_plain_short_ascii_names_in_its_own_folder() {
        // whatever the drawing is called (or however deep it lies), the converter command has only these arguments and runs
        // in the private folder, so neither the code page of the system nor the 260 characters of a path matter
        let work = Path::new("C:\\Temp\\dwg_in_x");
        let cmd = libredwg_command(Path::new("dwg2dxf.exe"), work);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
        assert_eq!(args, ["-v0", "-y", "-o", STAGED_OUTPUT, STAGED_INPUT]);
        assert!(args.iter().all(|a| a.is_ascii() && a.len() < 16));
        assert_eq!(cmd.get_current_dir(), Some(work));
    }

    /// Helper process of `a_drawing_with_an_unusual_path_is_converted_under_a_plain_name`: it plays the converter. It
    /// works only when the test starts it with the variable set; the test executable then looks at its own arguments.
    #[test]
    fn fake_converter_for_the_staging_test() {
        let Some(mode) = std::env::var_os("CAD2D_TEST_FAKE_CONVERTER") else { return };
        // the converter sees the staged names and nothing of the original path
        let input = std::fs::read(STAGED_INPUT).expect("the staged input is in the working folder");
        assert!(mode == "ok");
        std::fs::write(STAGED_OUTPUT, [b"0\nEOF\n".as_slice(), &input].concat()).expect("write the staged output");
    }

    #[test]
    fn a_drawing_with_an_unusual_path_is_converted_under_a_plain_name() {
        let root = tempfile::tempdir().unwrap();
        // a name outside every ANSI code page and a path over 260 characters
        let mut dir = root.path().join("složka Ж 图纸");
        while dir.to_string_lossy().chars().count() < 300 {
            dir = dir.join("hluboká-složka-ěščřžýáíé");
        }
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Výkres ěščřžýáíé — kopie (1) 图纸.dwg");
        std::fs::write(&src, b"DWGDATA").unwrap();
        let dst = root.path().join("out.dxf");
        let exe = std::env::current_exe().unwrap();
        let staged = std::sync::Mutex::new(PathBuf::new());
        let make = |work: &Path| {
            *staged.lock().unwrap() = work.to_path_buf();
            // the test executable is the converter: the same folder and names as the real command, run as a test that
            // writes `out.dxf` from `in.dwg` (the real arguments are not for the test harness, so they are left out)
            let mut c = Command::new(&exe);
            c.current_dir(work).args(["--exact", "dwg::tests::fake_converter_for_the_staging_test", "--nocapture", "--test-threads=1"]).env("CAD2D_TEST_FAKE_CONVERTER", "ok");
            c
        };
        convert_staged(&src, &dst, None, &make).expect("conversion");
        let got = std::fs::read(&dst).unwrap();
        assert!(got.ends_with(b"DWGDATA"), "{:?}", String::from_utf8_lossy(&got));
        // the staging folder has plain names and is gone
        let work = staged.lock().unwrap().clone();
        assert!(work.to_string_lossy().is_ascii() || work.starts_with(std::env::temp_dir()));
        assert!(!work.exists(), "{work:?}");
        // a converter that writes nothing is reported
        let none = move |work: &Path| {
            let mut c = Command::new(std::env::current_exe().unwrap());
            c.current_dir(work).args(["--exact", "dwg::tests::this_test_does_not_exist", "--test-threads=1"]);
            c
        };
        let e = convert_staged(&src, &root.path().join("none.dxf"), None, &none).unwrap_err();
        assert!(e.starts_with("Převod DWG selhal"), "{e}");
    }

    /// Helper process of `cancelling_stops_the_converter_process`: the test executable run with a variable set sleeps.
    #[test]
    fn sleeper_for_the_cancel_test() {
        if std::env::var_os("CAD2D_TEST_SLEEPER").is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[test]
    fn cancelling_stops_the_converter_process() {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "dwg::tests::sleeper_for_the_cancel_test", "--nocapture", "--test-threads=1"]).env("CAD2D_TEST_SLEEPER", "1");
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let setter = {
            let flag = flag.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                flag.store(true, Ordering::Relaxed);
            })
        };
        let t = Instant::now();
        let r = run_cancellable(cmd, Duration::from_secs(120), Some(&flag));
        setter.join().unwrap();
        assert_eq!(r.unwrap_err(), CANCELLED);
        assert!(t.elapsed() < Duration::from_secs(20), "the child was waited for: {:?}", t.elapsed());
        // a flag that is already set does not even start the process
        let t = Instant::now();
        assert_eq!(run_cancellable(Command::new(std::env::current_exe().unwrap()), Duration::from_secs(120), Some(&flag)).unwrap_err(), CANCELLED);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[cfg(windows)]
    #[test]
    fn closing_the_job_ends_the_converter_that_is_in_it() {
        // the sleeper would run for a minute; the job's kill-on-close ends it as soon as the job is closed
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "dwg::tests::sleeper_for_the_cancel_test", "--nocapture", "--test-threads=1"])
            .env("CAD2D_TEST_SLEEPER", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let held = job::Job::holding(&child);
        assert!(held.is_some(), "the system takes the child into a job");
        drop(held);
        // the sleeper only finishes after its 60 s sleep, so an end before that is the job's doing (the exit code the
        // system gives a process ended this way is not asserted: it is not the sleeper's own)
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                assert!(started.elapsed() < Duration::from_secs(60), "the child ended on its own: {:?}", started.elapsed());
                break;
            }
            if started.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                panic!("the converter outlived its job");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// A reader that gives `total` bytes of numbered lines and then ends; counts what it gave.
    struct Lines {
        total: usize,
        given: usize,
        n: usize,
        pending: Vec<u8>,
    }

    impl std::io::Read for Lines {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            if self.pending.is_empty() {
                if self.given >= self.total {
                    return Ok(0);
                }
                self.n += 1;
                self.pending = format!("line {}\n", self.n).into_bytes();
            }
            let k = self.pending.len().min(out.len());
            out[..k].copy_from_slice(&self.pending[..k]);
            self.pending.drain(..k);
            self.given += k;
            Ok(k)
        }
    }

    #[test]
    fn the_error_output_is_kept_only_as_a_bounded_tail() {
        // 200 MB of lines, then the end: only the end is kept, however much was read
        let total = 200_000_000;
        let src = Lines { total, given: 0, n: 0, pending: Vec::new() };
        let tail = std::sync::Mutex::new(Vec::new());
        let mut peak = 0usize;
        struct Watch<'a>(Lines, &'a std::sync::Mutex<Vec<u8>>, &'a mut usize);
        impl std::io::Read for Watch<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                *self.2 = (*self.2).max(self.1.lock().unwrap().len());
                self.0.read(out)
            }
        }
        drain_tail(&mut Watch(src, &tail, &mut peak), STDERR_TAIL, &tail);
        let kept = tail.lock().unwrap().clone();
        assert!(peak <= 2 * STDERR_TAIL, "the buffer grew to {peak} bytes");
        assert!(kept.len() >= STDERR_TAIL && kept.len() <= 2 * STDERR_TAIL, "{}", kept.len());
        // what is kept is the end of the input: its last line is whole
        let text = String::from_utf8_lossy(&kept);
        let last = text.trim_end().lines().last().unwrap();
        assert!(last.starts_with("line ") && last[5..].parse::<usize>().is_ok(), "{last}");
        // a short output is kept whole
        let short = std::sync::Mutex::new(Vec::new());
        drain_tail(&mut std::io::Cursor::new(b"a\nb\nlast error\n".to_vec()), STDERR_TAIL, &short);
        assert_eq!(short.into_inner().unwrap(), b"a\nb\nlast error\n");
    }

    /// Helper process of the error output tests: the test executable run with `CAD2D_TEST_FLOOD` writes 64 MiB of lines
    /// to its standard error and then a last line.
    #[test]
    fn flooder_for_the_stderr_test() {
        if std::env::var_os("CAD2D_TEST_FLOOD").is_some() {
            use std::io::Write;
            let block: String = (0..2048).map(|i| format!("converter noise {i:08}\n")).collect();
            let mut e = std::io::stderr().lock();
            for _ in 0..(64 << 20) / block.len() {
                let _ = e.write_all(block.as_bytes());
            }
            let _ = e.write_all(b"the very last error line\n");
        }
    }

    #[test]
    fn a_flood_of_error_output_comes_back_as_its_tail() {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "dwg::tests::flooder_for_the_stderr_test", "--nocapture", "--test-threads=1"]).env("CAD2D_TEST_FLOOD", "1");
        let (code, err) = run_cancellable(cmd, Duration::from_secs(120), None).unwrap();
        assert_eq!(code, Some(0));
        assert!(err.len() <= STDERR_TAIL, "{} bytes of error output were kept", err.len());
        assert_eq!(String::from_utf8_lossy(&err).trim().lines().last(), Some("the very last error line"));
    }

    /// Helper process of the pipe tests: the test executable run with `CAD2D_TEST_GRANDPARENT` starts a sleeper that
    /// inherits its standard error (so it holds the pipe of the caller), and then ends (`exit`) or stays (`stay`).
    #[test]
    fn grandparent_for_the_pipe_test() {
        if let Some(mode) = std::env::var_os("CAD2D_TEST_GRANDPARENT") {
            let mut c = Command::new(std::env::current_exe().unwrap());
            c.args(["--exact", "dwg::tests::sleeper_for_the_cancel_test", "--nocapture", "--test-threads=1"])
                .env("CAD2D_TEST_SLEEPER", "1")
                .env_remove("CAD2D_TEST_GRANDPARENT")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            let _grandchild = c.spawn().expect("start the grandchild");
            if mode == "stay" {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
    }

    fn grandparent(mode: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", "dwg::tests::grandparent_for_the_pipe_test", "--nocapture", "--test-threads=1"]).env("CAD2D_TEST_GRANDPARENT", mode);
        cmd
    }

    // Under a job object (Windows) the converter's whole tree ends with the job, so the pipe is closed at once; the 15 s of
    // `READER_GRACE` (5 s outside the tests) are only the way out for a system that refused the job. The time limit of these
    // tests is below it, and far above what a loaded computer needs to start two test processes.
    #[cfg(windows)]
    #[test]
    fn a_grandchild_that_holds_the_pipe_does_not_keep_a_finished_conversion_waiting() {
        let t = Instant::now();
        let (code, _) = run_cancellable(grandparent("exit"), Duration::from_secs(120), None).unwrap();
        assert_eq!(code, Some(0));
        assert!(t.elapsed() < Duration::from_secs(10), "the reader was waited for: {:?}", t.elapsed());
    }

    #[cfg(windows)]
    #[test]
    fn a_grandchild_that_holds_the_pipe_does_not_keep_a_timed_out_conversion_waiting() {
        let t = Instant::now();
        let e = run_cancellable(grandparent("stay"), Duration::from_millis(1500), None).unwrap_err();
        assert_eq!(e, "Převod DWG překročil časový limit.");
        assert!(t.elapsed() < Duration::from_secs(10), "the reader was waited for: {:?}", t.elapsed());
    }

    #[cfg(windows)]
    #[test]
    fn a_grandchild_that_holds_the_pipe_does_not_keep_a_cancelled_conversion_waiting() {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let setter = {
            let flag = flag.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                flag.store(true, Ordering::Relaxed);
            })
        };
        let t = Instant::now();
        let e = run_cancellable(grandparent("stay"), Duration::from_secs(120), Some(&flag)).unwrap_err();
        setter.join().unwrap();
        assert_eq!(e, CANCELLED);
        assert!(t.elapsed() < Duration::from_secs(10), "the reader was waited for: {:?}", t.elapsed());
    }

    #[test]
    fn a_reader_that_never_sees_the_end_of_the_pipe_is_given_up_on() {
        // something outside any job holds the pipe: finish returns what was read once the grace is over
        struct Stuck(Option<Vec<u8>>);
        impl std::io::Read for Stuck {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                match self.0.take() {
                    Some(b) => {
                        out[..b.len()].copy_from_slice(&b);
                        Ok(b.len())
                    }
                    None => {
                        std::thread::sleep(Duration::from_secs(30));
                        Ok(0)
                    }
                }
            }
        }
        let r = StderrTail::start(Some(Stuck(Some(b"half of the output\n".to_vec()))));
        let t = Instant::now();
        let got = r.finish(Duration::from_millis(300));
        assert_eq!(got, b"half of the output\n");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    #[test]
    fn a_cancelled_load_returns_cancelled_and_leaves_no_temporary_folder() {
        let dir = tempfile::tempdir().unwrap();
        let dwg = dir.path().join("a.dwg");
        std::fs::write(&dwg, b"AC1027 not really a drawing").unwrap();
        let flag = AtomicBool::new(true);
        let e = crate::load_cancellable(&dwg, &crate::LoadOptions::default(), &mut |_| {}, Some(&flag)).err().expect("cancelled");
        assert!(matches!(e, crate::LoadError::Cancelled), "{e}");
        assert!(std::fs::read_dir(dir.path()).unwrap().flatten().all(|f| !f.file_name().to_string_lossy().starts_with("dwg_") && !f.file_name().to_string_lossy().starts_with("StepViewer-")));
    }

    #[test]
    fn bad_input_gives_conversion_error() {
        let Some(exe) = find_dwg2dxf(Some(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../third_party"))) else { return };
        let _ = exe;
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("bad.dwg");
        std::fs::write(&src, b"not a dwg file at all").unwrap();
        let tp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../third_party");
        let r = dwg_to_dxf(&src, &d.path().join("o.dxf"), Some(&tp));
        if find_oda().is_none() {
            let e = r.unwrap_err();
            assert!(e.starts_with("Převod DWG selhal"), "{e}");
        }
    }
}
