//! Load worker roles `--worker` and `--worker-serve` (main.py: `run_worker`, `serve_worker`).
//!
//! The worker loads a model in a process of its own, so the memory of the STEP translation never ends up in the
//! window process. It writes progress lines to `<dst>.log` and ends them with `DONE` or `ERROR <message>`.
use std::path::{Path, PathBuf};

use occt_bridge::{BridgeError, Document};

use crate::util::run_big;

fn read(path: &Path, report: &mut dyn FnMut(&str)) -> Result<Document, BridgeError> {
    Document::read(path, report)
}

fn save(doc: &Document, dst: &Path) -> Result<(), BridgeError> {
    doc.save_xbf(dst)
}

/// `--worker <src> <dst>`: one load, exit code 0 on success.
pub fn worker(src: PathBuf, dst: PathBuf) -> i32 {
    run_big("worker", move || cad_core::worker::run_worker(&src, &dst, read, save)).unwrap_or(1)
}

/// `--worker-serve <job>`: waits for the job file, then loads. Exit code 0 after a finished job or an idle timeout.
pub fn serve(job: PathBuf) -> i32 {
    run_big("worker-serve", move || cad_core::worker::serve_worker(&job, read, save)).unwrap_or(1)
}
