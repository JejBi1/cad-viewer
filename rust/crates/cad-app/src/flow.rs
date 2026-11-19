//! Loading flow (`open_file` and everything it starts): cache, load worker process, warm worker, in-process load,
//! drawing loader, progress and cancel. Results of background threads come back as [`Event`]s.
//!
//! Rules (ARCHITECTURE.md, "Portable variant"): the source file is only read; in portable mode it is copied into the
//! local cache folder first (`paths::source_copy_if`; the copy keeps the file name of the source, so the tree and the
//! material assignments see the same names as outside portable mode), while the cache key keeps using the original
//! path, size and mtime. Nothing is written next to the source or into the application folder. `cache::prune` runs only between loads.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cad_2d::{Drawing, FontOptions, LoadOptions};
use cad_core::cache;
use cad_core::materials::Assignments;
use cad_core::paths;
use cad_core::worker::{self, WarmWorker};
use cad_types::Scene2d;
use eframe::egui;
use occt_bridge::{BridgeError, Document};

use crate::app::{App, Mode};
use crate::drawing2::Draw2;
use crate::model3::{Loaded3d, Model3, StlMesh};
use crate::scene2d::SceneSpace;
use crate::util::{base_name, is_drawing, secs1, spawn_big, third_party_dir};

/// Files from this size are loaded in a worker process (`WORKER_THRESHOLD`; env `STEPVIEW_WORKER_MIN`).
pub fn worker_threshold() -> u64 {
    std::env::var("STEPVIEW_WORKER_MIN").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(cache::CACHE_MIN)
}

/// Poll period of the worker process (`self._poll.start(150)`).
pub const POLL: Duration = Duration::from_millis(150);

/// A model could not be put on screen.
#[derive(Debug)]
pub enum LoadFail {
    /// Cyclic or too big assembly: a warning, nothing is wrong with the file as such.
    Complex(String),
    Failed(String),
}

/// What the drawing loader hands over.
pub struct Loaded2d {
    pub drawing: Arc<Drawing>,
    pub layout: String,
    pub scene: Result<(Scene2d, SceneSpace), String>,
    pub from_cache: bool,
}

pub enum Event {
    Progress { token: u64, text: String },
    /// The local copy of the source (portable mode) is ready.
    Source { token: u64, result: Result<PathBuf, String> },
    Model { token: u64, result: Result<Box<Loaded3d>, LoadFail> },
    CacheBroken { token: u64 },
    Drawing { token: u64, result: Result<Box<Loaded2d>, String> },
    /// A scene of the open drawing (layout, background or layer change).
    Scene { generation: u64, keep_view: bool, result: Result<(Scene2d, SceneSpace), String> },
    Stl { generation: u64, result: Option<Box<StlMesh>> },
    Collision(crate::collide::CollisionEvent),
    Drive(crate::drive::DriveEvent),
}

/// A running worker process.
pub struct WorkerRun {
    pub child: std::process::Child,
    pub job: PathBuf,
    /// `<cache>.tmp.xbf` and its `.log`.
    pub tmp: PathBuf,
    pub cache: PathBuf,
}

/// A pre-started idle worker, and the file whose job it already got (`self._warm`).
pub struct WarmSlot {
    pub worker: WarmWorker,
    pub submitted: Option<PathBuf>,
}

pub struct Flow {
    /// Generation of the current load: results of older ones are dropped.
    pub token: u64,
    pub busy: bool,
    pub loading_path: PathBuf,
    pub display_name: Option<String>,
    pub t0: Instant,
    pub from_cache: bool,
    pub run: Option<WorkerRun>,
    pub warm: Option<WarmSlot>,
    pub next_poll: Instant,
    /// Original path, size and cache name while the portable copy is made.
    pub pending: Option<(PathBuf, u64, PathBuf)>,
    /// Cache file to write after an in-process read of a model of a megabyte or more.
    pub store_after_show: Option<PathBuf>,
    /// The local copy the model is read from when it is not the file that was opened (portable mode, a path that is too
    /// long): the tree is named after the original again.
    pub source_copy: Option<PathBuf>,
    /// A thread is working for the current token.
    pub thread_busy: bool,
    /// Stops the DWG converter and the first scene build of the drawing load in progress.
    pub drawing_cancel: Option<Arc<AtomicBool>>,
    /// Stops the scene build of `render_drawing` that is running.
    pub scene_cancel: Option<Arc<AtomicBool>>,
    /// Answers "the drawing thread has ended": the sender lives in that thread, so the receiver sees a disconnect when
    /// it is done. The exit waits on it for a short time after the converter was told to stop.
    pub drawing_done: Option<Receiver<()>>,
    pub scene_generation: u64,
    pub model_generation: u64,
    /// Drawing layout / background rebuild in progress.
    pub scene_busy: bool,
    /// Interference checks started so far (all dialogs): the number of a check, so an answer is never taken for
    /// another one.
    pub collision_runs: u64,
}

impl Flow {
    pub fn new() -> Flow {
        Flow {
            token: 0,
            busy: false,
            loading_path: PathBuf::new(),
            display_name: None,
            t0: Instant::now(),
            from_cache: false,
            run: None,
            warm: None,
            next_poll: Instant::now(),
            pending: None,
            store_after_show: None,
            source_copy: None,
            thread_busy: false,
            drawing_cancel: None,
            scene_cancel: None,
            drawing_done: None,
            scene_generation: 0,
            model_generation: 0,
            scene_busy: false,
            collision_runs: 0,
        }
    }

    /// A load is running (worker process, thread, drive download): `prune` must wait.
    pub fn active(&self) -> bool {
        self.busy || self.run.is_some() || self.thread_busy || self.scene_busy
    }

    pub fn warm_ok(&mut self) -> bool {
        match &mut self.warm {
            Some(w) => w.submitted.is_none() && w.worker.usable(),
            None => false,
        }
    }
}

impl Default for Flow {
    fn default() -> Self {
        Flow::new()
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    paths::normcase_str(&a.to_string_lossy()) == paths::normcase_str(&b.to_string_lossy())
}

/// Model from the XBF cache of the document or from a fresh read, ready to be shown.
pub(crate) fn prepare_model(doc: Document, from_cache: bool) -> Result<Box<Loaded3d>, LoadFail> {
    let tree = match doc.tree() {
        Ok(t) => t,
        Err(BridgeError::TooComplex(m)) => return Err(LoadFail::Complex(m)),
        Err(e) => return Err(LoadFail::Failed(e.message().to_string())),
    };
    let protos = (0..doc.proto_count()).map(|i| doc.proto_mesh(i)).collect();
    Ok(Box::new(Loaded3d { doc, tree, protos, from_cache }))
}

fn touch(p: &Path) {
    if let Ok(f) = fs::OpenOptions::new().write(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Paths of this many characters or more are beyond the 259 characters that Win32 programs without the long-path
/// setting (the application manifest) can use.
const LONG_PATH_CHARS: usize = 260;

/// Characters of a path as Windows counts them (UTF-16 units), which is what its limit of 260 is about.
pub fn path_chars(p: &Path) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().count()
    }
    #[cfg(not(windows))]
    {
        p.as_os_str().to_string_lossy().chars().count()
    }
}

/// Options of the load of the drawing `path` that the user opened. The file that is loaded may be a local copy of it (portable
/// mode copies it from the Drive first, `paths::source_copy_if`): the pictures of its IMAGE entities lie next to the original,
/// so the folder of the original is what they are looked up in.
pub fn drawing_load_options(path: &Path, dxf_cache: Option<PathBuf>) -> LoadOptions {
    LoadOptions {
        dxf_cache,
        third_party_dir: third_party_dir(),
        fonts: FontOptions::default(),
        original_dir: path.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf),
    }
}

/// A 3D model whose path is too long for Open CASCADE (it reads through the old path interface of Windows, 259 characters
/// at the most, whatever the manifest says) is read from a copy in the local cache folder, as in portable mode. Only
/// Windows has this limit.
pub fn needs_local_copy(p: &Path) -> bool {
    cfg!(windows) && path_chars(p) >= LONG_PATH_CHARS
}

/// The tree of a model that was read from a copy of the source under another name: the nodes named after the copy are
/// named after the file that was opened (an STL is named after its file, and the copy of a file with a long name is called
/// `<key>.stl`).
pub fn name_after_source(tree: &mut cad_types::ModelTree, copy: &Path, original: &Path) {
    let (Some(copy_stem), Some(orig_stem)) = (copy.file_stem().and_then(|s| s.to_str()), original.file_stem().and_then(|s| s.to_str())) else { return };
    if copy_stem == orig_stem {
        return;
    }
    for n in tree.nodes.iter_mut() {
        if n.name == copy_stem {
            n.name = orig_stem.to_string();
        }
        if n.proto_name == copy_stem {
            n.proto_name = orig_stem.to_string();
        }
    }
}

/// The text of a model that could not be read. OCCT reads through the old path interface: a file whose path is too
/// long for it fails the same way as a damaged file, so a long path is named as a possible cause.
pub fn load_failure_text(path: &Path, reason: &str) -> String {
    let mut text = format!("Nepodařilo se načíst model:\n{reason}");
    let len = path_chars(path);
    if len >= LONG_PATH_CHARS {
        text.push_str(&format!("\nCesta k souboru má {len} znaků; dlouhá cesta k souboru nemusí jít přečíst."));
    }
    text
}

impl App {
    // ------------------------------------------------------------------ helpers

    pub fn send(tx: &Sender<Event>, ctx: &egui::Context, ev: Event) {
        let _ = tx.send(ev);
        ctx.request_repaint();
    }

    pub fn set_busy(&mut self, on: bool, text: impl Into<String>) {
        self.flow.busy = on;
        if on {
            self.status.model = text.into();
            self.status.temp = None;
        }
    }

    pub fn add_recent(&mut self, path: &Path) {
        self.settings.add_recent(&path.to_string_lossy());
        self.save_settings();
    }

    /// Runs `f` on a big-stack thread; it gets the event channel and the context.
    fn spawn_task(&mut self, name: &str, f: impl FnOnce(Sender<Event>, egui::Context) + Send + 'static) -> bool {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.flow.thread_busy = true;
        match spawn_big(name, move || f(tx, ctx)) {
            Ok(_) => true,
            Err(e) => {
                self.flow.thread_busy = false;
                self.set_busy(false, "");
                self.critical(format!("Nepodařilo se spustit načítání:\n{e}"));
                false
            }
        }
    }

    /// Folder of the local copies of the sources (portable mode): `sources` in the cache folder.
    pub fn sources_dir(&self) -> PathBuf {
        self.data_dir.join("sources")
    }

    // ------------------------------------------------------------------ open

    /// `open_file`.
    pub fn open_file(&mut self, path: PathBuf, display_name: Option<String>) {
        let path = std::path::absolute(&path).unwrap_or(path);
        if !path.exists() {
            self.warn(format!("Soubor neexistuje:\n{}", path.display()));
            return;
        }
        // a file that exists but cannot be opened (no rights, a path the system refuses): said so, not a load error
        if let Err(e) = fs::File::open(&path) {
            self.critical(format!("Soubor se nepodařilo otevřít (cesta není přístupná):\n{}\n{e}", path.display()));
            return;
        }
        if self.flow.active() {
            self.cancel_load();
        }
        self.add_recent(&path);
        self.flow.t0 = Instant::now();
        self.flow.loading_path = path.clone();
        self.flow.display_name = display_name;
        self.flow.from_cache = false;
        self.flow.token += 1;
        self.set_busy(true, format!("Načítám {}…", base_name(&path)));
        if is_drawing(&path) {
            self.open_drawing(path);
        } else {
            self.open_model(path);
        }
    }

    fn open_model(&mut self, path: PathBuf) {
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let cache_file = match cache::cache_path(&path) {
            Ok(c) => c,
            Err(e) => {
                self.set_busy(false, "");
                self.critical(format!("Nepodařilo se načíst model:\n{e}"));
                return;
            }
        };
        if cache_file.exists() {
            let token = self.flow.token;
            self.spawn_task("load-cache", move |tx, ctx| {
                touch(&cache_file);
                match Document::load_xbf(&cache_file) {
                    Ok(doc) => {
                        let result = prepare_model(doc, true);
                        App::send(&tx, &ctx, Event::Model { token, result });
                    }
                    Err(_) => {
                        // a broken cache entry: load the original
                        let _ = fs::remove_file(&cache_file);
                        App::send(&tx, &ctx, Event::CacheBroken { token });
                    }
                }
            });
            return;
        }
        self.open_model_uncached(path, size, cache_file);
    }

    fn open_model_uncached(&mut self, path: PathBuf, size: u64, cache_file: PathBuf) {
        if !self.portable && !needs_local_copy(&path) {
            self.open_model_source(path.clone(), path, size, cache_file);
            return;
        }
        // Drive files may be online-only or change while they are read, and a path of 260 characters or more cannot be read
        // by Open CASCADE: copy first
        self.flow.pending = Some((path.clone(), size, cache_file));
        let token = self.flow.token;
        let sources = self.sources_dir();
        self.spawn_task("source-copy", move |tx, ctx| {
            let result = paths::source_copy_if(true, &path, &sources).map_err(|e| e.to_string());
            App::send(&tx, &ctx, Event::Source { token, result });
        });
    }

    /// Chooses between the warm worker, a new worker process and the in-process read.
    fn open_model_source(&mut self, path: PathBuf, src: PathBuf, size: u64, cache_file: PathBuf) {
        self.flow.source_copy = (src != path).then(|| src.clone());
        let threshold = worker_threshold();
        // a worker started with the program for exactly this file (`poll() in (None, 0)`)
        let early = match &mut self.flow.warm {
            Some(w) => {
                w.submitted.as_ref().is_some_and(|s| same_path(s, &path))
                    && match w.worker.child.try_wait() {
                        Ok(None) => true,
                        Ok(Some(st)) => st.success(),
                        Err(_) => false,
                    }
            }
            None => false,
        };
        if !early && self.flow.warm.as_ref().is_some_and(|w| w.submitted.is_some()) {
            self.drop_warm(); // started for another file
        }
        // small files are quicker in-process, unless an idle worker is already waiting (keeps the GUI lean)
        if !early && size < threshold && !(size >= cache::CACHE_MIN && self.flow.warm_ok()) {
            self.read_in_process(src, size, cache_file);
            return;
        }
        let dir = cache::cache_dir();
        if let Err(e) = cache::ensure_dir(&dir) {
            self.set_busy(false, "");
            self.critical(format!("Nepodařilo se spustit načítání:\n{e}"));
            return;
        }
        let tmp = cache::part_path(&cache_file);
        let slot = if early {
            self.flow.warm.take()
        } else {
            let _ = fs::remove_file(worker::log_path(&tmp));
            let usable = self.flow.warm_ok();
            if !usable {
                self.drop_warm();
                match worker::spawn(&self.exe) {
                    Ok(w) => self.flow.warm = Some(WarmSlot { worker: w, submitted: None }),
                    Err(e) => {
                        self.set_busy(false, "");
                        self.critical(format!("Nepodařilo se spustit načítání:\n{e}"));
                        return;
                    }
                }
            }
            let slot = self.flow.warm.take();
            match slot {
                Some(s) => match worker::submit(&s.worker.job_file, &src, &tmp) {
                    Ok(()) => Some(s),
                    Err(e) => {
                        s.worker.discard();
                        self.set_busy(false, "");
                        self.critical(format!("Nepodařilo se spustit načítání:\n{e}"));
                        return;
                    }
                },
                None => None,
            }
        };
        let Some(slot) = slot else { return };
        self.flow.run = Some(WorkerRun { child: slot.worker.child, job: slot.worker.job_file, tmp, cache: cache_file });
        self.flow.next_poll = Instant::now();
        self.ctx.request_repaint_after(POLL);
    }

    fn read_in_process(&mut self, src: PathBuf, size: u64, cache_file: PathBuf) {
        let token = self.flow.token;
        if size >= cache::CACHE_MIN {
            self.flow.store_after_show = Some(cache_file);
        }
        self.spawn_task("read-model", move |tx, ctx| {
            let t2 = tx.clone();
            let c2 = ctx.clone();
            let name = base_name(&src);
            let mut report = move |msg: &str| {
                let _ = t2.send(Event::Progress { token, text: format!("{name}: {msg}") });
                c2.request_repaint();
            };
            match Document::read(&src, &mut report) {
                Ok(doc) => {
                    let result = prepare_model(doc, false);
                    App::send(&tx, &ctx, Event::Model { token, result });
                }
                Err(e) => {
                    App::send(&tx, &ctx, Event::Model { token, result: Err(LoadFail::Failed(e.message().to_string())) });
                }
            }
        });
    }

    // ------------------------------------------------------------------ warm worker

    /// `prestart_worker`: an idle worker imports OCCT while the user is still picking a file.
    pub fn prestart_worker(&mut self) {
        if let Some(w) = &mut self.flow.warm {
            if matches!(w.worker.child.try_wait(), Ok(Some(_)) | Err(_)) {
                let _ = fs::remove_file(&w.worker.job_file);
                self.flow.warm = None;
            }
        }
        if self.flow.warm.is_none() {
            self.flow.warm = worker::spawn(&self.exe).ok().map(|worker| WarmSlot { worker, submitted: None });
        }
    }

    /// `_drop_warm`.
    pub fn drop_warm(&mut self) {
        if let Some(w) = self.flow.warm.take() {
            w.worker.discard();
        }
    }

    // ------------------------------------------------------------------ worker process

    /// Called every frame: reads the progress log of the worker process every 150 ms and reacts to its end.
    pub fn poll_worker(&mut self) {
        let now = Instant::now();
        if self.flow.run.is_none() {
            return;
        }
        if now < self.flow.next_poll {
            self.ctx.request_repaint_after(self.flow.next_poll - now);
            return;
        }
        self.flow.next_poll = now + POLL;
        self.ctx.request_repaint_after(POLL);
        let Some(run) = self.flow.run.as_mut() else { return };
        match run.child.try_wait() {
            Ok(Some(status)) => self.on_worker_finished(status.code().unwrap_or(-1)),
            Ok(None) => {
                let lines = worker::log_lines(&run.tmp);
                let msg = lines.last().cloned().unwrap_or_else(|| "Spouštím načítání…".to_string());
                self.status.model = format!(
                    "{}: {}  ({:.0} s)",
                    base_name(&self.flow.loading_path),
                    msg,
                    self.flow.t0.elapsed().as_secs_f64()
                );
            }
            Err(_) => self.on_worker_finished(-1),
        }
    }

    fn cleanup_run(run: &WorkerRun) {
        worker::remove_tmp(&run.tmp);
        let _ = fs::remove_file(&run.job); // left over only if the worker died before taking it
    }

    fn on_worker_finished(&mut self, code: i32) {
        let Some(run) = self.flow.run.take() else { return };
        let lines = worker::log_lines(&run.tmp);
        if let Some(err) = worker::failure_message(&lines, code) {
            Self::cleanup_run(&run);
            self.finish_failed_load(format!("Nepodařilo se načíst model:\n{err}"), false);
            return;
        }
        self.status.model = "Zobrazuji model…".to_string();
        let token = self.flow.token;
        let (tmp, cache_file, job) = (run.tmp.clone(), run.cache.clone(), run.job.clone());
        self.spawn_task("load-result", move |tx, ctx| {
            let loaded = fs::rename(&tmp, &cache_file)
                .map_err(|e| e.to_string())
                .and_then(|_| Document::load_xbf(&cache_file).map_err(|e| e.message().to_string()));
            let _ = fs::remove_file(worker::log_path(&tmp));
            let _ = fs::remove_file(&job);
            let ev = match loaded {
                Ok(doc) => Event::Model { token, result: prepare_model(doc, false) },
                Err(msg) => {
                    let _ = fs::remove_file(&cache_file);
                    Event::Model { token, result: Err(LoadFail::Failed(msg)) }
                }
            };
            App::send(&tx, &ctx, ev);
        });
    }

    /// A load ended with an error: back to a quiet state and a message.
    fn finish_failed_load(&mut self, text: String, warning: bool) {
        self.flow.thread_busy = false;
        self.flow.store_after_show = None;
        self.set_busy(false, "");
        self.update_status();
        if warning {
            self.warn(text);
        } else {
            self.critical(text);
        }
    }

    /// `cancel_load`. A thread cannot be stopped; its result is dropped by the token.
    pub fn cancel_load(&mut self) {
        self.flow.token += 1;
        self.flow.pending = None;
        self.flow.thread_busy = false;
        self.flow.store_after_show = None;
        if let Some(mut run) = self.flow.run.take() {
            let _ = run.child.kill();
            let _ = run.child.wait();
            Self::cleanup_run(&run);
        }
        if let Some(d) = self.drive.take() {
            d.cancel.cancel();
        }
        if let Some(c) = self.flow.drawing_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.cancel_scene_build();
        self.set_busy(false, "");
        self.update_status();
    }

    /// Stops the scene build of the open drawing that is running (layout, background or layer change): its thread leaves
    /// the work within a fraction of a second and its answer, if it comes at all, is dropped by the generation.
    pub fn cancel_scene_build(&mut self) {
        if let Some(c) = self.flow.scene_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.flow.scene_generation += 1;
        self.flow.scene_busy = false;
    }

    /// Program exit: the drawing load (its DWG converter) and the scene build in progress are told to stop, then the
    /// exit waits at most `wait` for the drawing thread to end. The thread kills its converter when it is told to stop;
    /// a converter it could not stop in time is ended by the job object when this process ends (`cad_2d::dwg`).
    pub fn stop_drawing_work(&mut self, wait: Duration) {
        if let Some(c) = self.flow.drawing_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if let Some(c) = self.flow.scene_cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if let Some(done) = self.flow.drawing_done.take() {
            // `Disconnected`: the thread has ended; `Timeout`: it is still at work and the exit goes on without it
            let _ = done.recv_timeout(wait);
        }
    }

    // ------------------------------------------------------------------ events

    /// Takes everything the background threads delivered.
    pub fn pump_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Progress { token, text } => {
                    if token == self.flow.token && self.flow.busy {
                        self.status.model = format!("{text}  ({:.0} s)", self.flow.t0.elapsed().as_secs_f64());
                    }
                }
                Event::Source { token, result } => {
                    if token != self.flow.token {
                        continue;
                    }
                    self.flow.thread_busy = false;
                    let pending = self.flow.pending.take();
                    match (result, pending) {
                        (Ok(src), Some((path, size, cache_file))) => self.open_model_source(path, src, size, cache_file),
                        (Err(e), _) => self.finish_failed_load(format!("Nepodařilo se načíst model:\n{e}"), false),
                        _ => {}
                    }
                }
                Event::CacheBroken { token } => {
                    if token != self.flow.token {
                        continue;
                    }
                    self.flow.thread_busy = false;
                    let path = self.flow.loading_path.clone();
                    // the source may have vanished since the cache entry was taken: the load ends with a message
                    if !path.exists() {
                        self.finish_failed_load(format!("Soubor neexistuje:\n{}", path.display()), false);
                        continue;
                    }
                    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    match cache::cache_path(&path) {
                        Ok(cache_file) => self.open_model_uncached(path, size, cache_file),
                        Err(e) => self.finish_failed_load(format!("Nepodařilo se načíst model:\n{e}"), false),
                    }
                }
                Event::Model { token, result } => {
                    if token != self.flow.token {
                        continue;
                    }
                    self.flow.thread_busy = false;
                    match result {
                        Ok(loaded) => {
                            let mut loaded = *loaded;
                            if loaded.from_cache {
                                self.flow.from_cache = true;
                            }
                            loaded.from_cache = self.flow.from_cache;
                            self.show_doc(loaded);
                        }
                        Err(LoadFail::Complex(m)) => self.finish_failed_load(m, true),
                        Err(LoadFail::Failed(m)) => {
                            let text = load_failure_text(&self.flow.loading_path, &m);
                            self.finish_failed_load(text, false);
                        }
                    }
                }
                Event::Drawing { token, result } => {
                    if token != self.flow.token {
                        continue;
                    }
                    self.flow.thread_busy = false;
                    match result {
                        Ok(l) => self.show_drawing(*l),
                        Err(m) => self.finish_failed_load(format!("Nepodařilo se načíst výkres:\n{m}"), false),
                    }
                }
                Event::Scene { generation, keep_view, result } => {
                    if generation == self.flow.scene_generation {
                        self.flow.scene_busy = false;
                        self.apply_scene(result, keep_view);
                    }
                }
                Event::Stl { generation, result } => {
                    if generation == self.flow.model_generation {
                        if let Some(s) = result {
                            self.stl_ready(*s);
                        }
                    }
                }
                Event::Collision(ev) => self.on_collision_event(ev),
                Event::Drive(ev) => self.on_drive_event(ev),
            }
        }
    }

    // ------------------------------------------------------------------ show

    /// `clear_model`.
    pub fn clear_model(&mut self) {
        self.set_measure(None);
        // a check that is still running is told to stop (it ends by itself and drops the document it holds)
        if let Some(d) = self.dlg.collisions.take() {
            d.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.dlg.material = None;
        self.model = None;
        self.draw = None;
        self.status.note.clear();
        self.overlays = Default::default();
        self.flow.model_generation += 1;
        self.cancel_scene_build();
        self.r3().clear_model();
        self.r3().clear_overlays();
        {
            // the scene of the drawing that was open is not kept on the graphics card for the model that comes next
            let mut r = self.r2();
            r.set_scene(&Scene2d::default());
            r.set_overlays(Default::default());
        }
        self.meas = Default::default();
    }

    /// `show_doc`: puts a loaded model on screen.
    fn show_doc(&mut self, mut loaded: Loaded3d) {
        let path = self.flow.loading_path.clone();
        if let Some(copy) = self.flow.source_copy.take() {
            name_after_source(&mut loaded.tree, &copy, &path);
        }
        self.clear_model();
        self.set_mode(Mode::ThreeD);
        self.sync_viewports();
        let mats = Assignments::load(&self.data_dir, &path).ok();
        // an STL keeps its mesh to add the sharp-edge outlines later
        let is_stl = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"));
        let leaves: Vec<_> = loaded.tree.nodes.iter().filter(|n| !n.is_assembly).collect();
        let stl_proto = (is_stl && leaves.len() == 1).then(|| leaves[0].proto).flatten().and_then(|p| loaded.protos.get(p as usize).cloned());
        // the model is on the graphics card now: a part that did not fit is a Czech warning, as for the drawings
        let warning = {
            let mut r = self.r3();
            r.set_model(&loaded.protos, &loaded.tree);
            r.take_warning()
        };
        if let Some(w) = warning {
            self.warn(w);
        }
        self.apply_style3();
        {
            let persp = self.opts.persp;
            let mut r = self.r3();
            r.set_perspective(persp);
            r.fit_all();
        }
        let display = self.flow.display_name.clone();
        let store = self.flow.store_after_show.take();
        let model = Model3::new(path.clone(), display, loaded, mats);
        self.model = Some(model);
        self.load_secs = self.flow.t0.elapsed().as_secs_f64();
        if let Some(cache_file) = store {
            // the model is on screen first; saving the copy can wait a moment (main.py: QTimer.singleShot(0))
            if let Some(doc) = self.model.as_ref().and_then(|m| m.doc.as_ref()) {
                let _ = cache::store(&cache_file, |p| doc.save_xbf(p));
            }
        }
        self.update_section(false);
        self.set_busy(false, "");
        self.update_status();
        self.show_model_info();
        self.rebuild_tree_rows();
        self.prune_due = Some(Instant::now());
        if let Some(proto) = stl_proto {
            self.start_stl_analysis(path, proto);
        }
    }

    /// STL: numpy mesh for measuring, built in a background thread (big meshes take a second or two).
    fn start_stl_analysis(&mut self, path: PathBuf, proto: cad_types::ProtoMesh) {
        let generation = self.flow.model_generation;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        let (copy, sources) = (self.portable || needs_local_copy(&path), self.sources_dir());
        let _ = spawn_big("stl-mesh", move || {
            // portable mode: the file on the Drive may be online-only or change, so the analysis reads the local copy
            // (the copy that was made for the load is found again by its name; outside portable mode this is `path`, unless
            // the path is too long to be read)
            let src = paths::source_copy_if(copy, &path, &sources).ok();
            let mesh = src.and_then(|src| cad_core::meshmeasure::mesh_for_file(&src, cad_core::meshmeasure::MAX_TRIS)).map(|mut m| {
                let loops: Vec<_> = m.loops().to_vec();
                let mut proto = proto;
                add_loop_edges(&mut proto, &loops);
                Box::new(StlMesh { mesh: m, proto })
            });
            App::send(&tx, &ctx, Event::Stl { generation, result: mesh });
        });
    }

    /// The analysed mesh arrived: its sharp edges are drawn as edges of the model and become pickable.
    fn stl_ready(&mut self, stl: StlMesh) {
        let Some(model) = self.model.as_mut() else { return };
        let tree = model.tree.clone();
        let hidden = model.hidden.clone();
        let selected = model.selection();
        let proto = stl.proto.clone();
        model.stl = Some(stl);
        let warning = {
            let mut r = self.r3();
            r.set_model(&[proto], &tree);
            r.set_visibility(&hidden);
            r.set_selection(&selected);
            r.take_warning()
        };
        if let Some(w) = warning {
            self.warn(w);
        }
        self.apply_style3();
        if self.measure == Some(crate::app::MeasureKind::Dist) {
            self.r3().set_measure_mode(true);
        }
        self.refresh_overlays();
        if self.measure.is_none() && self.model.as_ref().is_some_and(|m| m.selected.len() == 1) {
            self.refresh_props();
        }
    }

    /// `on_drawing_loaded`.
    fn show_drawing(&mut self, l: Loaded2d) {
        let path = self.flow.loading_path.clone();
        self.clear_model();
        self.set_mode(Mode::TwoD);
        self.sync_viewports();
        let display = self.flow.display_name.clone();
        let from_cache = l.from_cache;
        let mut draw = Draw2::new(path, display, l.drawing, l.layout, from_cache);
        let scene = match l.scene {
            Ok((scene, space)) => {
                draw.space = space;
                draw.rebuild_layers(&scene);
                Some(scene)
            }
            Err(msg) => {
                self.warn(msg);
                None
            }
        };
        self.draw = Some(draw);
        match scene {
            Some(scene) => self.put_scene(scene, false),
            None => self.put_empty_scene(),
        }
        let content = self.r2().visible_bounds();
        if let Some(d) = self.draw.as_mut() {
            d.content = content;
        }
        self.load_secs = self.flow.t0.elapsed().as_secs_f64();
        self.set_busy(false, "");
        self.update_status();
        self.show_drawing_info();
        self.prune_due = Some(Instant::now());
        self.flow.from_cache = from_cache;
    }

    fn open_drawing(&mut self, path: PathBuf) {
        let is_dwg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dwg"));
        let dxf_cache = if is_dwg {
            let _ = cache::ensure_dir(&cache::cache_dir());
            cache::cache_path(&path).ok().map(|c| c.with_extension("dxf"))
        } else {
            None
        };
        self.flow.from_cache = dxf_cache.as_ref().is_some_and(|c| c.exists());
        let token = self.flow.token;
        let light = self.settings.light_bg;
        let portable = self.portable;
        let sources = self.sources_dir();
        let from_cache = self.flow.from_cache;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.flow.drawing_cancel.replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        self.flow.drawing_done = Some(done_rx);
        self.spawn_task("load-drawing", move |tx, ctx| {
            // `done_tx` is dropped when this thread ends: the exit waits for that disconnect (`stop_drawing_work`)
            let _done = done_tx;
            let t2 = tx.clone();
            let c2 = ctx.clone();
            let name = base_name(&path);
            let mut report = move |m: &str| {
                let _ = t2.send(Event::Progress { token, text: format!("{name}: {m}") });
                c2.request_repaint();
            };
            let result = (|| -> Result<Box<Loaded2d>, String> {
                let src = paths::source_copy_if(portable, &path, &sources).map_err(|e| e.to_string())?;
                let opts = drawing_load_options(&path, dxf_cache);
                let drawing = cad_2d::load_cancellable(&src, &opts, &mut report, Some(&*cancel)).map_err(|e| e.to_string())?;
                report("Vykresluji…");
                let layout = drawing.default_layout();
                // cancelling the load (or opening another file) stops this build as well, not only the DWG conversion
                let scene = match crate::scene2d::build_view(&drawing, &layout, light, true, &cancel) {
                    Ok(s) => Ok(s),
                    Err(cad_2d::SceneError::TooComplex(e)) => Err(e.to_string()),
                    Err(cad_2d::SceneError::Cancelled) => return Err(cad_2d::SceneError::Cancelled.to_string()),
                };
                Ok(Box::new(Loaded2d { drawing: Arc::new(drawing), layout, scene, from_cache }))
            })();
            App::send(&tx, &ctx, Event::Drawing { token, result });
        });
    }

    /// `render_drawing`: the scene of the current layout again (layout, background or a layer change).
    pub fn render_drawing(&mut self, keep_view: bool) {
        let Some(d) = &self.draw else { return };
        let (drawing, layout) = (d.drawing.clone(), d.layout.clone());
        // a build that is still running is not wanted any more: a new one replaces it
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(old) = self.flow.scene_cancel.replace(cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        self.flow.scene_generation += 1;
        self.flow.scene_busy = true;
        let generation = self.flow.scene_generation;
        let light = self.settings.light_bg;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.status.model = "Vykresluji…".to_string();
        let _ = spawn_big("scene-2d", move || {
            match crate::scene2d::build_view(&drawing, &layout, light, true, &cancel) {
                Ok(result) => App::send(&tx, &ctx, Event::Scene { generation, keep_view, result: Ok(result) }),
                Err(cad_2d::SceneError::TooComplex(e)) => App::send(&tx, &ctx, Event::Scene { generation, keep_view, result: Err(e.to_string()) }),
                // cancelled: nobody waits for the answer (the generation moved on)
                Err(cad_2d::SceneError::Cancelled) => {}
            }
        });
    }

    /// Buffers that are made for a frame or a pick (overlays, the highlight of an entity, the instances of a model) and did
    /// not fit into the memory of the graphics card: the renderers leave them out and say so once, here.
    pub fn poll_gpu_warnings(&mut self) {
        let w2 = self.r2().take_warning();
        let w3 = self.r3().take_warning();
        for w in [w2, w3].into_iter().flatten() {
            self.warn(w);
        }
    }

    /// The scene did not fit into the memory of the graphics card (the renderer drew what fitted): a warning, no crash.
    pub fn warn_gpu_2d(&mut self) {
        let warning = self.r2().take_warning();
        if let Some(w) = warning {
            self.warn(w);
        }
    }

    /// Puts a scene of the open drawing on the graphics card, with the background of its layout and the layers that are shown,
    /// and gives the scene up: its batches are freed as they are uploaded. Nothing else of the drawing is kept in two copies.
    fn put_scene(&mut self, scene: Scene2d, keep_view: bool) {
        let Some(d) = self.draw.as_mut() else { return };
        let light = self.settings.light_bg;
        let bg = d.drawing.layout_background(&d.layout, light);
        let mask = d.drawing.visible_batches();
        // the pens that are the foreground of the layout (and the wipeouts, which are its background) are drawn in the colours
        // of the palette: the white background option changes those, not the scene
        let roles: Vec<u8> = d.drawing.batch_roles().iter().map(|r| r.code()).collect();
        let fg = d.drawing.layout_foreground(&d.layout, light);
        d.shown_layout = d.layout.clone();
        d.shown_light = light;
        self.r2().set_scene_owned(scene, &mask, &roles, Some((fg, bg)), keep_view);
        self.warn_gpu_2d();
    }

    /// An empty scene (the layout is too complex to be drawn) with the background of the layout.
    fn put_empty_scene(&mut self) {
        let Some(d) = self.draw.as_mut() else { return };
        let light = self.settings.light_bg;
        let bg = d.drawing.layout_background(&d.layout, light);
        let fg = d.drawing.layout_foreground(&d.layout, light);
        d.shown_layout = d.layout.clone();
        d.shown_light = light;
        let mut r = self.r2();
        r.set_scene(&Scene2d::default());
        r.set_palette(fg, bg);
    }

    fn apply_scene(&mut self, result: Result<(Scene2d, SceneSpace), String>, keep_view: bool) {
        let Some(draw) = self.draw.as_mut() else { return };
        match result {
            Ok((scene, space)) => {
                draw.space = space;
                draw.rebuild_layers(&scene);
                self.put_scene(scene, keep_view);
            }
            Err(msg) => {
                let key = (self.draw.as_ref().map(|d| d.layout.clone()).unwrap_or_default(), msg.clone());
                self.put_empty_scene();
                if self.draw.as_ref().is_some_and(|d| d.warned.as_ref() != Some(&key)) {
                    if let Some(d) = self.draw.as_mut() {
                        d.warned = Some(key);
                    }
                    self.warn(msg);
                }
            }
        }
        let content = self.r2().visible_bounds();
        if let Some(d) = self.draw.as_mut() {
            d.content = content;
            d.clear_pick();
        }
        self.meas2.clear();
        self.update_status();
        self.show_drawing_info();
    }

    /// "Zrušit" while the scene of a layout or a background is being built: the build is told to stop and the screen keeps the
    /// scene that is on it. The layout and the white background option go back to what that scene is (the layout list and
    /// the check box said what was asked for).
    pub fn cancel_scene_rebuild(&mut self) {
        self.cancel_scene_build();
        let mut restore_light = None;
        if let Some(d) = self.draw.as_mut() {
            d.layout = d.shown_layout.clone();
            if d.drawing.light_option_matters(&d.layout) && self.settings.light_bg != d.shown_light {
                restore_light = Some(d.shown_light);
            }
        }
        if let Some(l) = restore_light {
            self.settings.light_bg = l;
            self.save_settings();
        }
        self.update_status();
    }

    // ------------------------------------------------------------------ cache housekeeping

    /// `prune_cache`, only between loads.
    pub fn maybe_prune(&mut self) {
        if let Some(due) = self.prune_due {
            if Instant::now() >= due && !self.flow.active() {
                cache::prune(&self.data_dir);
                self.prune_due = None;
            }
        }
    }

    // ------------------------------------------------------------------ status line

    /// `update_status`.
    pub fn update_status(&mut self) {
        self.status.note.clear();
        if self.mode == Mode::TwoD {
            if let Some(d) = &self.draw {
                if !d.drawing.scene_notes(&d.layout).is_empty() {
                    self.status.note = crate::strings::STATUS_REDUCED_DETAIL.to_string();
                }
                let info = d.drawing.info();
                let n = d.drawing.layout_entities(&d.layout).len();
                // the layers of the list on the left (Python: `len(doc.layers)`, which is also what its list shows)
                let layers = if d.layers.is_empty() { info.layers } else { d.layers.len() };
                let mut txt = format!(
                    "{}  |  {}  |  {} entit, {} hladin",
                    crate::model3::file_label(&d.path, &d.display_name),
                    info.release,
                    n,
                    layers
                );
                if let Some(c) = &d.content {
                    txt.push_str(&format!(
                        "  |  rozměr {} × {} {}",
                        cad_core::fmt::fmt_len(c.1[0] - c.0[0]),
                        cad_core::fmt::fmt_len(c.1[1] - c.0[1]),
                        info.units
                    ));
                }
                txt.push_str(&format!(
                    "  |  načteno za {} s{}",
                    secs1(self.load_secs),
                    if d.from_cache { " (DWG z mezipaměti)" } else { "" }
                ));
                self.status.model = txt;
                return;
            }
        }
        let Some(m) = &self.model else {
            self.status.model = "Žádný model".to_string();
            return;
        };
        if !m.has_nodes() {
            self.status.model = "Žádný model".to_string();
            return;
        }
        let parts = m.tree.nodes.iter().filter(|n| !n.is_assembly).count();
        let asms = m.tree.nodes.len() - parts;
        let mut txt = format!(
            "{}  |  {} dílů ({} unikátních), {} sestav",
            crate::model3::file_label(&m.path, &m.display_name),
            parts,
            m.tree.proto_instances.len(),
            asms
        );
        if let Some(b) = &m.model_box {
            txt.push_str(&format!("  |  rozměr {} mm", Model3::size_text(b)));
        }
        txt.push_str(&format!("  |  načteno za {} s{}", secs1(self.load_secs), if m.from_cache { " (z mezipaměti)" } else { "" }));
        self.status.model = txt;
    }

    /// `show_model_info` / `show_drawing_info`: the panel when nothing is selected.
    pub fn show_model_info(&mut self) {
        if let Some(m) = &self.model {
            if m.has_nodes() {
                self.props = m.model_info(self.load_secs);
            }
        }
    }

    /// `set_mode`.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.set_measure(None);
        self.mode = mode;
        self.status.coord.clear();
        self.set_measure(None);
    }
}

/// Puts the sharp-edge loops of an STL into the proto mesh as polylines (edges), a closed loop returns to its start.
pub fn add_loop_edges(proto: &mut cad_types::ProtoMesh, loops: &[cad_core::meshmeasure::Loop]) {
    proto.edge_points.clear();
    proto.edges.clear();
    for lp in loops {
        // every loop gets an edge (even a degenerate one), so edge index = loop index
        let first = proto.edge_points.len() as u32;
        for p in &lp.pts {
            proto.edge_points.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
        if lp.closed {
            let p = lp.pts[0];
            proto.edge_points.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
        proto.edges.push(cad_types::EdgeSpan { first_point: first, point_count: proto.edge_points.len() as u32 - first });
    }
}

#[cfg(test)]
mod scene_cancel_tests {
    use super::*;
    use crate::ui::Init;
    use cad_core::settings::Settings;
    use cad_render::{wgpu, GpuContext, Renderer2d, Renderer3d};

    /// An application without a window (needs a graphics adapter; without one the test says so and returns).
    fn app() -> Option<(App, GpuContext, tempfile::TempDir)> {
        let gpu = GpuContext::new_headless().ok()?;
        let dir = tempfile::tempdir().ok()?;
        let fmt = wgpu::TextureFormat::Rgba8Unorm;
        let r3 = Renderer3d::new(&gpu.device, fmt).with_queue(&gpu.queue).shared();
        let r2 = Renderer2d::new(&gpu.device, fmt).with_queue(&gpu.queue).shared();
        let init = Init { file: None, link: None, exe: PathBuf::from("StepViewer"), early: None };
        let mut app = App::build(egui::Context::default(), r3, r2, None, init, Settings::default());
        app.settings_path = dir.path().join("settings.json");
        app.data_dir = dir.path().join("cache");
        app.prune_due = None;
        Some((app, gpu, dir))
    }

    fn with_drawing(app: &mut App) {
        let d = cad_2d::load_bytes(crate::testgen::dxf_drawing().as_bytes(), cad_2d::FontOptions { system_fonts: false, dirs: Vec::new() }).expect("drawing");
        app.draw = Some(Draw2::new(PathBuf::from("a.dxf"), None, Arc::new(d), "Model".to_string(), false));
    }

    #[test]
    fn cancelling_a_load_or_closing_the_drawing_stops_the_running_scene_build() {
        let Some((mut app, _gpu, _dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        for how in ["cancel_load", "clear_model"] {
            let flag = Arc::new(AtomicBool::new(false));
            app.flow.scene_cancel = Some(flag.clone());
            app.flow.scene_busy = true;
            let generation = app.flow.scene_generation;
            match how {
                "cancel_load" => app.cancel_load(),
                _ => app.clear_model(),
            }
            assert!(flag.load(Ordering::Relaxed), "{how}: the build is told to stop");
            assert!(!app.flow.scene_busy, "{how}");
            assert!(app.flow.scene_generation > generation, "{how}: an answer that still comes is dropped");
            assert!(app.flow.scene_cancel.is_none());
        }
    }

    #[test]
    fn a_new_scene_replaces_the_one_that_is_still_being_built() {
        let Some((mut app, _gpu, _dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        with_drawing(&mut app);
        app.render_drawing(false);
        let first = app.flow.scene_cancel.clone().expect("the build has a flag");
        app.render_drawing(true);
        let second = app.flow.scene_cancel.clone().expect("the second build has a flag");
        assert!(first.load(Ordering::Relaxed), "the first build was told to stop");
        assert!(!Arc::ptr_eq(&first, &second));
        // the answer of the second one arrives and is used; the one of the first (if it came) is dropped by its generation
        let t0 = Instant::now();
        while app.flow.scene_busy && t0.elapsed() < Duration::from_secs(20) {
            app.pump_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.flow.scene_busy, "the scene arrived");
        assert!(app.r2().visible_bounds().is_some());
    }

    #[test]
    fn a_broken_cache_entry_whose_source_has_vanished_ends_the_load_with_a_message() {
        let Some((mut app, _gpu, dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        app.flow.token += 1;
        app.flow.loading_path = dir.path().join("zmizely.step");
        app.flow.busy = true;
        app.flow.thread_busy = true;
        let token = app.flow.token;
        app.tx.send(Event::CacheBroken { token }).unwrap();
        app.pump_events();
        assert!(!app.flow.active(), "the busy state is cleared: nothing stays in \"Načítám…\"");
        let text = app.msgs.back().map(|m| m.text.clone()).unwrap_or_default();
        assert!(text.starts_with("Soubor neexistuje:"), "{text}");
    }

    #[test]
    fn the_exit_stops_a_drawing_load_and_waits_only_a_bounded_time() {
        let Some((mut app, _gpu, _dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        app.flow.drawing_cancel = Some(cancel.clone());
        // a drawing thread that is still at work: its sender is kept alive here
        let (still_working, done) = std::sync::mpsc::channel::<()>();
        app.flow.drawing_done = Some(done);
        let t = Instant::now();
        app.stop_drawing_work(Duration::from_millis(200));
        assert!(cancel.load(Ordering::Relaxed), "the converter is told to stop");
        assert!(t.elapsed() < Duration::from_secs(5), "the wait is bounded: {:?}", t.elapsed());
        drop(still_working);
        // a drawing thread that has ended costs no wait at all
        let (ended_tx, ended) = std::sync::mpsc::channel::<()>();
        drop(ended_tx);
        app.flow.drawing_done = Some(ended);
        let t = Instant::now();
        app.stop_drawing_work(Duration::from_secs(30));
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        // closing the window does the same (`on_exit`)
        let cancel = Arc::new(AtomicBool::new(false));
        app.flow.drawing_cancel = Some(cancel.clone());
        eframe::App::on_exit(&mut app);
        assert!(cancel.load(Ordering::Relaxed));
    }

    /// The note of a drawing that is drawn with less detail is the first row of its panel and a short note in the status bar.
    #[test]
    fn a_drawing_drawn_with_less_detail_says_so_first_in_the_panel_and_in_the_status_bar() {
        let Some((mut app, _gpu, _dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        let fonts = || cad_2d::FontOptions { system_fonts: false, dirs: Vec::new() };
        // the detail of the filled shapes (the text of the sample) is reduced from the first vertex
        let mut reduced = cad_2d::load_bytes(crate::testgen::dxf_drawing().as_bytes(), fonts()).expect("drawing");
        reduced.set_out_limits(0, 10_000_000);
        app.draw = Some(Draw2::new(PathBuf::from("a.dxf"), None, Arc::new(reduced), "Model".to_string(), false));
        app.mode = Mode::TwoD;
        let d = app.draw.as_ref().expect("drawing").drawing.clone();
        let (scene, space) = crate::scene2d::build(&d, "Model", false, false).expect("scene");
        app.apply_scene(Ok((scene, space)), false);
        assert_eq!(app.status.note, crate::strings::STATUS_REDUCED_DETAIL);
        assert!(app.status.model.starts_with("a.dxf  |  "), "{}", app.status.model);
        let crate::props::Props::Rows { rows, .. } = &app.props else { panic!("the panel of the drawing") };
        assert_eq!(rows[0].label, "Upozornění", "{rows:?}");
        assert!(matches!(&rows[0].value[..], [crate::props::Span::Warn(t)] if t.contains("zjednodušen")), "{:?}", rows[0]);
        // the plain rows follow the warnings, in their own order
        let at = |l: &str| rows.iter().position(|r| r.label == l).unwrap_or_else(|| panic!("no row {l}"));
        assert!(at("Složka") > 0 && at("Typy entit") > at("Složka"), "{rows:?}");
        // a drawing at full detail has no note, and closing the drawing takes the note away
        let full = cad_2d::load_bytes(crate::testgen::dxf_drawing().as_bytes(), fonts()).expect("drawing");
        app.draw = Some(Draw2::new(PathBuf::from("a.dxf"), None, Arc::new(full), "Model".to_string(), false));
        let d = app.draw.as_ref().expect("drawing").drawing.clone();
        let (scene, space) = crate::scene2d::build(&d, "Model", false, false).expect("scene");
        app.apply_scene(Ok((scene, space)), false);
        assert!(app.status.note.is_empty());
        let crate::props::Props::Rows { rows, .. } = &app.props else { panic!("the panel of the drawing") };
        assert!(rows.iter().all(|r| r.label != "Upozornění"), "{rows:?}");
        app.status.note = "x".to_string();
        app.clear_model();
        assert!(app.status.note.is_empty());
    }

    #[test]
    fn a_scene_that_does_not_fit_the_graphics_card_gives_a_czech_warning() {
        let Some((mut app, _gpu, _dir)) = app() else {
            eprintln!("SKIP: no graphics adapter");
            return;
        };
        with_drawing(&mut app);
        let d = app.draw.as_ref().expect("drawing").drawing.clone();
        let (scene, space) = crate::scene2d::build(&d, "Model", false, false).expect("scene");
        // a device that takes no buffer of this size
        app.r2().set_simulated_buffer_limit(Some(16));
        app.apply_scene(Ok((scene.clone(), space)), false);
        let warned: Vec<&str> = app.msgs.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(warned, [cad_render::gpubuf::GPU_BUFFER_WARNING], "{warned:?}");
        assert!(warned[0].starts_with("Výkres se nevešel celý do paměti grafické karty"));
        // a device that takes them: no warning
        app.msgs.clear();
        app.r2().set_simulated_buffer_limit(None);
        app.apply_scene(Ok((scene, space)), false);
        assert!(app.msgs.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cad_core::meshmeasure::Loop;

    #[test]
    fn worker_threshold_default_is_the_cache_minimum() {
        // the environment of the test process has no STEPVIEW_WORKER_MIN
        if std::env::var_os("STEPVIEW_WORKER_MIN").is_none() {
            assert_eq!(worker_threshold(), 1024 * 1024);
        }
    }

    #[test]
    fn loop_edges_become_polylines_and_closed_loops_return_to_the_start() {
        let mut proto = cad_types::ProtoMesh::default();
        let open = Loop::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], false);
        let closed = Loop::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]], true);
        add_loop_edges(&mut proto, &[open, closed]);
        assert_eq!(proto.edges.len(), 2);
        assert_eq!((proto.edges[0].first_point, proto.edges[0].point_count), (0, 3));
        assert_eq!((proto.edges[1].first_point, proto.edges[1].point_count), (3, 5));
        assert_eq!(proto.edge_points[3], proto.edge_points[7]);
        assert_eq!(proto.edge_points.len(), 8);
    }

    #[test]
    fn a_long_path_is_named_when_a_model_cannot_be_read() {
        assert_eq!(load_failure_text(Path::new(r"C:\a.step"), "X"), "Nepodařilo se načíst model:\nX");
        let long = PathBuf::from(format!(r"C:\{}\a.step", "d".repeat(260)));
        let text = load_failure_text(&long, "X");
        assert!(text.starts_with("Nepodařilo se načíst model:\nX\n"), "{text}");
        assert!(text.contains("znaků"), "{text}");
    }

    /// Portable mode loads a copy of the drawing from the cache; the pictures of its IMAGE entities lie next to the original.
    #[test]
    fn the_load_options_of_a_drawing_name_the_folder_of_the_original() {
        let orig = tempfile::tempdir().unwrap();
        let sources = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])).save(orig.path().join("pic.png")).unwrap();
        let dxf = orig.path().join("plan.dxf");
        fs::write(&dxf, crate::testgen::dxf_with_picture("pic.png")).unwrap();
        let copy = paths::source_copy_if(true, &dxf, sources.path()).unwrap();
        assert_ne!(copy.parent(), dxf.parent(), "the copy is somewhere else");
        let opts = drawing_load_options(&dxf, None);
        assert_eq!(opts.original_dir.as_deref(), Some(orig.path()));
        let pictures = |opts: &LoadOptions| {
            let d = cad_2d::load(&copy, opts, &mut |_| {}).unwrap();
            let layout = d.default_layout();
            crate::scene2d::build(&d, &layout, false, true).unwrap().0.images.len()
        };
        assert_eq!(pictures(&opts), 1, "the picture next to the original is drawn");
        assert_eq!(pictures(&LoadOptions { original_dir: None, ..opts }), 0, "next to the copy there is none");
        // a bare file name has no folder to name
        assert_eq!(drawing_load_options(Path::new("plan.dxf"), None).original_dir, None);
    }

    #[test]
    fn same_path_ignores_case_on_windows() {
        if cfg!(windows) {
            assert!(same_path(Path::new(r"C:\A\b.step"), Path::new(r"c:\a\B.STEP")));
        }
        assert!(!same_path(Path::new("a.step"), Path::new("b.step")));
    }
}
