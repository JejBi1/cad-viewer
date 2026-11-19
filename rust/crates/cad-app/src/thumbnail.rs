//! `--thumbnail <src> <png> <size>`: the renderer behind the Explorer thumbnail handler (`stepview/thumbs.py`).
//!
//! 3D models (STEP / IGES / STL) are meshed and drawn from the isometric direction on a transparent background;
//! drawings (DXF / DWG) on white paper. The renderer uses the GPU through wgpu and falls back to a software adapter
//! (WARP on Windows) when there is none. No window is ever created and no dialog is shown: any failure ends with exit
//! code 1 and the shell shows the icon. Exit code 0 means the PNG was written.
use std::fs;
use std::path::{Path, PathBuf};

use cad_2d::{FontOptions, LoadOptions};
use cad_render::{wgpu, GpuContext, Renderer2d, Renderer3d};
use cad_types::ProtoMesh;
use occt_bridge::Document;

use crate::steprefs;
use crate::util::{ext_lower, run_big, third_party_dir};

/// Largest input that gets a thumbnail (`MAX_INPUT`, 80 MiB; the shell extension applies the same limit).
pub const MAX_INPUT: u64 = 80 * 1024 * 1024;
/// Size is clamped to this range (`max(16, min(size, 1024))`).
pub const MIN_SIZE: i64 = 16;
pub const MAX_SIZE: i64 = 1024;
/// Environment switch: force the software adapter (`STEPVIEW_SOFT_THUMBS`).
pub const ENV_SOFT: &str = "STEPVIEW_SOFT_THUMBS";

/// Parses and clamps the size argument. Anything that is not an integer is an error (exit code 1).
pub fn clamp_size(text: &str) -> Option<u32> {
    let v: i64 = text.trim().parse().ok()?;
    Some(v.clamp(MIN_SIZE, MAX_SIZE) as u32)
}

/// What a file is rendered as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Model,
    Drawing,
}

pub fn kind_of(path: &Path) -> Kind {
    match ext_lower(path).as_str() {
        "dxf" | "dwg" => Kind::Drawing,
        _ => Kind::Model,
    }
}

/// Entry point of the role. Runs the render on a big-stack thread (OCCT recursion) and returns the exit code.
pub fn run(src: &Path, out: &Path, size: &str) -> i32 {
    let (src, out, size) = (src.to_path_buf(), out.to_path_buf(), size.to_string());
    run_big("thumbnail", move || run_inner(&src, &out, &size)).unwrap_or(1)
}

fn run_inner(src: &Path, out: &Path, size: &str) -> i32 {
    run_with(src, out, size, None)
}

/// `run_inner` on a device that takes no buffer over `gpu_limit` bytes (tests of the failure path; `None` is the real device).
fn run_with(src: &Path, out: &Path, size: &str, gpu_limit: Option<u64>) -> i32 {
    let Some(size) = clamp_size(size) else { return 1 };
    match fs::metadata(src) {
        Ok(m) if m.len() <= MAX_INPUT => {}
        _ => return 1,
    }
    let ext = ext_lower(src);
    // only STEP is scanned (thumbs.run): an unreadable file is refused as well
    if matches!(ext.as_str(), "step" | "stp") && steprefs::remote_step_refs(src).unwrap_or(true) {
        return 1;
    }
    let result = match kind_of(src) {
        Kind::Drawing => render_2d(src, size, gpu_limit),
        Kind::Model => render_3d(src, size),
    };
    match result {
        Ok(png) => {
            if fs::write(out, png).is_ok() && out.exists() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("thumbnail failed: {e}");
            1
        }
    }
}

/// Hardware adapter first, then the software one; `STEPVIEW_SOFT_THUMBS` goes straight to software.
fn gpu() -> Result<GpuContext, String> {
    let g = if std::env::var_os(ENV_SOFT).is_some() { GpuContext::new_software() } else { GpuContext::new_headless() };
    g.map_err(|e| e.to_string())
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Meshes of every prototype of a model plus its instance tree.
pub fn load_model(path: &Path) -> Result<(Document, cad_types::ModelTree, Vec<ProtoMesh>), String> {
    let doc = Document::read(path, &mut |_| {}).map_err(|e| e.to_string())?;
    let tree = doc.tree().map_err(|e| e.to_string())?;
    let protos = (0..doc.proto_count()).map(|i| doc.proto_mesh(i)).collect();
    Ok((doc, tree, protos))
}

/// A private short copy of a file whose path is too long for Open CASCADE (260 characters or more, see
/// `flow::needs_local_copy`): the temporary folder, a name of six letters after `thumb_in_` (what the removal of leftovers
/// of a killed program recognises) and the file under a plain name. The folder is removed when this goes.
struct ShortCopy {
    dir: PathBuf,
    file: PathBuf,
}

impl ShortCopy {
    fn make(src: &Path) -> Result<ShortCopy, String> {
        let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let base = std::env::temp_dir();
        // `create_dir` fails when the name is taken, so two programs never share a folder
        for _ in 0..1000 {
            let n = (u64::from(std::process::id()).wrapping_mul(2_654_435_761) ^ cad_core::paths::temp_seq().wrapping_mul(40_503)) & 0xFF_FFFF;
            let dir = base.join(format!("{}{n:06x}", cad_2d::dwg::PREFIX_THUMB_IN));
            match fs::create_dir(&dir) {
                Ok(()) => {
                    let file = dir.join(format!("model{ext}"));
                    let copy = ShortCopy { dir, file };
                    // the marker lets the window remove the folder if this process is killed (Explorer's timeout)
                    cad_2d::dwg::mark_private_dir(&copy.dir).map_err(|e| e.to_string())?;
                    // a failed copy removes the folder through `Drop`
                    fs::copy(src, &copy.file).map_err(|e| e.to_string())?;
                    return Ok(copy);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("no folder for the copy".to_string())
    }
}

impl Drop for ShortCopy {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// What a renderer says about a model or scene that did not fit into the memory of the graphics card (`take_warning`) is an
/// error here: the picture would be a blank or partial one, and Explorer keeps what a thumbnail handler hands over under the
/// name of the file. No picture at all (exit code 1) shows the icon of the file instead and asks again next time.
fn no_gpu_warning(warning: Option<String>) -> Result<(), String> {
    match warning {
        None => Ok(()),
        Some(w) => Err(w),
    }
}

fn render_3d(src: &Path, size: u32) -> Result<Vec<u8>, String> {
    let copy = if crate::flow::needs_local_copy(src) { Some(ShortCopy::make(src)?) } else { None };
    let (_doc, tree, protos) = load_model(copy.as_ref().map(|c| c.file.as_path()).unwrap_or(src))?;
    let g = gpu()?;
    let mut r = Renderer3d::new(&g.device, FORMAT).with_queue(&g.queue);
    r.set_model(&protos, &tree);
    no_gpu_warning(r.take_warning())?;
    let png = r.thumbnail_png(size).map_err(|e| e.to_string())?;
    // the buffers of the frame itself are made under error scopes as well
    no_gpu_warning(r.take_warning())?;
    Ok(png)
}

/// The drawing of a thumbnail, with the limits of its scene: small and fixed, for an image of 1024 pixels at the most, and lower
/// when the process has little memory left (`cad_2d::memory::thumbnail_out_limits`). The limits of the window (the memory the
/// computer has free, up to 2.9 GB) are not for a process that the sandbox of the shell extension keeps within 2 GiB: a drawing
/// of 50 000 texts took more than that and the process was ended by the system. The detail of texts and hatches is reduced
/// (letters become bars) long before the image shows it, the strokes are as ever.
fn load_drawing(src: &Path) -> Result<cad_2d::Drawing, String> {
    // no local copy is made for a thumbnail and no picture is looked up (`render_2d`): `original_dir` stays empty
    let opts = LoadOptions { dxf_cache: None, third_party_dir: third_party_dir(), fonts: FontOptions::default(), original_dir: None };
    let mut drawing = cad_2d::load(src, &opts, &mut |_| {}).map_err(|e| e.to_string())?;
    // what the drawing has taken is off the limit of the job: measured after it was read
    let (soft, max) = cad_2d::memory::thumbnail_out_limits(cad_2d::memory::process_headroom_bytes());
    drawing.set_out_limits(soft, max);
    Ok(drawing)
}

fn render_2d(src: &Path, size: u32, gpu_limit: Option<u64>) -> Result<Vec<u8>, String> {
    let drawing = load_drawing(src)?;
    let layout = drawing.default_layout();
    // white paper, images are not looked up (they would touch other files)
    let (scene, _space) = crate::scene2d::build(&drawing, &layout, true, false).map_err(|e| e.to_string())?;
    let g = gpu()?;
    let mut r = Renderer2d::new(&g.device, FORMAT).with_queue(&g.queue);
    if gpu_limit.is_some() {
        r.set_simulated_buffer_limit(gpu_limit);
    }
    // the scene is given up while it is uploaded (a big drawing is not held twice)
    r.set_scene_owned(scene, &[], &[], None, false);
    no_gpu_warning(r.take_warning())?;
    let png = r.thumbnail_png(size).map_err(|e| e.to_string())?;
    // the buffers of the frame itself are made under error scopes as well
    no_gpu_warning(r.take_warning())?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_clamped_like_thumbs_run() {
        assert_eq!(clamp_size("256"), Some(256));
        assert_eq!(clamp_size("4"), Some(16));
        assert_eq!(clamp_size("0"), Some(16));
        assert_eq!(clamp_size("-5"), Some(16));
        assert_eq!(clamp_size("5000"), Some(1024));
        assert_eq!(clamp_size(" 32 "), Some(32));
        assert_eq!(clamp_size("abc"), None);
        assert_eq!(clamp_size(""), None);
        assert_eq!(clamp_size("12.5"), None);
    }

    /// A drawing of `n` texts of 17 letters in rows of 30.
    fn texts_dxf(n: usize) -> String {
        let mut s = String::from("  0
SECTION
  2
ENTITIES
");
        for i in 0..n {
            s.push_str(&format!(
                "  0
TEXT
  8
0
 10
{}
 20
{}
 30
0.0
 40
2.5
  1
Pozice 12 M12x1,5
",
                (i % 30) as f64 * 40.0,
                (i / 30) as f64 * 6.0
            ));
        }
        s.push_str("  0
ENDSEC
  0
EOF
");
        s
    }

    /// The thumbnail role sizes its scene by its own small limits and not by the memory that the computer has free: a drawing
    /// of texts has less detail in it than in the window, and a scene of the limits of the window does not fit the 2 GiB of the
    /// sandbox of the shell extension (which is what ended the process of a drawing of 50 000 texts).
    #[test]
    fn the_scene_of_a_thumbnail_has_the_small_limits_of_a_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("texty.dxf");
        // 1 200 texts are 4.8 million vertices in full detail: over the 2 million of a thumbnail, far under what the window takes
        fs::write(&src, texts_dxf(1200)).unwrap();
        let drawing = load_drawing(&src).expect("drawing");
        let layout = drawing.default_layout();
        let (scene, _) = crate::scene2d::build(&drawing, &layout, true, false).expect("scene");
        let verts: usize = scene.batches.iter().map(|b| b.lines.len() + b.fills.len()).sum();
        let (soft, max) = cad_2d::memory::thumbnail_out_limits(cad_2d::memory::process_headroom_bytes());
        assert!(verts as u64 <= max, "{verts} of {max}");
        assert!(!drawing.scene_notes(&layout).is_empty(), "the detail of the texts was reduced from {soft} vertices");
        // the same drawing in the window (no limits set) is in full detail
        let opts = LoadOptions { dxf_cache: None, third_party_dir: None, fonts: FontOptions::default(), original_dir: None };
        let window = cad_2d::load(&src, &opts, &mut |_| {}).expect("drawing");
        let (full, _) = crate::scene2d::build(&window, &layout, true, false).expect("scene");
        let full_verts: usize = full.batches.iter().map(|b| b.lines.len() + b.fills.len()).sum();
        assert!(window.scene_notes(&layout).is_empty());
        assert!(full_verts > verts + 1_000_000, "{full_verts} in the window, {verts} in a thumbnail");
    }

    /// A scene that the graphics card cannot take is an error and no picture: Explorer would keep a blank PNG as the thumbnail.
    #[test]
    fn a_scene_that_does_not_fit_the_graphics_card_gives_no_thumbnail() {
        if GpuContext::new_headless().is_err() {
            eprintln!("SKIP: no graphics adapter");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("deska.dxf");
        fs::write(&src, crate::testgen::dxf_drawing()).unwrap();
        // an ordinary device: the thumbnail is made
        let good = dir.path().join("good.png");
        assert_eq!(run_with(&src, &good, "64", None), 0);
        assert!(good.exists());
        // a device that takes no buffer of this size: exit code 1 and nothing written
        let bad = dir.path().join("bad.png");
        assert_eq!(run_with(&src, &bad, "64", Some(16)), 1);
        assert!(!bad.exists(), "a blank picture was written");
        let e = render_2d(&src, 64, Some(16)).unwrap_err();
        assert_eq!(e, cad_render::gpubuf::GPU_BUFFER_WARNING);
    }

    #[test]
    fn a_graphics_warning_is_an_error_and_no_warning_is_none() {
        assert_eq!(no_gpu_warning(None), Ok(()));
        assert_eq!(no_gpu_warning(Some("W".to_string())), Err("W".to_string()));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a.DXF")), Kind::Drawing);
        assert_eq!(kind_of(Path::new("a.dwg")), Kind::Drawing);
        assert_eq!(kind_of(Path::new("a.step")), Kind::Model);
        assert_eq!(kind_of(Path::new("a.stl")), Kind::Model);
    }

    #[test]
    fn missing_or_oversized_input_fails_without_output() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("o.png");
        assert_eq!(run(&dir.path().join("none.step"), &out, "64"), 1);
        assert!(!out.exists());
        assert_eq!(run(&dir.path().join("none.step"), &out, "bad"), 1);
    }

    #[test]
    fn external_reference_is_refused_before_anything_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("evil.step");
        fs::write(&src, b"ISO-10303-21;\nDATA;\n#1=DOCUMENT_FILE('\\\\\\\\server\\\\share\\\\x');\nENDSEC;\n").unwrap();
        let out = dir.path().join("o.png");
        assert_eq!(run(&src, &out, "64"), 1);
        assert!(!out.exists());
    }
}
