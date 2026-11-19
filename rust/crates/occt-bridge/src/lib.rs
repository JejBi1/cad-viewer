//! OCCT bridge: STEP / IGES / STL import, meshing, XBF hand-over, instance tree, mesh extraction, mass properties,
//! picked-entity measurement and the interference check. See docs/rust-port/ARCHITECTURE.md.
//!
//! The geometry kernel is reached through a small C++ layer (cxx). Policy lives on the Rust side: constants,
//! the deflection formula and its half-octave buckets, every Czech message, the measurement text. Behaviour
//! reference: `stepview/core.py` and `stepview/analysis.py`.
//!
//! Threading: a [`Document`] may be moved to another thread but not shared (`Send`, not `Sync`). OCCT objects are
//! reference counted atomically; reading STEP/IGES and XBF input/output are serialised inside the bridge, so
//! several documents can be loaded from different threads. OCCT recurses deeply on nested assemblies: run
//! loading on a thread with a generous stack (8 MiB or more).

mod ffi;
pub mod measure;

use std::cell::OnceCell;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use cad_types::{
    Aabb, EdgeSpan, EntityRef, FaceSpan, Interference, MassProps, ModelTree, NodeId, P3, ProtoId,
    ProtoMesh, Rgb, TreeNode,
};

use ffi::ffi as raw;

/// Test support: the next fault point of the C++ side on this thread raises an access violation once (0: off). The tests
/// of the crash guards use it (`raw::test_inject_fault`); nothing in the program calls it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn inject_fault(id: u32) {
    raw::test_inject_fault(id);
}

pub use measure::{
    Distance, Draw, Entity, LoopCircle, MeasureItem, MeasureKind, MeasureRow, Measurement,
    MeshLoop, MeshPatch, fmt_len, fmt_p, fmt_thousands, line_line_distance, relation, summary,
};

// ------------------------------------------------------------------ constants (core.py, analysis.py)

/// `MAX_INSTANCES`: more instances than this and the assembly is refused.
pub const MAX_INSTANCES: u64 = 300_000;
/// `AMPLIFY_MIN`: below this many instances the amplification check is not applied.
pub const AMPLIFY_MIN: u64 = 50_000;
/// `AMPLIFY_RATIO`: refuse when `total > AMPLIFY_MIN and total > AMPLIFY_RATIO * stored`.
pub const AMPLIFY_RATIO: u64 = 100;
/// Nesting deeper than this is refused.
pub const MAX_ASSEMBLY_DEPTH: i32 = 200;
/// `DELABELLA_MIN_FACES`: models with more faces are meshed with the Delabella algorithm.
pub const DELABELLA_MIN_FACES: u32 = 2000;
/// Interference: enlargement of the solids' boxes (collisions.py).
pub const INTERFERENCE_BOX_GAP: f64 = 0.01;
/// Interference: a common volume above this is a collision (analysis.py).
pub const CLASH_VOLUME_MM3: f64 = 1e-3;

/// First bytes of every BinXCAF (XBF) file written by OCCT.
const XBF_MAGIC: &[u8; 7] = b"BINFILE";

/// Environment switch: `STEPVIEW_FULL_FIX=1` keeps the full ShapeFix repair of STEP input.
pub const ENV_FULL_FIX: &str = "STEPVIEW_FULL_FIX";
/// Environment switch: `STEPVIEW_NO_DELABELLA=1` never uses the Delabella mesher.
pub const ENV_NO_DELABELLA: &str = "STEPVIEW_NO_DELABELLA";

// ------------------------------------------------------------------ Czech messages (core.py)

pub const MSG_READING: &str = "Čtu soubor…";
pub const MSG_TRANSFERRING: &str = "Převádím geometrii…";
pub const MSG_CHECKING: &str = "Kontroluji strukturu sestavy…";
pub const MSG_READING_STL: &str = "Čtu STL…";
pub const MSG_READ_FAILED: &str =
    "Soubor se nepodařilo přečíst (neplatný nebo poškozený STEP/IGES).";
pub const MSG_TRANSFER_FAILED: &str = "Převod geometrie selhal.";
pub const MSG_STL_FAILED: &str = "STL soubor se nepodařilo přečíst nebo je prázdný.";
pub const MSG_CYCLIC: &str =
    "Sestava obsahuje sama sebe (cyklický odkaz mezi díly) – nelze zobrazit.";
pub const MSG_TOO_DEEP: &str =
    "Sestava je vnořená příliš hluboko (více než 200 úrovní) – nelze zobrazit.";
pub const MSG_NO_NAME: &str = "(bez názvu)";

/// `f"Síťuji {n} těles…"`
pub fn msg_meshing(n: usize) -> String {
    format!("Síťuji {n} těles…")
}

/// `f"Uložení mezivýsledku selhalo: {st}"`
pub fn msg_save_failed(status: &str) -> String {
    format!("Uložení mezivýsledku selhalo: {status}")
}

/// `f"Načtení mezivýsledku selhalo: {status}"`
pub fn msg_load_failed(status: &str) -> String {
    format!("Načtení mezivýsledku selhalo: {status}")
}

/// Message for an assembly that would expand to more than `limit` instances.
pub fn msg_too_many_instances(limit: u64) -> String {
    format!(
        "Sestava by se rozvinula na více než {} instancí dílů a sestav – \
         to je příliš na zobrazení (pravděpodobně vnořené sestavy, které se opakovaně odkazují na sebe). \
         Soubor se neotevře.",
        fmt_thousands(limit)
    )
}

/// Message for an assembly with few records that expands to very many instances (the "bomb" message).
pub fn msg_amplification(stored: u64, total: u64) -> String {
    format!(
        "Sestava má v souboru jen {stored} záznamů, ale vnořením sestav by se rozvinula na {total} \
         instancí ({}× víc) – to vypadá jako záměrně vytvořená „bomba“, \
         zobrazení by trvalo minuty. Soubor se neotevře.",
        total / stored.max(1)
    )
}

// ------------------------------------------------------------------ errors

/// Failure of a bridge operation. The text is Czech and meant for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    /// The assembly is cyclic, nested too deeply or would expand to too many instances. Carries the exact message.
    TooComplex(String),
    /// Reading, saving or any other operation failed.
    Failed(String),
}

impl BridgeError {
    pub fn message(&self) -> &str {
        match self {
            BridgeError::TooComplex(m) | BridgeError::Failed(m) => m,
        }
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for BridgeError {}

impl From<cxx::Exception> for BridgeError {
    fn from(e: cxx::Exception) -> Self {
        BridgeError::Failed(e.what().to_string())
    }
}

type Res<T> = Result<T, BridgeError>;

// ------------------------------------------------------------------ meshing policy (core.py: mesh_doc)

/// `defl = min(max(size * 0.001, 0.002), 0.5)` for a part of the given largest extent (mm).
pub fn mesh_deflection(size: f64) -> f64 {
    (size * 0.001).clamp(0.002, 0.5)
}

/// Half-octave bucket of a deflection: `round(log2(defl) * 2)` (Python rounds half to even).
pub fn mesh_bucket_key(defl: f64) -> i32 {
    (defl.log2() * 2.0).round_ties_even() as i32
}

/// Deflection used for a whole bucket: `2 ** (key / 2)`.
pub fn mesh_bucket_deflection(key: i32) -> f64 {
    2.0_f64.powf(f64::from(key) / 2.0)
}

fn env_is_one(name: &str) -> bool {
    std::env::var(name).map(|v| v == "1").unwrap_or(false)
}

/// How a file is read and meshed. The defaults follow the environment switches of the Python viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadOptions {
    /// Keep the full ShapeFix repair of STEP input (`STEPVIEW_FULL_FIX=1`).
    pub full_fix: bool,
    /// Never use the Delabella mesher (`STEPVIEW_NO_DELABELLA=1`).
    pub no_delabella: bool,
    /// Models with more faces than this are meshed with Delabella (`DELABELLA_MIN_FACES`).
    pub delabella_min_faces: u32,
}

impl ReadOptions {
    /// Options from the values of the two environment variables (only the exact text "1" switches on).
    pub fn from_values(full_fix: Option<&str>, no_delabella: Option<&str>) -> Self {
        ReadOptions {
            full_fix: full_fix == Some("1"),
            no_delabella: no_delabella == Some("1"),
            delabella_min_faces: DELABELLA_MIN_FACES,
        }
    }

    /// Options from the process environment.
    pub fn from_env() -> Self {
        ReadOptions {
            full_fix: env_is_one(ENV_FULL_FIX),
            no_delabella: env_is_one(ENV_NO_DELABELLA),
            delabella_min_faces: DELABELLA_MIN_FACES,
        }
    }
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions::from_env()
    }
}

// ------------------------------------------------------------------ interference

/// Progress of [`Document::interferences_ex`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterferenceProgress {
    /// Pairs of solids tested so far.
    pub done: usize,
    /// Pairs of solids whose boxes overlap.
    pub total: usize,
    /// Part pairs with a collision so far.
    pub found: usize,
}

/// Outcome of the interference check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InterferenceReport {
    /// Collisions and touches per part pair, sorted by volume (largest first).
    pub results: Vec<Interference>,
    /// Part pairs with overlapping solid boxes ("dvojic dílů").
    pub part_pairs: usize,
    /// Solid pairs with overlapping boxes ("dvojic těles s překrytím obálek").
    pub body_pairs: usize,
    /// The progress callback asked to stop.
    pub stopped: bool,
}

/// A touch (faces on each other, no volume) is reported with a zero volume; a collision exceeds
/// [`CLASH_VOLUME_MM3`].
pub fn is_touch(i: &Interference) -> bool {
    i.volume_mm3 <= CLASH_VOLUME_MM3
}

// ------------------------------------------------------------------ Document

/// An XCAF document with meshed shapes.
pub struct Document {
    inner: cxx::UniquePtr<raw::Doc>,
    tree: OnceCell<ModelTree>,
}

fn path_str(path: &Path) -> Res<&str> {
    path.to_str()
        .ok_or_else(|| BridgeError::Failed("Cesta k souboru obsahuje neplatné znaky.".to_string()))
}

fn extension_lower(path: &Path) -> String {
    // os.path.splitext(path)[1].lower(): a leading dot alone is not an extension
    path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

impl Document {
    fn wrap(inner: cxx::UniquePtr<raw::Doc>) -> Self {
        Document {
            inner,
            tree: OnceCell::new(),
        }
    }

    /// Read STEP, IGES or STL, check the assembly structure and mesh every part (`read_cad`). `report` receives
    /// the progress texts of core.py.
    pub fn read(path: &Path, report: &mut dyn FnMut(&str)) -> Res<Document> {
        Document::read_with(path, report, &ReadOptions::from_env())
    }

    /// [`Document::read`] with explicit options.
    pub fn read_with(
        path: &Path,
        report: &mut dyn FnMut(&str),
        opts: &ReadOptions,
    ) -> Res<Document> {
        let p = path_str(path)?;
        let ext = extension_lower(path);
        if ext == ".stl" {
            report(MSG_READING_STL);
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let doc = raw::read_stl(p, &name)?;
            if doc.is_null() {
                return Err(BridgeError::Failed(MSG_STL_FAILED.to_string()));
            }
            return Ok(Document::wrap(doc));
        }
        let iges = ext == ".iges" || ext == ".igs";
        let doc = raw::new_doc()?;
        {
            let reader = raw::new_reader(iges)?;
            report(MSG_READING);
            // OCCT reports "done" for a file that does not exist; a file that cannot be opened is a read failure
            if std::fs::File::open(path).is_err() || !raw::reader_read_file(&reader, p)? {
                return Err(BridgeError::Failed(MSG_READ_FAILED.to_string()));
            }
            // two O(n^2) ShapeFix repairs are switched off for STEP unless STEPVIEW_FULL_FIX=1 (best effort)
            if !iges && !opts.full_fix {
                let _ = raw::reader_relax_shapefix(&reader);
            }
            report(MSG_TRANSFERRING);
            if !raw::reader_transfer(&reader, &doc)? {
                return Err(BridgeError::Failed(MSG_TRANSFER_FAILED.to_string()));
            }
        }
        let document = Document::wrap(doc);
        report(MSG_CHECKING);
        // instance bomb / cyclic assembly: fail before meshing
        document.count_instances()?;
        document.mesh(report, opts)?;
        Ok(document)
    }

    /// Mesh every non-assembly shape once; the deflection scales with the part size (`mesh_doc`).
    fn mesh(&self, report: &mut dyn FnMut(&str), opts: &ReadOptions) -> Res<()> {
        let sizes = raw::doc_mesh_items(&self.inner)?;
        let keys: Vec<i32> = sizes
            .iter()
            .map(|&s| mesh_bucket_key(mesh_deflection(s)))
            .collect();
        report(&msg_meshing(sizes.len()));
        raw::doc_mesh_run(
            &self.inner,
            &keys,
            opts.no_delabella,
            opts.delabella_min_faces,
        )?;
        Ok(())
    }

    /// Save the document as BinXCAF (XBF) with its triangulation.
    pub fn save_xbf(&self, path: &Path) -> Res<()> {
        let status = raw::doc_save_xbf(&self.inner, path_str(path)?)
            .map_err(|e| BridgeError::Failed(msg_save_failed(e.what())))?;
        if status.contains("OK") {
            Ok(())
        } else {
            Err(BridgeError::Failed(msg_save_failed(&status)))
        }
    }

    /// Load a document written by [`Document::save_xbf`].
    pub fn load_xbf(path: &Path) -> Res<Document> {
        let p = path_str(path)?;
        // OCCT can crash on input that is not an XBF file at all: refuse a wrong magic number up front
        // (a file that cannot be opened is left to OCCT, which reports it as a status).
        if let Ok(mut f) = std::fs::File::open(path) {
            let mut magic = [0u8; XBF_MAGIC.len()];
            if f.read_exact(&mut magic).is_err() || magic != *XBF_MAGIC {
                return Err(BridgeError::Failed(msg_load_failed(
                    "PCDM_ReaderStatus.PCDM_RS_UnrecognizedFileFormat",
                )));
            }
        }
        let mut status = String::new();
        let doc = raw::load_xbf(p, &mut status)
            .map_err(|e| BridgeError::Failed(msg_load_failed(e.what())))?;
        if status.contains("OK") {
            Ok(Document::wrap(doc))
        } else {
            Err(BridgeError::Failed(msg_load_failed(&status)))
        }
    }

    /// Number of tree nodes the assembly expands to (`count_instances`), using the limits of core.py.
    pub fn count_instances(&self) -> Res<u64> {
        self.count_instances_with(
            MAX_INSTANCES,
            AMPLIFY_MIN,
            AMPLIFY_RATIO,
            MAX_ASSEMBLY_DEPTH,
        )
    }

    /// [`Document::count_instances`] with explicit limits (for tests and tools).
    pub fn count_instances_with(
        &self,
        limit: u64,
        amplify_min: u64,
        amplify_ratio: u64,
        max_depth: i32,
    ) -> Res<u64> {
        let r =
            raw::doc_count_instances(&self.inner, limit, amplify_min, amplify_ratio, max_depth)?;
        count_result(&r, limit, max_depth)
    }

    /// The instance tree (`build_tree`). Refuses instance bombs and cyclic assemblies with
    /// [`BridgeError::TooComplex`]. The result is computed once.
    pub fn tree(&self) -> Res<ModelTree> {
        if let Some(t) = self.tree.get() {
            return Ok(t.clone());
        }
        self.count_instances()?;
        let t = model_tree_from(raw::doc_tree(&self.inner)?);
        let _ = self.tree.set(t.clone());
        Ok(t)
    }

    /// Number of distinct prototypes (non-assembly parts). `ProtoId` is an index below this count.
    pub fn proto_count(&self) -> u32 {
        raw::doc_proto_count(&self.inner).unwrap_or(0)
    }

    /// Triangulated prototype in its own coordinates. An empty mesh when the prototype has no geometry.
    pub fn proto_mesh(&self, id: ProtoId) -> ProtoMesh {
        self.try_proto_mesh(id).unwrap_or_default()
    }

    /// [`Document::proto_mesh`] with the error instead of an empty mesh.
    pub fn try_proto_mesh(&self, id: ProtoId) -> Res<ProtoMesh> {
        Ok(mesh_from_rec(raw::doc_proto_mesh(&self.inner, id)?))
    }

    /// Volume, area (from the display mesh), centroid (prototype coordinates), solid and face counts
    /// (`proto_props`). Zeros when the prototype has no geometry.
    pub fn mass_props(&self, id: ProtoId) -> MassProps {
        match raw::doc_mass(&self.inner, id) {
            Ok(m) => MassProps {
                volume_mm3: m.volume,
                area_mm2: m.area,
                centroid: m.centroid,
                solids: m.solids,
                faces: m.faces,
            },
            Err(_) => MassProps::default(),
        }
    }

    /// Node ids are indices of [`Document::tree`]; the tree is built (and checked) on first need.
    fn ensure_tree(&self) -> Res<()> {
        if self.tree.get().is_none() {
            self.tree()?;
        }
        Ok(())
    }

    /// World bounding box of a node (all parts below it). `None` without geometry or for an unknown node.
    pub fn node_bounds(&self, node: NodeId) -> Option<Aabb> {
        self.ensure_tree().ok()?;
        let b = raw::doc_node_bounds(&self.inner, node).ok()?;
        b.valid.then_some(Aabb {
            min: b.min,
            max: b.max,
        })
    }

    // ---------------------------------------------------------------- measurement

    /// Data of one measurable item (`Entity`), e.g. for the hover preview: `entity(..)?.short()`.
    pub fn entity(&self, item: &MeasureItem) -> Res<Entity> {
        match item {
            MeasureItem::Entity(r) => {
                self.ensure_tree()?;
                Ok(Entity::from_rec(&raw::doc_entity(
                    &self.inner,
                    r.node,
                    measure::kind_code(r.kind),
                    r.index,
                )?))
            }
            MeasureItem::Point {
                point,
                normal,
                mesh,
                snapped,
                patch,
            } => Entity::picked_point(*point, *normal, *mesh, *snapped, *patch)
                .map_err(BridgeError::Failed),
            MeasureItem::Loop(lp) => Entity::mesh_loop(lp).map_err(BridgeError::Failed),
        }
    }

    /// Measure picked B-rep entities: properties of one, relation of two, totals of three or more.
    pub fn measure(&self, entities: &[EntityRef]) -> Measurement {
        let items: Vec<MeasureItem> = entities.iter().copied().map(MeasureItem::Entity).collect();
        self.measure_items(&items)
    }

    /// [`Document::measure`] for any picked item (B-rep entities, exact surface points, mesh points and outlines).
    /// A selection that cannot be evaluated gives a "nelze vyhodnotit" row instead of an error.
    pub fn measure_items(&self, items: &[MeasureItem]) -> Measurement {
        self.try_measure_items(items)
            .unwrap_or_else(|e| Measurement::failed(e.message()))
    }

    /// [`Document::measure_items`] with the error.
    pub fn try_measure_items(&self, items: &[MeasureItem]) -> Res<Measurement> {
        let mut ents = Vec::with_capacity(items.len());
        for it in items {
            ents.push(self.entity(it)?);
        }
        let dist = if items.len() == 2 {
            self.distance(&items[0], &items[1])?
        } else {
            None
        };
        Ok(measure::assemble(ents, dist))
    }

    /// `BRepExtrema_DistShapeShape` of two items: the first solution, when there is one.
    pub fn distance(&self, a: &MeasureItem, b: &MeasureItem) -> Res<Option<Distance>> {
        self.ensure_tree()?;
        let r = raw::doc_distance(&self.inner, &shape_spec(a), &shape_spec(b))?;
        Ok((r.done && r.nb_solutions > 0).then_some(Distance {
            value: r.value,
            p1: r.p1,
            p2: r.p2,
        }))
    }

    /// `face_normal_at`: outward normal of a B-rep face at a world point (reversed for reversed faces).
    pub fn face_normal_at(&self, face: EntityRef, point: P3) -> Option<P3> {
        self.ensure_tree().ok()?;
        let r = raw::doc_face_normal_at(
            &self.inner,
            face.node,
            face.index,
            point[0],
            point[1],
            point[2],
        )
        .ok()?;
        r.valid.then_some(r.n)
    }

    // ---------------------------------------------------------------- interference

    /// Collisions between the parts below `nodes` (collisions.py without the touch option). `progress` is called
    /// about every 0.1 s; returning `false` stops the check.
    pub fn interferences(
        &self,
        nodes: &[NodeId],
        progress: &mut dyn FnMut(&InterferenceProgress) -> bool,
    ) -> Vec<Interference> {
        self.interferences_ex(nodes, false, progress)
            .map(|r| r.results)
            .unwrap_or_default()
    }

    /// The interference check with the touch option and the statistics of the status line.
    /// Work is per solid; parts of the same node pair are tested once; STL meshes are skipped.
    pub fn interferences_ex(
        &self,
        nodes: &[NodeId],
        include_touch: bool,
        progress: &mut dyn FnMut(&InterferenceProgress) -> bool,
    ) -> Res<InterferenceReport> {
        self.ensure_tree()?;
        let run = raw::interf_begin(&self.inner, nodes, INTERFERENCE_BOX_GAP)?;
        let run = run.as_ref().ok_or_else(|| {
            BridgeError::Failed("Kontrola kolizí se nepodařila spustit.".to_string())
        })?;
        let total = raw::interf_pair_count(run) as usize;
        let part_pairs = raw::interf_part_pair_count(run) as usize;
        let mut last: Option<Instant> = None;
        let mut stopped = false;
        for k in 0..total {
            raw::interf_step(run, k as u64, include_touch)?;
            // progress at most every 0.1 s (collisions.py)
            if last.is_none_or(|t| t.elapsed() > Duration::from_millis(100)) {
                let p = InterferenceProgress {
                    done: k + 1,
                    total,
                    found: raw::interf_collisions(run) as usize,
                };
                if !progress(&p) {
                    stopped = true;
                    break;
                }
                last = Some(Instant::now());
            }
        }
        let mut results: Vec<Interference> = raw::interf_results(run)?
            .into_iter()
            .map(|r| Interference {
                a: r.a,
                b: r.b,
                volume_mm3: r.volume,
                bounds: r.has_bounds.then_some(Aabb {
                    min: r.min,
                    max: r.max,
                }),
            })
            .collect();
        // results per part pair, largest volume first (stable)
        results.sort_by(|x, y| {
            y.volume_mm3
                .partial_cmp(&x.volume_mm3)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(InterferenceReport {
            results,
            part_pairs,
            body_pairs: total,
            stopped,
        })
    }

    /// Mesh (world coordinates) of the common volume of two colliding parts, for the red overlay. `None` when the
    /// parts do not overlap.
    pub fn interference_mesh(&self, a: NodeId, b: NodeId) -> Res<Option<ProtoMesh>> {
        self.ensure_tree()?;
        let m = mesh_from_rec(raw::doc_interference_mesh(&self.inner, a, b)?);
        Ok((!m.indices.is_empty()).then_some(m))
    }
}

/// Maps the instance-count status of the C++ side to the result / the Czech message.
fn count_result(r: &raw::CountRec, limit: u64, max_depth: i32) -> Res<u64> {
    match r.status {
        0 => Ok(r.total),
        1 => Err(BridgeError::TooComplex(MSG_CYCLIC.to_string())),
        2 => Err(BridgeError::TooComplex(
            MSG_TOO_DEEP.replace("200", &max_depth.to_string()),
        )),
        3 => Err(BridgeError::TooComplex(msg_too_many_instances(limit))),
        _ => Err(BridgeError::TooComplex(msg_amplification(
            r.stored, r.total,
        ))),
    }
}

fn shape_spec(item: &MeasureItem) -> raw::ShapeSpec {
    match item {
        MeasureItem::Entity(r) => raw::ShapeSpec {
            kind: 0,
            node: r.node,
            ekind: measure::kind_code(r.kind),
            index: r.index,
            pts: Vec::new(),
            closed: false,
        },
        MeasureItem::Point { point, .. } => raw::ShapeSpec {
            kind: 1,
            node: 0,
            ekind: 0,
            index: 0,
            pts: point.to_vec(),
            closed: false,
        },
        MeasureItem::Loop(lp) => raw::ShapeSpec {
            kind: 2,
            node: 0,
            ekind: 0,
            index: 0,
            pts: lp.points.iter().flatten().copied().collect(),
            closed: lp.closed,
        },
    }
}

fn model_tree_from(rec: raw::TreeRec) -> ModelTree {
    let mut nodes: Vec<TreeNode> = rec
        .nodes
        .into_iter()
        .enumerate()
        .map(|(i, n)| TreeNode {
            id: i as NodeId,
            parent: (n.parent >= 0).then_some(n.parent as NodeId),
            children: Vec::new(),
            name: n.name,
            proto_name: n.proto_name,
            key: n.key,
            is_assembly: n.is_assembly,
            proto: (n.proto >= 0).then_some(n.proto as ProtoId),
            location: n.location,
            color: n
                .has_color
                .then_some(Rgb(n.color[0], n.color[1], n.color[2])),
            own_style: n.own_style,
        })
        .collect();
    let mut roots = Vec::new();
    for i in 0..nodes.len() {
        match nodes[i].parent {
            Some(p) => nodes[p as usize].children.push(i as NodeId),
            None => roots.push(i as NodeId),
        }
    }
    ModelTree {
        roots,
        nodes,
        proto_instances: rec.proto_instances,
    }
}

fn triples(v: &[f32]) -> Vec<[f32; 3]> {
    v.as_chunks::<3>().0.to_vec()
}

fn mesh_from_rec(r: raw::MeshRec) -> ProtoMesh {
    let faces = (0..r.face_first.len())
        .map(|i| FaceSpan {
            first_index: r.face_first[i],
            index_count: r.face_count[i],
            color: (r.face_has_color[i] != 0).then(|| {
                Rgb(
                    r.face_color[i * 3],
                    r.face_color[i * 3 + 1],
                    r.face_color[i * 3 + 2],
                )
            }),
        })
        .collect();
    let edges = r
        .edge_first
        .iter()
        .zip(&r.edge_count)
        .map(|(&f, &c)| EdgeSpan {
            first_point: f,
            point_count: c,
        })
        .collect();
    let positions = triples(&r.positions);
    let normals = triples(&r.normals);
    let edge_points = triples(&r.edge_points);
    let vertices = triples(&r.vertices);
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in positions.iter().chain(&edge_points).chain(&vertices) {
        for k in 0..3 {
            lo[k] = lo[k].min(f64::from(p[k]));
            hi[k] = hi[k].max(f64::from(p[k]));
        }
    }
    let bounds = (lo[0] <= hi[0]).then_some(Aabb { min: lo, max: hi });
    ProtoMesh {
        positions,
        normals,
        indices: r.indices,
        faces,
        edge_points,
        edges,
        vertices,
        bounds,
        mesh_only: r.mesh_only,
    }
}

#[cfg(test)]
mod stepgen;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_geom;
