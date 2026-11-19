//! DXF / DWG drawings for the CAD viewer: parsing, DWG conversion, scene building with z-order preserving
//! batching, layer visibility, picking, snapping and drawing information.
//! See docs/rust-port/ARCHITECTURE.md (crate `cad-2d`) and docs/rust-port/SPEC.md sections 2, 8, 9 and 10.
//!
//! Units of `DrawBatch.width`: the width of the cosmetic (screen space) pen in pixels, `lineweight / 0.3527`;
//! 0 means a hairline (1 pixel). A zero length segment (`lines` entry with two equal points) is a dimensionless
//! point and has to be drawn as a dot.
pub mod bspline;
pub mod budget;
pub mod clip;
pub mod colors;
pub mod ctl;
pub mod dwg;
pub mod dxf;
pub mod font;
pub mod frontend;
pub mod geom;
pub mod hatchdraw;
pub mod info;
pub mod levels;
pub mod memory;
pub mod mtext;
pub mod props;
pub mod raster;
pub mod scene;
pub mod tess;
pub mod textdraw;

pub use budget::DrawingTooComplex;
pub use ctl::{Ctl, Halt, SCENE_TIME_LIMIT};
pub use cad_types::{DrawBatch, DrawImage, Layer, LayerId, Scene2d};
pub use font::FontOptions;
pub use props::Role;
pub use scene::{EntityId, OriginMode};

/// Tooltip of a layer that is switched off or frozen in the file.
pub const LAYER_OFF_TOOLTIP: &str = "Ve výkresu vypnutá / zmrazená hladina";
/// Heading of the rows of the picked part inside a block reference (`EntityInfo::block_rows`).
pub const IN_BLOCK_HEADING: &str = "V bloku";

use frontend::Frontend;
use scene::{Built, Container, LayerVis};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Options of `load`.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    /// Cache file of the DXF converted from a DWG (`<cache>/<key>.dxf`); an existing file is used instead of converting.
    pub dxf_cache: Option<PathBuf>,
    /// Folder with `libredwg/dwg2dxf.exe` (the repository `third_party` folder or the installed one).
    pub third_party_dir: Option<PathBuf>,
    pub fonts: FontOptions,
    /// Folder of the drawing the user opened, when the file given to `load` is a local copy of it (portable mode copies
    /// the drawing from the Drive into the cache first): raster pictures of IMAGE entities are looked up there, next to
    /// the original, and not next to the copy, where they never are. `None`: next to the file that is loaded. The rules
    /// for the picture names are the same either way (`raster::resolve_path`: a remote name is refused, and a drawing
    /// with one is drawn with frames only).
    pub original_dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// Converter missing / failed (Czech message of the viewer).
    #[error("{0}")]
    Convert(String),
    #[error("Soubor se nepodařilo přečíst: {0}")]
    Io(String),
    #[error("Soubor není platný výkres DXF.")]
    Invalid,
    /// `load_cancellable`: the caller cancelled the load.
    #[error("Načítání bylo zrušeno.")]
    Cancelled,
}

/// Why a cancellable scene build gave no scene.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SceneError {
    /// The layout is too complex to draw (the Czech message of the viewer).
    #[error("{0}")]
    TooComplex(DrawingTooComplex),
    /// The caller set the cancel flag: the build stopped and nothing was kept.
    #[error("Vykreslování bylo zrušeno.")]
    Cancelled,
}

impl From<DrawingTooComplex> for SceneError {
    fn from(e: DrawingTooComplex) -> Self {
        SceneError::TooComplex(e)
    }
}

/// Options of a scene.
#[derive(Clone, Copy, Debug)]
pub struct SceneOptions {
    /// White paper instead of the dark model space background (colours follow: ACI 7 is black).
    pub light_bg: bool,
    /// `OriginMode::Center`: batch coordinates are relative to `Scene2d::origin` (keeps f32 precision for drawings far
    /// from the origin).
    pub origin: OriginMode,
    /// Raster pictures of IMAGE entities are read from files next to the drawing and drawn (as the Python viewer did).
    /// A picture that cannot be read is shown as its file name. False (thumbnails): frames only, no file is read. A
    /// drawing with a remote (UNC, URL) picture name is drawn with frames only, and no file name either.
    pub images: bool,
}

impl Default for SceneOptions {
    fn default() -> Self {
        SceneOptions { light_bg: false, origin: OriginMode::World, images: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutSummary {
    pub name: String,
    /// Number of drawable entities (the number shown in the layout combo box).
    pub drawable: usize,
}

/// Document level facts for the information panel.
#[derive(Clone, Debug, Default)]
pub struct DrawingInfo {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub version_code: String,
    pub release: String,
    pub units: String,
    pub layers: usize,
    pub blocks: usize,
    /// "mezipaměť", "ODA File Converter" or "LibreDWG" for DWG files.
    pub converter: Option<String>,
    /// Repaired structure errors ("Opraveno chyb").
    pub errors: u32,
    /// Inventor drawing views converted to block references.
    pub inventor: u32,
    /// Image file names that are blocked because they are not plain local paths.
    pub remote_images: Vec<String>,
}

/// One row of the information panel; `warning` rows are drawn in orange.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InfoRow {
    pub label: String,
    pub value: String,
    pub warning: bool,
}

/// What a click on the drawing hit.
#[derive(Clone, Debug)]
pub struct EntityInfo {
    /// Top level entity (selection is made of all its primitives).
    pub id: EntityId,
    pub dxftype: String,
    pub layer: String,
    pub handle: String,
    /// Rows of the top level entity (`entity_rows`).
    pub rows: Vec<(String, String)>,
    /// Rows of the picked part when it lies inside a block reference (without Handle); empty otherwise.
    /// The panel shows them below the heading "V bloku".
    pub block_rows: Vec<(String, String)>,
    pub bounds: Option<([f64; 2], [f64; 2])>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapPoint {
    pub pos: [f64; 2],
    /// End point of a plain LINE (snapped only when no path vertex is near).
    pub line_end: bool,
}

/// (layout, white paper, origin, images). The layouts of paper space have the same white paper and black pens whatever the
/// option "bílé pozadí" says: their key has no white-paper flag, so a change of the option is no new scene for them.
type CacheKey = (usize, bool, OriginMode, bool);

/// The batches and pictures of a built scene: what the viewer hands over to the renderer (taken out of the cache entry,
/// not copied) and what a `Built` does not need for its queries.
#[derive(Clone)]
struct Geometry {
    batches: Vec<DrawBatch>,
    images: Vec<DrawImage>,
}

/// The one build that is kept: the active layout. `geometry` is `None` once the viewer took it.
struct Cached {
    key: CacheKey,
    built: Arc<Built>,
    geometry: Option<Geometry>,
}

struct State {
    layout: Option<usize>,
    light_bg: bool,
    origin: OriginMode,
    images: bool,
    overrides: HashMap<LayerId, bool>,
    /// At most one entry. Every build of a scene (a layout, a background, an origin) is kept as long as it is the active one
    /// and dropped when another is made active: two or four of them held two to four times the memory of the drawing
    /// (3.5 GB at rest and 6.1 GB after one change of the background for 25 000 texts).
    cache: Vec<Cached>,
}

pub struct Drawing {
    doc: dxf::Document,
    path: PathBuf,
    /// Folder the pictures of IMAGE entities are looked up in: the folder of `path`, or `LoadOptions::original_dir`.
    doc_dir: Option<PathBuf>,
    size_bytes: u64,
    converter: Option<String>,
    fonts: FontOptions,
    layout_names: Vec<String>,
    layout_keys: Vec<String>,
    summaries: Vec<LayoutSummary>,
    budget: Mutex<HashMap<usize, Result<usize, DrawingTooComplex>>>,
    state: Mutex<State>,
    /// (path elements, primitives) one scene may record (`scene::MAX_SCENE_ELEMS`, `MAX_SCENE_PRIMS`).
    pub(crate) scene_limits: (usize, usize),
    /// (output vertices up to which the detail stays full, most output vertices) of a scene at the most
    /// (`scene::SOFT_OUT_VERTS`, `MAX_OUT_VERTS`); the memory the computer has free lowers them (`memory::out_limits_for`).
    pub(crate) out_limits: (u64, u64),
    /// Path elements that dashed lines may record together before the rest is drawn solid (`scene::DASH_SOFT_ELEMS`).
    pub(crate) dash_limit: usize,
    /// Time each of the two stages of a scene may take, drawing the entities and making their geometry
    /// (`ctl::SCENE_TIME_LIMIT`).
    pub(crate) time_limit: Duration,
}

fn looks_empty(doc: &dxf::Document) -> bool {
    doc.layout_entities.values().all(|v| v.is_empty()) && doc.blocks.is_empty() && doc.layers.is_empty() && doc.header.values.is_empty()
}

/// Reads a DXF or DWG file. DWG files are converted first (see `LoadOptions`); `progress` receives the Czech
/// status lines "Převádím DWG na DXF…" and "Čtu DXF…".
pub fn load(path: &Path, opts: &LoadOptions, progress: &mut dyn FnMut(&str)) -> Result<Drawing, LoadError> {
    load_cancellable(path, opts, progress, None)
}

/// `load` that stops the DWG converter process when `cancel` is set (the result is `LoadError::Cancelled`). Without a
/// converter run (DXF files, a cached conversion) the flag is only looked at before the file is parsed; parsing itself
/// is not interrupted.
pub fn load_cancellable(path: &Path, opts: &LoadOptions, progress: &mut dyn FnMut(&str), cancel: Option<&AtomicBool>) -> Result<Drawing, LoadError> {
    let is_cancelled = || cancel.is_some_and(|c| c.load(Ordering::Relaxed));
    let is_dwg = path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("dwg")).unwrap_or(false);
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if is_cancelled() {
        return Err(LoadError::Cancelled);
    }
    let (bytes, converter) = if !is_dwg {
        progress("Čtu DXF…");
        (std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?, None)
    } else {
        let c = dwg::convert_cached_cancellable(path, opts.dxf_cache.as_deref(), opts.third_party_dir.as_deref(), progress, cancel)
            .map_err(|e| if e == dwg::CANCELLED { LoadError::Cancelled } else { LoadError::Convert(e) })?;
        progress("Čtu DXF…");
        let b = std::fs::read(&c.dxf).map_err(|e| LoadError::Io(e.to_string()));
        let conv = c.converter.clone();
        drop(c); // removes the private temporary folder
        (b?, Some(conv))
    };
    if is_cancelled() {
        return Err(LoadError::Cancelled);
    }
    let doc = dxf::load_document(&bytes);
    if looks_empty(&doc) {
        return Err(LoadError::Invalid);
    }
    let mut drawing = Drawing::from_document(doc, path.to_path_buf(), size_bytes, converter, opts.fonts.clone());
    if let Some(dir) = opts.original_dir.as_ref().filter(|d| !d.as_os_str().is_empty()) {
        drawing.doc_dir = Some(dir.clone());
    }
    Ok(drawing)
}

/// Builds a drawing from DXF bytes (tests, tools).
pub fn load_bytes(bytes: &[u8], fonts: FontOptions) -> Result<Drawing, LoadError> {
    let doc = dxf::load_document(bytes);
    if looks_empty(&doc) {
        return Err(LoadError::Invalid);
    }
    Ok(Drawing::from_document(doc, PathBuf::new(), bytes.len() as u64, None, fonts))
}

impl Drawing {
    fn from_document(doc: dxf::Document, path: PathBuf, size_bytes: u64, converter: Option<String>, fonts: FontOptions) -> Drawing {
        let mut layout_names = vec!["Model".to_string()];
        let mut layout_keys = vec!["model".to_string()];
        let mut paper: Vec<&dxf::LayoutDef> = doc.layouts.iter().filter(|l| !l.is_model).collect();
        paper.sort_by_key(|l| l.tab_order);
        for l in paper {
            layout_names.push(l.name.clone());
            layout_keys.push(l.name.to_lowercase());
        }
        let summaries = layout_keys
            .iter()
            .zip(&layout_names)
            .map(|(k, n)| LayoutSummary {
                name: n.clone(),
                drawable: doc.layout_entities.get(k).map(|v| v.iter().filter(|e| info::is_drawable(e)).count()).unwrap_or(0),
            })
            .collect();
        Drawing {
            doc,
            doc_dir: path.parent().map(|p| p.to_path_buf()),
            path,
            size_bytes,
            converter,
            fonts,
            layout_names,
            layout_keys,
            summaries,
            budget: Mutex::new(HashMap::new()),
            state: Mutex::new(State { layout: None, light_bg: false, origin: OriginMode::World, images: true, overrides: HashMap::new(), cache: Vec::new() }),
            scene_limits: (scene::MAX_SCENE_ELEMS, scene::MAX_SCENE_PRIMS),
            out_limits: (scene::SOFT_OUT_VERTS, scene::MAX_OUT_VERTS),
            dash_limit: scene::DASH_SOFT_ELEMS,
            time_limit: SCENE_TIME_LIMIT,
        }
    }

    /// Output vertices up to which shapes are drawn in full detail, and the most a scene may have before it is refused
    /// (defaults `scene::SOFT_OUT_VERTS` and `MAX_OUT_VERTS`). The memory the computer has free lowers them further, never
    /// raises them. The thumbnail role sets its own small ones (`memory::thumbnail_out_limits`); tests and measurements of the
    /// reduced detail set others.
    pub fn set_out_limits(&mut self, soft: u64, max: u64) {
        self.out_limits = (soft, max);
    }

    /// Changes the time each of the two stages of a scene may take (default `SCENE_TIME_LIMIT`): going through the entities,
    /// and making the geometry of what was recorded. A scene that runs out of it is cut off where it is, shows what has
    /// been drawn, and says so in `scene_notes`.
    pub fn set_scene_time_limit(&mut self, limit: Duration) {
        self.time_limit = limit;
    }

    /// The parsed document (read only).
    pub fn document(&self) -> &dxf::Document {
        &self.doc
    }

    pub fn layouts(&self) -> &[LayoutSummary] {
        &self.summaries
    }

    /// "Model" when it has drawable entities, else the first paper space layout with some, else "Model".
    pub fn default_layout(&self) -> String {
        if self.summaries.first().map(|s| s.drawable > 0).unwrap_or(false) {
            return "Model".into();
        }
        for s in self.summaries.iter().skip(1) {
            if s.drawable > 0 {
                return s.name.clone();
            }
        }
        "Model".into()
    }

    fn layout_index(&self, layout: &str) -> usize {
        self.layout_names
            .iter()
            .position(|n| n == layout)
            .or_else(|| self.layout_names.iter().position(|n| n.eq_ignore_ascii_case(layout)))
            .unwrap_or(0)
    }

    /// Entities of the layout (file order).
    pub fn layout_entities(&self, layout: &str) -> &[dxf::Entity] {
        let i = self.layout_index(layout);
        self.doc.layout_entities.get(&self.layout_keys[i]).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Budget check of a layout (`DrawingTooComplex` for nested block bombs and cyclic blocks); the number of
    /// entities the layout expands to.
    pub fn check_budget(&self, layout: &str) -> Result<usize, DrawingTooComplex> {
        let i = self.layout_index(layout);
        if let Some(r) = self.budget.lock().ok().and_then(|b| b.get(&i).cloned()) {
            return r;
        }
        let r = budget::expanded_entity_count(&self.doc, self.layout_entities(layout), i == 0, budget::MAX_EXPANDED_ENTITIES);
        if let Ok(mut b) = self.budget.lock() {
            b.insert(i, r.clone());
        }
        r
    }

    fn key_of(idx: usize, opts: &SceneOptions) -> CacheKey {
        (idx, opts.light_bg && idx == 0, opts.origin, opts.images)
    }

    /// Output vertex limits of the next scene: the configured ones, lower when the computer has little memory free.
    fn effective_out_limits(&self) -> (u64, u64) {
        let (soft, max) = memory::out_limits_for(memory::available_physical_bytes());
        (self.out_limits.0.min(soft), self.out_limits.1.min(max))
    }

    /// Builds the scene of a layout from the beginning. The result is the build without its batches and pictures (what is
    /// needed to answer questions about the scene) and its geometry (what is drawn); nothing is stored.
    fn build_fresh(&self, idx: usize, opts: &SceneOptions, cancel: Option<&Arc<AtomicBool>>) -> Result<(Arc<Built>, Geometry), SceneError> {
        let name = self.layout_names[idx].clone();
        let ctl = Ctl::new(cancel.cloned(), Instant::now().checked_add(self.time_limit));
        if ctl.is_cancelled() {
            return Err(SceneError::Cancelled);
        }
        let layout_key = self.layout_keys[idx].clone();
        let (soft_out, max_out) = self.effective_out_limits();
        let mut fe = Frontend::new(&self.doc, opts.light_bg, idx == 0, self.fonts.clone(), self.layout_keys.clone());
        // an element makes one output vertex at the least: more of them than the scene may have vertices is refused while it is
        // recorded, not after its geometry (a vertex is 8 bytes there, an element 17) was made; the dashes may take half of it
        fe.rec.set_limits(self.scene_limits.0.min(max_out.min(usize::MAX as u64) as usize), self.scene_limits.1);
        fe.rec.set_out_limits(soft_out, max_out);
        fe.rec.set_dash_limit(self.dash_limit.min((max_out / 2).min(usize::MAX as u64) as usize));
        fe.ctl = ctl.clone();
        fe.images = opts.images && info::remote_images(&self.doc).is_empty();
        fe.doc_dir = self.doc_dir.clone();
        fe.draw_layout(&layout_key, idx as u32);
        if fe.rec.halt() == Some(Halt::Cancelled) {
            return Err(SceneError::Cancelled);
        }
        if fe.rec.is_over() {
            // the backstop behind the entity budget: the scene is incomplete and is refused. The limit depends on the memory
            // that is free now, so the refusal is not remembered: the next scene of this layout works it out again.
            return Err(budget::scene_too_large(fe.rec.max_elems()).into());
        }
        let mut notes = fe.notes();
        let mut timed_out = fe.rec.halt() == Some(Halt::TimedOut);
        let bg = fe.ctx.bg;
        let bg_sensitive = fe.bg_sensitive;
        let n_layers = self.doc.layer_names.len();
        let file_visible: Vec<bool> = (0..n_layers as u32).map(|i| fe.ctx.layer_visible_in_file(i)).collect();
        let rec = std::mem::take(&mut fe.rec);
        drop(fe);
        let ents = self.doc.layout_entities.get(&layout_key).map(|v| v.as_slice()).unwrap_or(&[]);
        let mut counts = vec![0u32; n_layers];
        for e in ents {
            if let Some(c) = counts.get_mut(e.c.layer as usize) {
                *c += 1;
            }
        }
        let layers: Vec<Layer> = (0..n_layers as u32)
            .map(|i| Layer {
                id: i,
                name: self.doc.layer_names.name(i).to_string(),
                entities: counts[i as usize],
                visible: file_visible[i as usize],
                off_in_file: !file_visible[i as usize],
            })
            .collect();
        // the geometry of what was recorded is made in its own time limit: a drawing that used up the time of the first
        // stage (it stops there and is shown as far as it got) still has to be turned into batches
        let make_ctl = Ctl::new(cancel.cloned(), Instant::now().checked_add(self.time_limit));
        let mut built = Built::build_ctl(rec, 1.0, layers, &file_visible, &name, self.layout_names.clone(), bg, opts.origin, &make_ctl);
        if built.halt == Some(Halt::Cancelled) {
            return Err(SceneError::Cancelled);
        }
        if built.over_out {
            // not remembered either (see above): with more memory free the same scene may fit
            return Err(budget::scene_too_large(max_out.min(usize::MAX as u64) as usize).into());
        }
        timed_out |= built.halt == Some(Halt::TimedOut);
        notes.extend(std::mem::take(&mut built.notes));
        if timed_out {
            notes.push(budget::note_timeout(self.time_limit.as_secs()));
        }
        built.notes = notes;
        built.bg_sensitive = bg_sensitive;
        // what is drawn is handed over apart from what is asked about: the batches are by far the biggest part
        let geometry = Geometry { batches: std::mem::take(&mut built.scene.batches), images: std::mem::take(&mut built.scene.images) };
        Ok((Arc::new(built), geometry))
    }

    /// The build of a layout for the queries of the active drawing (`pick`, `highlight`, ...): the cached one, else a new
    /// one that replaces it. Its geometry is not kept.
    fn build(&self, idx: usize, opts: &SceneOptions, cancel: Option<&Arc<AtomicBool>>) -> Result<Arc<Built>, SceneError> {
        let name = self.layout_names[idx].clone();
        self.check_budget(&name)?;
        let key = Self::key_of(idx, opts);
        // for questions about the scene the build of the other background does as well, unless a picture was blended with it:
        // only the colours of the foreground and the background differ, and those are drawn from `batch_roles`
        let same = |e: &Cached| e.key == key || (e.key.0 == key.0 && e.key.2 == key.2 && e.key.3 == key.3 && !e.built.bg_sensitive);
        if let Some(b) = self.state.lock().ok().and_then(|s| s.cache.iter().find(|e| same(e)).map(|e| e.built.clone())) {
            return Ok(b);
        }
        // the old build is not needed any more when this one is there: it is dropped first
        if let Ok(mut s) = self.state.lock() {
            s.cache.clear();
        }
        let (built, _geometry) = self.build_fresh(idx, opts, cancel)?;
        if let Ok(mut s) = self.state.lock() {
            s.cache.clear();
            s.cache.push(Cached { key, built: built.clone(), geometry: None });
        }
        Ok(built)
    }

    fn active(&self) -> Option<(Arc<Built>, LayerVis)> {
        let (idx, opts, overrides) = {
            let s = self.state.lock().ok()?;
            (s.layout?, SceneOptions { light_bg: s.light_bg, origin: s.origin, images: s.images }, s.overrides.clone())
        };
        let built = self.build(idx, &opts, None).ok()?;
        let vis = Self::effective_vis(&built, &overrides);
        Some((built, vis))
    }

    fn effective_vis(built: &Built, overrides: &HashMap<LayerId, bool>) -> LayerVis {
        LayerVis(built.scene.layers.iter().map(|l| overrides.get(&l.id).copied().unwrap_or(!l.off_in_file)).collect())
    }

    /// Scene of a layout with the current background and layer visibility (the layout becomes the active one for
    /// `pick`, `snap_points` and `highlight`). Layouts that are too complex give an empty scene; see `scene_checked`.
    pub fn scene(&self, layout: &str) -> Scene2d {
        match self.scene_checked(layout) {
            Ok(s) => s,
            Err(_) => Scene2d { layout: layout.to_string(), layouts: self.layout_names.clone(), ..Default::default() },
        }
    }

    pub fn scene_checked(&self, layout: &str) -> Result<Scene2d, DrawingTooComplex> {
        let (light, origin, images) = self.state.lock().map(|s| (s.light_bg, s.origin, s.images)).unwrap_or((false, OriginMode::World, true));
        self.scene_with(layout, &SceneOptions { light_bg: light, origin, images })
    }

    pub fn scene_with(&self, layout: &str, opts: &SceneOptions) -> Result<Scene2d, DrawingTooComplex> {
        match self.scene_inner(layout, opts, None, false) {
            Ok(s) => Ok(s),
            Err(SceneError::TooComplex(e)) => Err(e),
            // no cancel flag was given, so this does not happen; the message is the one of a cancelled build
            Err(SceneError::Cancelled) => Err(DrawingTooComplex(SceneError::Cancelled.to_string())),
        }
    }

    /// `scene_with` that stops as soon as `cancel` is set (`SceneError::Cancelled`, nothing is kept; the flag is looked at
    /// between entities, inside long clipping and hatching loops and between the pieces the scene is assembled from, so
    /// it takes effect within a fraction of a second). Layout, layer and background changes of the drawing are made only
    /// by a build that finished.
    ///
    /// This is the call of the viewer: the batches and pictures are handed over, not copied. The drawing keeps what it
    /// needs for `pick`, `highlight` and `snap`, not the geometry, so the scene exists once in the program (then on the
    /// graphics card). Asking for the same scene again builds it again; layers are switched with `visible_batches`
    /// instead.
    pub fn scene_cancellable(&self, layout: &str, opts: &SceneOptions, cancel: &Arc<AtomicBool>) -> Result<Scene2d, SceneError> {
        self.scene_inner(layout, opts, Some(cancel), true)
    }

    /// `take`: the geometry goes to the caller and the drawing keeps none; else the drawing keeps a copy of it (the scene
    /// can be asked for again; thumbnails, tests).
    fn scene_inner(&self, layout: &str, opts: &SceneOptions, cancel: Option<&Arc<AtomicBool>>, take: bool) -> Result<Scene2d, SceneError> {
        let idx = self.layout_index(layout);
        let name = self.layout_names[idx].clone();
        self.check_budget(&name)?;
        let key = Self::key_of(idx, opts);
        // the cached build of this scene, with its geometry when it still has it
        let cached = self.state.lock().ok().and_then(|mut s| {
            let e = s.cache.iter_mut().find(|e| e.key == key)?;
            let geometry = if take { e.geometry.take() } else { e.geometry.clone() }?;
            Some((e.built.clone(), geometry))
        });
        let (built, geometry) = match cached {
            Some((b, g)) => (b, g),
            None => {
                // a build without geometry (it was handed over before) is built again; it is dropped first, so that the
                // old and the new one are not in memory together
                if let Ok(mut s) = self.state.lock() {
                    if s.cache.iter().any(|e| e.key == key) {
                        s.cache.clear();
                    }
                }
                self.build_fresh(idx, opts, cancel)?
            }
        };
        let mut sc = built.scene.clone(); // layers, bounds and the rest: the batches and pictures are not in it
        if let Ok(mut s) = self.state.lock() {
            s.layout = Some(idx);
            s.light_bg = opts.light_bg;
            s.origin = opts.origin;
            s.images = opts.images;
            // the active build replaces whatever was kept; its geometry is kept only for callers that do not take it
            s.cache.retain(|e| e.key == key);
            let kept = if take { None } else { Some(geometry.clone()) };
            match s.cache.iter_mut().find(|e| e.key == key) {
                Some(e) => e.geometry = kept,
                None => s.cache.push(Cached { key, built: built.clone(), geometry: kept }),
            }
            for l in sc.layers.iter_mut() {
                l.visible = s.overrides.get(&l.id).copied().unwrap_or(!l.off_in_file);
            }
        }
        sc.batches = geometry.batches;
        sc.images = geometry.images;
        let vis = LayerVis(sc.layers.iter().map(|l| l.visible).collect());
        sc.bounds = built.shown_bounds(&vis);
        Ok(sc)
    }

    /// The colour of the pens that are the foreground of a layout (see `props::Role`): white on the dark model space, black on
    /// white paper, that is for "bílé pozadí" and in paper space.
    pub fn layout_foreground(&self, layout: &str, light_bg: bool) -> [u8; 3] {
        if light_bg || self.layout_index(layout) != 0 {
            [0, 0, 0]
        } else {
            [0xFF, 0xFF, 0xFF]
        }
    }

    /// The role of the colour of each batch of the active scene (`props::Role`), in the order of the batches. A batch of the
    /// role `Fg` or `Bg` is drawn in the foreground / background colour of the layout whatever colour the scene was built
    /// with: a change of the white background option is then a change of colours, not a new scene.
    pub fn batch_roles(&self) -> Vec<props::Role> {
        self.active().map(|(b, _)| b.batch_roles.clone()).unwrap_or_default()
    }

    /// Whether the colours of the active scene can be changed without making it again: false when a picture of it was
    /// blended with the background (the fade of an IMAGE), and when there is no scene.
    pub fn colors_follow_background(&self) -> bool {
        self.active().is_some_and(|(b, _)| !b.bg_sensitive)
    }

    /// Whether the option "bílé pozadí" changes the scene of a layout: the model space follows it, the layouts of paper space
    /// are white with black pens either way (a change of the option is then no new scene for them).
    pub fn light_option_matters(&self, layout: &str) -> bool {
        self.layout_index(layout) == 0
    }

    /// Background colour of a layout: the dark model space colour, white for "bílé pozadí" and always for the layouts of
    /// paper space (which are drawn with black pens on white paper whatever the option says).
    pub fn layout_background(&self, layout: &str, light_bg: bool) -> [u8; 3] {
        if light_bg || self.layout_index(layout) != 0 {
            [0xFF, 0xFF, 0xFF]
        } else {
            props::MODEL_SPACE_BG
        }
    }

    /// White paper (true) or the default background of the layout (model space dark, paper space white).
    pub fn set_light_background(&self, on: bool) {
        if let Ok(mut s) = self.state.lock() {
            s.light_bg = on;
        }
    }

    /// Background colour of the active layout.
    pub fn background_color(&self) -> [u8; 3] {
        self.active().map(|(b, _)| b.bg).unwrap_or([0x21, 0x28, 0x30])
    }

    /// Offset added to batch coordinates built with `OriginMode::Center`.
    pub fn scene_origin(&self) -> [f64; 2] {
        self.active().map(|(b, _)| b.origin).unwrap_or([0.0; 2])
    }

    // ------------------------------------------------------------------------------------------ layers
    pub fn set_layer_visible(&self, layer: LayerId, visible: bool) {
        if let Ok(mut s) = self.state.lock() {
            s.overrides.insert(layer, visible);
        }
    }

    /// Drops every user override: layers are shown as in the file.
    pub fn reset_layer_visibility(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.overrides.clear();
        }
    }

    /// "Zobrazit všechny hladiny": every layer on.
    pub fn show_all_layers(&self) {
        let ids: Vec<LayerId> = (0..self.doc.layer_names.len() as u32).collect();
        if let Ok(mut s) = self.state.lock() {
            for i in ids {
                s.overrides.insert(i, true);
            }
        }
    }

    pub fn layer_visible(&self, layer: LayerId) -> bool {
        let off = self.doc.layer(layer).map(|l| !(l.is_on() && !l.is_frozen())).unwrap_or(false);
        self.state.lock().ok().and_then(|s| s.overrides.get(&layer).copied()).unwrap_or(!off)
    }

    /// Which batches of the active scene are shown: a batch depends on every layer its primitives were drawn for
    /// (an entity on layer "0" inside a block follows the layer of the INSERT, a dimension its own layer and the
    /// layers of its parts).
    pub fn visible_batches(&self) -> Vec<bool> {
        match self.active() {
            Some((b, vis)) => b.batch_layers.iter().map(|d| vis.is_shown(d)).collect(),
            None => Vec::new(),
        }
    }

    /// Bounds of what is shown (`LayerMap.bounding_rect`).
    pub fn visible_bounds(&self) -> Option<([f64; 2], [f64; 2])> {
        let (b, vis) = self.active()?;
        b.shown_bounds(&vis)
    }

    /// Layers every batch of the active scene depends on.
    pub fn batch_layers(&self) -> Vec<Vec<LayerId>> {
        self.active().map(|(b, _)| b.batch_layers.clone()).unwrap_or_default()
    }

    /// Layer table: (name, colour, visible in file).
    pub fn layer_table(&self) -> Vec<(String, [u8; 3], bool)> {
        let light = self.state.lock().map(|s| s.light_bg).unwrap_or(false);
        info::layer_table(&self.doc, light)
    }

    /// Rows of the layer panel for the layer `name` in the active layout.
    pub fn layer_rows(&self, name: &str) -> Vec<(String, String)> {
        let layout = self.state.lock().ok().and_then(|s| s.layout).unwrap_or(0);
        let ents = self.doc.layout_entities.get(&self.layout_keys[layout]).map(|v| v.as_slice()).unwrap_or(&[]);
        info::layer_rows(&self.doc, ents, name)
    }

    // ------------------------------------------------------------------------------------------ info
    pub fn info(&self) -> DrawingInfo {
        let version = info::loaded_version(&self.doc.dxf_version_code);
        DrawingInfo {
            path: self.path.clone(),
            size_bytes: self.size_bytes,
            release: info::release_name(&version),
            version_code: version,
            units: info::units_name(self.doc.header.num("$INSUNITS").unwrap_or(0.0) as i32),
            layers: self.doc.layers.len(),
            blocks: self.doc.blocks.len(),
            converter: self.converter.clone(),
            errors: self.doc.errors,
            inventor: self.doc.inventor,
            remote_images: info::remote_images(&self.doc),
        }
    }

    /// Czech notes about what the scene of a layout leaves out or draws with less detail (dashes too fine to cut, glyph
    /// outlines of a huge number of texts simplified or, when even that does not fit the memory that is free, texts and
    /// hatches drawn as a box, hatches filled solid, a scene that ran out of time). Empty until the scene has been built.
    pub fn scene_notes(&self, layout: &str) -> Vec<String> {
        let idx = self.layout_index(layout);
        self.state
            .lock()
            .ok()
            .and_then(|s| s.cache.iter().rev().find(|e| e.key.0 == idx).map(|e| e.built.notes.clone()))
            .unwrap_or_default()
    }

    /// Rows of the drawing information panel for a layout (`show_drawing_info`); `bounds` are the drawing extents of
    /// the scene, `load_secs` the load time shown in the last row (None: no such row). The rows that are warnings (`warning`)
    /// are the first ones.
    pub fn info_rows(&self, layout: &str, bounds: Option<([f64; 2], [f64; 2])>, load_secs: Option<f64>) -> Vec<InfoRow> {
        let i = self.info();
        let ents = self.layout_entities(layout);
        let mut rows: Vec<InfoRow> = Vec::new();
        let mut row = |l: &str, v: String, w: bool| rows.push(InfoRow { label: l.to_string(), value: v, warning: w });
        row("Složka", i.path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(), false);
        row("Velikost", format!("{:.2} MB", i.size_bytes as f64 / 1024.0 / 1024.0), false);
        row("Verze", format!("{} ({})", i.release, i.version_code), false);
        row("Jednotky", i.units.clone(), false);
        row("Layout", layout.to_string(), false);
        row("Entit", ents.len().to_string(), false);
        row("Hladin / bloků", format!("{} / {}", i.layers, i.blocks), false);
        if let Some((mn, mx)) = bounds {
            if mx[0] > mn[0] && mx[1] > mn[1] {
                row("Rozměr výkresu", format!("{} × {} {}", info::fmt_p(mx[0] - mn[0], 2), info::fmt_p(mx[1] - mn[1], 2), i.units), false);
            }
        }
        let counts = info::type_counts(ents);
        row("Typy entit", info::types_text(&counts), false);
        let acis: usize = counts.iter().filter(|(t, _)| info::ACIS_TYPES.contains(&t.as_str())).map(|(_, n)| n).sum();
        if acis > 0 {
            row("Upozornění", format!("{}× 3D těleso (ACIS) – v 2D náhledu se nezobrazí", acis), true);
        }
        if !i.remote_images.is_empty() {
            row("Obrázky", format!("{}× obrázek ze síťové cesty nenačten (bezpečnost) – zobrazen jen rámeček", i.remote_images.len()), true);
        }
        if i.inventor > 0 {
            row("Inventor", format!("{} pohledů převedeno", i.inventor), false);
        }
        for note in self.scene_notes(layout) {
            row("Upozornění", note, true);
        }
        if let Some(c) = &i.converter {
            row("Převod DWG", c.clone(), false);
        }
        if i.errors > 0 {
            row("Opraveno chyb", i.errors.to_string(), false);
        }
        if let Some(t) = load_secs {
            row("Načteno za", format!("{:.1} s", t), false);
        }
        // The warnings (a drawing drawn with less detail, a 3D solid or a picture that is not shown) come first: the panel is
        // the lowest part of the left column, and a note below the list of the entity types is below the fold. Stable: the
        // warnings and the other rows each keep their order.
        rows.sort_by_key(|r| !r.warning);
        rows
    }

    // ------------------------------------------------------------------------------------------ picking
    /// Entity under `point` (drawing coordinates) within `tolerance` (half side of the pick square, 5 pixels in drawing
    /// units): the topmost visible one of the active layout.
    pub fn pick(&self, point: [f64; 2], tolerance: f64) -> Option<EntityInfo> {
        let (b, vis) = self.active()?;
        // batch coordinates are relative to the origin, primitives keep world coordinates
        let rect = [point[0] - tolerance, point[1] - tolerance, point[0] + tolerance, point[1] + tolerance];
        let pi = b.pick(&vis, rect)?;
        let p = b.rec.prims[pi as usize];
        let top = b.rec.tops[p.top as usize];
        let leaf = b.rec.leaves[p.leaf as usize];
        let top_e = self.entity(top)?;
        let rows = info::entity_rows(&self.doc, top_e, None);
        let mut block_rows = Vec::new();
        if leaf.id != top || leaf.attr.is_some() {
            if let Some(le) = self.leaf_entity(&leaf) {
                let xf = b.rec.xforms.get(leaf.xf as usize).copied();
                block_rows = info::entity_rows(&self.doc, le, xf.as_ref()).into_iter().filter(|(k, _)| k != "Handle").collect();
            }
        }
        let bounds = b.top_bounds(&vis, top).map(|r| ([r[0], r[1]], [r[2], r[3]]));
        Some(EntityInfo {
            id: top,
            dxftype: top_e.dxftype().to_string(),
            layer: self.doc.layer_names.name(top_e.c.layer).to_string(),
            handle: if top_e.c.handle != 0 { format!("{:X}", top_e.c.handle) } else { String::new() },
            rows,
            block_rows,
            bounds,
        })
    }

    /// The entity an id refers to.
    pub fn entity(&self, id: EntityId) -> Option<&dxf::Entity> {
        match id.container {
            Container::Layout(i) => self.doc.layout_entities.get(self.layout_keys.get(i as usize)?)?.get(id.index as usize),
            Container::Block(b) => {
                let key = self.doc.block_order.get(b as usize)?;
                self.doc.blocks.get(key)?.entities.get(id.index as usize)
            }
        }
    }

    fn leaf_entity(&self, leaf: &scene::Leaf) -> Option<&dxf::Entity> {
        let e = self.entity(leaf.id)?;
        match (leaf.attr, &e.d) {
            (Some(k), dxf::Data::Insert(i)) => i.attribs.get(k as usize),
            _ => Some(e),
        }
    }

    /// Orange selection overlay of a top level entity (batches in the coordinates of the active scene).
    pub fn highlight(&self, id: EntityId) -> Vec<DrawBatch> {
        match self.active() {
            Some((b, vis)) => b.highlight(&vis, id),
            None => Vec::new(),
        }
    }

    /// Bounds of the shown primitives of an entity (zoom to selection).
    pub fn entity_bounds(&self, id: EntityId) -> Option<([f64; 2], [f64; 2])> {
        let (b, vis) = self.active()?;
        b.top_bounds(&vis, id).map(|r| ([r[0], r[1]], [r[2], r[3]]))
    }

    // ------------------------------------------------------------------------------------------ snapping
    /// Snap candidates inside the rectangle: vertices of paths and polylines, and end points of plain lines.
    pub fn snap_points(&self, min: [f64; 2], max: [f64; 2]) -> Vec<SnapPoint> {
        let Some((b, vis)) = self.active() else { return Vec::new() };
        b.snap_vertices(&vis, [min[0], min[1], max[0], max[1]]).into_iter().map(|(_, p, line)| SnapPoint { pos: p, line_end: line }).collect()
    }

    /// Snap point nearest to the cursor: `rect` is the search rectangle in drawing coordinates (10 pixels around the
    /// cursor), `to_px` maps drawing coordinates to viewport pixels, `cursor_px` is the cursor in pixels. Path
    /// vertices win over line end points; the distance has to be below 10 pixels.
    pub fn snap(&self, rect: ([f64; 2], [f64; 2]), to_px: &dyn Fn([f64; 2]) -> [f64; 2], cursor_px: [f64; 2]) -> Option<[f64; 2]> {
        let (b, vis) = self.active()?;
        let r = [rect.0[0], rect.0[1], rect.1[0], rect.1[1]];
        for lines in [false, true] {
            if let Some((d, p)) = b.snap(&vis, r, to_px, cursor_px, lines) {
                if d < 100.0 {
                    return Some(p);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod image_tests;
#[cfg(test)]
mod tests;
