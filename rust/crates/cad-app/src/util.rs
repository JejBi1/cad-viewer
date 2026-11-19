//! Small helpers shared by the process roles and the window.
use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

/// Stack of every thread that calls Open CASCADE (SPEC: at least 64 MiB). OCCT recurses deeply on nested assemblies.
pub const BIG_STACK: usize = 64 * 1024 * 1024;

/// Starts a named thread with [`BIG_STACK`].
pub fn spawn_big<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> io::Result<JoinHandle<T>> {
    std::thread::Builder::new().name(name.to_string()).stack_size(BIG_STACK).spawn(f)
}

/// Runs `f` on a big-stack thread and waits for it. `None` when the thread could not be started or panicked.
pub fn run_big<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    spawn_big(name, f).ok()?.join().ok()
}

/// Lower-case extension without the dot, empty when there is none.
pub fn ext_lower(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// 3D model extensions (`CAD_EXT`).
pub const CAD_EXT: [&str; 5] = ["step", "stp", "iges", "igs", "stl"];
/// Drawing extensions (`view2d.DRAW_EXT`).
pub const DRAW_EXT: [&str; 2] = ["dxf", "dwg"];

pub fn is_cad(p: &Path) -> bool {
    CAD_EXT.contains(&ext_lower(p).as_str())
}

pub fn is_drawing(p: &Path) -> bool {
    DRAW_EXT.contains(&ext_lower(p).as_str())
}

pub fn is_supported(p: &Path) -> bool {
    is_cad(p) || is_drawing(p)
}

/// File name for messages and the title.
pub fn base_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Folder where the `libredwg` converter folder is searched in addition to the places `cad-2d` knows (next to the
/// executable). Development builds also look into the repository's `third_party` folder.
pub fn third_party_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..").join("third_party");
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// File size as the panel shows it: megabytes with two decimals and a decimal point, as Python's
/// `f"{os.path.getsize(path) / 1024 / 1024:.2f} MB"` (`main.py`, `show_model_info` and `show_drawing_info`).
pub fn size_mb(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1024.0 / 1024.0)
}

/// Seconds as shown in the status line: one decimal with a decimal point, as Python's `f"{t:.1f}"`.
pub fn secs1(t: f64) -> String {
    format!("{t:.1}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_classes() {
        for f in ["a.STEP", "a.stp", "a.iges", "a.IGS", "a.stl"] {
            assert!(is_cad(Path::new(f)), "{f}");
            assert!(!is_drawing(Path::new(f)), "{f}");
        }
        for f in ["a.dxf", "a.DWG"] {
            assert!(is_drawing(Path::new(f)), "{f}");
            assert!(is_supported(Path::new(f)));
        }
        assert!(!is_supported(Path::new("a.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn big_stack_thread_runs_and_reports_panics() {
        assert_eq!(run_big("t", || 21 * 2), Some(42));
        assert_eq!(run_big("t", || -> i32 { panic!("boom") }), None);
        // the stack really is as big as asked: an 8 MiB array on the stack fits (a 2 MiB default stack would not)
        let r = run_big("t", || {
            let a = std::hint::black_box([1u8; 8 * 1024 * 1024]);
            a.iter().map(|b| *b as u64).sum::<u64>()
        });
        assert_eq!(r, Some(8 * 1024 * 1024));
    }

    #[test]
    fn names() {
        assert_eq!(base_name(Path::new("C:\\a\\b.step")), if cfg!(windows) { "b.step" } else { "C:\\a\\b.step" });
        assert_eq!(secs1(1.25), "1.2");
    }

    #[test]
    fn file_size_is_written_like_python_does() {
        // f"{n / 1024 / 1024:.2f} MB": a decimal point (also in the Czech window), two decimals, mebibytes
        assert_eq!(size_mb(0), "0.00 MB");
        assert_eq!(size_mb(1024), "0.00 MB");
        assert_eq!(size_mb(6 * 1024), "0.01 MB");
        assert_eq!(size_mb(1024 * 1024), "1.00 MB");
        assert_eq!(size_mb(1024 * 1024 * 3 / 2), "1.50 MB");
        assert_eq!(size_mb(55_900_000), "53.31 MB");
    }
}
