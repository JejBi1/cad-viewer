//! Recorded drawing primitives, batching into z-ordered `DrawBatch`es, picking, snapping and highlighting
//! (port of `fastscene.BatchBackend.build` and `fastscene.Batch`).
use crate::colors::Rgba;
use crate::geom::*;
use crate::levels::{self, MAX_LEVELS, VISIT_BUDGET};
use crate::props::Role;
use crate::tess;
use cad_types::{DrawBatch, DrawImage, Layer, Scene2d};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

pub const K_LINES: u8 = 0; // segment pairs
pub const K_POLY: u8 = 1; // one polyline
pub const K_PATH: u8 = 2; // QPainterPath elements

/// Most path elements (points) one scene records: the memory of the recorder (17 bytes each) and nothing else. A last
/// resort far above what real drawings need (a drawing of 25 000 texts of 30 letters records 32 000 000, 290 000 dashed
/// lines 31 000 000); everything below it is drawn, with less detail when it gets dense (see `SOFT_OUT_VERTS`).
pub const MAX_SCENE_ELEMS: usize = 100_000_000;
/// Most primitives one scene records.
pub const MAX_SCENE_PRIMS: usize = 6_000_000;
/// Output vertices (points of line segments plus corners of fill triangles) up to which every shape is drawn in full
/// detail (10 000 short texts are 40 million; it is half of `MAX_OUT_VERTS` on a computer that has the memory for that).
/// Above it, the filled shapes recorded later (glyphs, hatches) get simplified outlines (`Lod::Simple`); strokes never do.
pub const SOFT_OUT_VERTS: u64 = 32_000_000;
/// Ceiling of the hard limit of the output vertices of a scene (8 bytes each in the scene, 12 on the graphics card, 45 at the
/// peak of making it: 2.9 GB, what an 8 GB computer can spare). The scene is refused above the limit, which is this number
/// at the most and lower when the computer has less memory free (`memory::out_limits_for`). It was 300 million, and a
/// drawing of 2 KB took 5 to 7.5 GB before it was refused. Above `SOFT_OUT_VERTS` (or half of the limit) the detail of
/// filled shapes is reduced first (`Lod::Simple`), and when even that does not fit, glyphs and fills are drawn as a box
/// (`Lod::Box`, a few vertices each): only a scene whose boxes and strokes alone do not fit is refused.
pub const MAX_OUT_VERTS: u64 = 64_000_000;
/// Path elements that dashed lines may record together; lines that would exceed it are drawn solid (a dash pattern of
/// 12.7 / 6.35 on 290 000 lines of length 1000 records 31 000 000).
pub const DASH_SOFT_ELEMS: usize = 48_000_000;

/// Path elements of the glyph outlines that one line of text may have (a line of 23 000 letters: the outlines are 34 MB
/// as recorded, and three copies of them exist while they are made); a longer line is recorded as a box.
pub const MAX_LINE_GLYPH_ELEMS: usize = 1_000_000;

/// Vertices (points of segments plus corners of triangles) after which a batch is closed and the next one of the same pen is
/// started: 4 194 304 are 32 MB. See `Built::build_ctl`.
pub const BATCH_MAX_VERTS: usize = 1 << 22;

/// Times a scene that overflowed although its strokes and boxes fit is made again (`Built::build_ctl`): with room kept free for
/// what the first pass missed, with more of it, and with every filled shape as a box.
const RETRY_PASSES: usize = 3;

/// Primitives / path elements that are flattened together by `Built::build`: the geometry of one slice is alive at once.
const SLICE_PRIMS: usize = 2048;
const SLICE_ELEMS: usize = 100_000;

pub const CHUNK_ELEMS: usize = 6000; // path elements per merged stroke batch (~2000 segments)
pub const BIG_PRIM: usize = 3000; // elements: above this, picking strokes only the segments near the cursor
/// mm of a cosmetic pen to pixels (`lineweight / 0.3527`).
pub const MM_TO_PX: f64 = 1.0 / 0.3527;
/// Highlight colour of the selection (orange).
pub const SELECTION_COLOR: Rgba = [255, 140, 0, 255];
/// Points of outlines (two per segment) the highlight of one entity may have: 4 000 000 are 2 000 000 segments, 32 MB in
/// the scene and 48 MB on the graphics card. A block reference of 9 000 texts has more than ten times as many.
pub const HIGHLIGHT_MAX_POINTS: usize = 4_000_000;
/// Rectangles of bounds (8 points each) that stand in for the outlines of the primitives after the budget of
/// `HIGHLIGHT_MAX_POINTS` is used up; the primitives after those share one rectangle.
pub const HIGHLIGHT_MAX_BOXES: usize = 200_000;

pub const FLAG_ISLINE: u8 = 1; // a plain LINE (snapped only as a fallback)
pub const FLAG_NOSNAP: u8 = 2; // polygons and points do not snap
pub const FLAG_POINT: u8 = 4; // dimensionless point (zero length segment)
/// The glyph outlines of one line of text: what `Lod::Box` draws instead of them is the oriented bar of the line.
pub const FLAG_TEXT: u8 = 8;
/// A filled path that already is a box (the recorder made it instead of the glyphs of a text it had no room for, see
/// `Recorder::glyph_elems_left`): there is nothing left to reduce, it is always drawn as a plain fill.
pub const FLAG_BOX: u8 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GKind {
    /// Strokes
    S,
    /// Filled paths (text glyphs, hatches): fill and stroked outline
    F,
    /// Filled polygons (SOLID, wide polylines): fill only
    P,
    /// Raster picture (IMAGE): its corners are the primitive, the pixels are in `Recorder::images`.
    I,
}

#[derive(Clone, Debug)]
pub struct GroupDef {
    pub kind: GKind,
    /// Layers the visibility of the primitive depends on (lowest id first as met: parents, then the entity itself).
    pub layers: Vec<u32>,
    pub color: Rgba,
    /// mm
    pub lw: f32,
    /// Whether the colour is the foreground or the background of the layout (see `Role`).
    pub role: Role,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    Layout(u32),
    Block(u32),
}

/// An entity of the document: `index` in the entity list of its layout / block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityId {
    pub container: Container,
    pub index: u32,
}

/// The entity a primitive was drawn for: the entity itself (which may be inside a block, or an attribute of an
/// INSERT) and the transformation of its block reference chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Leaf {
    pub id: EntityId,
    pub attr: Option<u32>,
    pub xf: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Prim {
    pub gid: u32,
    pub kind: u8,
    pub estart: u32,
    pub ecount: u32,
    pub leaf: u32,
    pub top: u32,
    pub flags: u8,
    /// Index into `Recorder::images` for `GKind::I` primitives; 0 otherwise.
    pub aux: u32,
}

/// Pixels of one raster picture, with clipping and transparency applied (see `cad_types::DrawImage`).
#[derive(Clone, Debug)]
pub struct PlacedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug)]
struct Limits {
    elems: usize,
    prims: usize,
    /// Output vertices up to which the detail stays full, and the most the scene may have.
    soft_out: u64,
    max_out: u64,
    /// Path elements that dashed lines may record together (`DASH_SOFT_ELEMS`).
    soft_dash: usize,
    /// Vertices after which a batch is closed (`BATCH_MAX_VERTS`).
    batch_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { elems: MAX_SCENE_ELEMS, prims: MAX_SCENE_PRIMS, soft_out: SOFT_OUT_VERTS, max_out: MAX_OUT_VERTS, soft_dash: DASH_SOFT_ELEMS, batch_max: BATCH_MAX_VERTS }
    }
}

/// How much detail a primitive gets when the scene is built. Only filled paths (glyphs, hatches) are ever reduced: a stroked
/// curve is always drawn as finely as the tolerance of the drawing says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lod {
    Full,
    /// Curves flattened with one step per curve and a coarser tolerance, filled shapes without their outline stroke:
    /// about a fifth of the vertices of `Full` for text.
    Simple,
    /// The last level, for a scene that does not fit even simplified: the line of a text is its oriented bar (two
    /// triangles, 6 vertices), any other filled shape the outline of its bounding box (4 segments, 8 vertices). Nothing
    /// of the drawing is left out, the letters are.
    Box,
}

#[derive(Default)]
pub struct Recorder {
    pub groups: Vec<GroupDef>,
    gmap: HashMap<(GKind, Vec<u32>, Rgba, u32, Role), u32>,
    pub prims: Vec<Prim>,
    pub images: Vec<PlacedImage>,
    pub e_xy: Vec<V2>,
    pub e_ty: Vec<u8>,
    pub leaves: Vec<Leaf>,
    pub tops: Vec<EntityId>,
    pub xforms: Vec<Xform>,
    top_index: HashMap<EntityId, u32>,
    cur_leaf: u32,
    cur_top: u32,
    limits: Limits,
    /// Path elements of glyph outlines recorded so far (what `glyph_elems_left` counts against).
    text_elems: usize,
    /// A primitive was refused because a limit was reached: the scene is incomplete and is not used.
    over: bool,
    /// The build was stopped (cancelled, out of time): nothing more is recorded.
    halt: Option<crate::ctl::Halt>,
}

impl Recorder {
    pub fn new() -> Recorder {
        Recorder { xforms: vec![Xform::IDENTITY], ..Default::default() }
    }

    /// Other limits than the defaults (tests).
    pub fn set_limits(&mut self, max_elems: usize, max_prims: usize) {
        self.limits.elems = max_elems;
        self.limits.prims = max_prims;
    }

    /// Other output vertex limits than the defaults (tests): `soft` is where the detail gets reduced, `max` where the
    /// scene is refused.
    pub fn set_out_limits(&mut self, soft: u64, max: u64) {
        self.limits.soft_out = soft;
        self.limits.max_out = max;
    }

    /// Another dash budget than the default (tests).
    pub fn set_dash_limit(&mut self, elems: usize) {
        self.limits.soft_dash = elems;
    }

    /// Another size of a batch than `BATCH_MAX_VERTS` (tests).
    pub fn set_batch_max(&mut self, verts: usize) {
        self.limits.batch_max = verts.max(1);
    }

    /// Path elements that dashed lines may record together before the rest is drawn solid.
    pub fn soft_dash(&self) -> usize {
        self.limits.soft_dash
    }

    /// Output vertices up to which the detail of the scene stays full.
    pub fn soft_out(&self) -> u64 {
        self.limits.soft_out
    }

    /// The most output vertices the scene may have.
    pub fn max_out(&self) -> u64 {
        self.limits.max_out
    }

    /// The scene reached a limit; nothing is recorded any more and the frontend stops drawing.
    pub fn is_over(&self) -> bool {
        self.over
    }

    /// Declares the scene too large (the frontend predicted that a primitive would not fit).
    pub fn mark_over(&mut self) {
        self.over = true;
    }

    /// The build was stopped: the frontend leaves its loops, nothing more is recorded.
    pub fn set_halt(&mut self, h: crate::ctl::Halt) {
        if self.halt.is_none() || h == crate::ctl::Halt::Cancelled {
            self.halt = Some(h);
        }
    }

    pub fn halt(&self) -> Option<crate::ctl::Halt> {
        self.halt
    }

    /// Nothing more should be drawn: a limit was reached or the build was stopped.
    pub fn is_stopped(&self) -> bool {
        self.over || self.halt.is_some()
    }

    /// Elements that still fit into the scene.
    pub fn elems_left(&self) -> usize {
        self.limits.elems.saturating_sub(self.e_xy.len())
    }

    /// The limits of the scene, for the message.
    pub fn max_elems(&self) -> usize {
        self.limits.elems
    }

    /// Path elements the glyph outlines of texts may still take: three quarters of the elements of the scene (the rest is for
    /// the other primitives and for the boxes that stand in for the texts after this one is used up), and never more than what
    /// is left of the scene. A text whose outlines do not fit is recorded as a box (`Frontend::draw_text_line`, flag
    /// `FLAG_BOX`) of five elements, so that a drawing of any number of texts is recorded in memory that does not depend on
    /// how many there are. Outlines beyond this could hardly be drawn within the vertices of the scene either: simplified
    /// they make 1.0 to 1.5 vertices per element, and the scene may have as many vertices as it may have elements.
    pub fn glyph_elems_left(&self) -> usize {
        (self.limits.elems / 4 * 3).saturating_sub(self.text_elems).min(self.elems_left())
    }

    /// Counts the glyph outlines of a text that were recorded (`glyph_elems_left`).
    pub fn add_glyph_elems(&mut self, n: usize) {
        self.text_elems = self.text_elems.saturating_add(n);
    }

    pub fn group(&mut self, kind: GKind, layers: &[u32], color: Rgba, lw: f64) -> u32 {
        self.group_role(kind, layers, color, lw, Role::None)
    }

    /// `group` for a colour that is the foreground or the background of the layout: two things of the same colour that
    /// differ in the role are two groups (they are different colours on the other background).
    pub fn group_role(&mut self, kind: GKind, layers: &[u32], color: Rgba, lw: f64, role: Role) -> u32 {
        let lw = if kind == GKind::P { 0.0 } else { lw as f32 };
        let key = (kind, layers.to_vec(), color, lw.to_bits(), role);
        if let Some(&g) = self.gmap.get(&key) {
            return g;
        }
        let g = self.groups.len() as u32;
        self.groups.push(GroupDef { kind, layers: layers.to_vec(), color, lw, role });
        self.gmap.insert(key, g);
        g
    }

    pub fn push_xform(&mut self, x: Xform) -> u32 {
        self.xforms.push(x);
        (self.xforms.len() - 1) as u32
    }

    pub fn set_top(&mut self, id: EntityId) {
        let n = self.tops.len() as u32;
        let t = *self.top_index.entry(id).or_insert_with(|| n);
        if t == n {
            self.tops.push(id);
        }
        self.cur_top = t;
    }

    pub fn set_leaf(&mut self, leaf: Leaf) {
        // consecutive prims of one entity share a leaf entry
        if let Some(last) = self.leaves.last() {
            if *last == leaf && self.cur_leaf as usize + 1 == self.leaves.len() {
                return;
            }
        }
        self.leaves.push(leaf);
        self.cur_leaf = (self.leaves.len() - 1) as u32;
    }

    fn add(&mut self, gid: u32, kind: u8, ty: &[u8], xy: &[V2], flags: u8) {
        if self.is_stopped() || xy.is_empty() || xy.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return;
        }
        if self.e_xy.len().saturating_add(xy.len()) > self.limits.elems || self.prims.len() >= self.limits.prims {
            self.over = true;
            return;
        }
        let estart = self.e_xy.len() as u32;
        self.e_xy.extend_from_slice(xy);
        self.e_ty.extend_from_slice(ty);
        self.prims.push(Prim { gid, kind, estart, ecount: xy.len() as u32, leaf: self.cur_leaf, top: self.cur_top, flags, aux: 0 });
    }

    /// A raster picture in the drawing order. `corners`: top-left, top-right, bottom-right, bottom-left in drawing
    /// coordinates. Its outline is recorded as a filled quad, so picking, snapping and bounds see it like a fill.
    pub fn image(&mut self, gid: u32, corners: [V2; 4], pic: PlacedImage) {
        let idx = self.images.len() as u32;
        self.images.push(pic);
        let before = self.prims.len();
        let ty = [M_MOVE, M_LINE, M_LINE, M_LINE];
        self.add(gid, K_POLY, &ty, &corners, FLAG_NOSNAP);
        if self.prims.len() > before {
            self.prims[before].aux = idx;
        }
    }

    /// Dimensionless point: a zero length segment.
    pub fn point(&mut self, gid: u32, p: V2) {
        self.add(gid, K_LINES, &[M_MOVE, M_LINE], &[p, p], FLAG_NOSNAP | FLAG_POINT);
    }

    pub fn line(&mut self, gid: u32, a: V2, b: V2) {
        if v2_close(a, b) {
            self.point(gid, a);
        } else {
            self.add(gid, K_LINES, &[M_MOVE, M_LINE], &[a, b], FLAG_ISLINE);
        }
    }

    pub fn solid_lines(&mut self, gid: u32, lines: &[(V2, V2)]) {
        let mut xy = Vec::with_capacity(lines.len() * 2);
        for &(s, e) in lines {
            if v2_close(s, e) {
                self.point(gid, s);
            } else {
                xy.push(s);
                xy.push(e);
            }
        }
        if !xy.is_empty() {
            let ty: Vec<u8> = (0..xy.len()).map(|i| if i % 2 == 0 { M_MOVE } else { M_LINE }).collect();
            self.add(gid, K_LINES, &ty, &xy, FLAG_ISLINE);
        }
    }

    pub fn polyline(&mut self, gid: u32, pts: &[V2]) {
        if pts.len() < 2 {
            return;
        }
        let ty: Vec<u8> = (0..pts.len()).map(|i| if i == 0 { M_MOVE } else { M_LINE }).collect();
        self.add(gid, K_POLY, &ty, pts, 0);
    }

    pub fn path(&mut self, gid: u32, e: &Elems) {
        self.path_flags(gid, e, 0);
    }

    /// `path` with the flags of the primitive (`FLAG_TEXT`, `FLAG_BOX`).
    pub fn path_flags(&mut self, gid: u32, e: &Elems, flags: u8) {
        if e.is_empty() {
            return;
        }
        self.add(gid, K_PATH, &e.ty, &e.xy, flags);
    }

    pub fn filled_polygon(&mut self, gid: u32, pts: &[V2]) {
        if pts.is_empty() {
            return;
        }
        let ty: Vec<u8> = (0..pts.len()).map(|i| if i == 0 { M_MOVE } else { M_LINE }).collect();
        self.add(gid, K_POLY, &ty, pts, FLAG_NOSNAP);
    }

    pub fn elems(&self, p: &Prim) -> (&[u8], &[V2]) {
        let s = p.estart as usize;
        let n = p.ecount as usize;
        (&self.e_ty[s..s + n], &self.e_xy[s..s + n])
    }
}

/// Which layers are switched on; index = layer id.
#[derive(Clone, Debug, Default)]
pub struct LayerVis(pub Vec<bool>);

impl LayerVis {
    pub fn is_shown(&self, layers: &[u32]) -> bool {
        layers.iter().all(|&l| self.0.get(l as usize).copied().unwrap_or(true))
    }
}

/// How the geometry of a scene was made (tests): the primitives are flattened in slices, one slice alive at a time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub slices: usize,
    /// Most primitives and most path elements of any slice.
    pub max_slice_prims: usize,
    pub max_slice_elems: usize,
    /// Primitives drawn at `Lod::Simple`.
    pub simplified: usize,
    /// Primitives drawn as a box: the ones that `Lod::Box` took down because the scene did not fit, and the texts that
    /// were recorded as a box (`FLAG_BOX`).
    pub boxed: usize,
    /// Output vertices the levels of detail were estimated to make (0: far below the limit, not estimated).
    pub est_out: u64,
    /// Times the scene was made: 1, or more when it overflowed although the strokes and the boxes fit and was made again with
    /// less room for the filled shapes (`Built::build_ctl`).
    pub passes: usize,
}

/// A built layout: primitives, the batches made of them and what is needed to query them.
pub struct Built {
    pub rec: Recorder,
    pub bb: Vec<[f64; 4]>,
    pub level: Vec<i32>,
    pub scene: Scene2d,
    /// Layers each batch depends on (a batch is shown when all of them are).
    pub batch_layers: Vec<Vec<u32>>,
    /// The role of the colour of each batch (`Role`).
    pub batch_roles: Vec<Role>,
    /// The scene has a picture whose pixels were blended with the background (the fade of an IMAGE): the colours of the
    /// layout cannot be changed without making the scene again.
    pub bg_sensitive: bool,
    /// Bounds of every batch (world coordinates).
    pub batch_bounds: Vec<Option<[f64; 4]>>,
    pub lw_scale: f64,
    pub bg: [u8; 3],
    pub origin: [f64; 2],
    pub fallback_levels: bool,
    /// Czech notes about what was left out or simplified (shown with the drawing information).
    pub notes: Vec<String>,
    /// Why the build stopped before everything was made (`Cancelled`: the result is not wanted).
    pub halt: Option<crate::ctl::Halt>,
    /// The output reached `MAX_OUT_VERTS`: the scene is incomplete and is refused.
    pub over_out: bool,
    pub stats: BuildStats,
}

fn px_width(lw_mm: f64, scale: f64) -> f32 {
    let px = lw_mm * MM_TO_PX * scale;
    if px < 1.0 {
        0.0
    } else {
        px as f32
    }
}

fn to_f32(p: V2, origin: [f64; 2]) -> [f32; 2] {
    [(p[0] - origin[0]) as f32, (p[1] - origin[1]) as f32]
}

struct Flat {
    lines: Vec<V2>, // point pairs
    tris: Vec<V2>,
}

/// Path points a curve of a filled path is counted as in `full_verts` (`flatten(tol, 4)` gives four steps at the least; a
/// glyph curve is mostly just that). Strokes are not estimated with it: they are counted exactly.
const CURVE_PTS: u64 = 5;

/// Steps per curve at the full detail of a primitive (what `Built::flatten_prim_lod` flattens with, and `stroke_path_verts`
/// counts).
const FULL_STEPS: usize = 4;

/// (move / line elements, curves) of a path.
fn count_elems(ty: &[u8]) -> (u64, u64) {
    let (mut lines, mut curves) = (0u64, 0u64);
    for &t in ty {
        match t {
            M_MOVE | M_LINE => lines += 1,
            M_CURVE => curves += 1,
            _ => {}
        }
    }
    (lines, curves)
}

/// Output vertices of a stroked path: two for every segment of its flattened subpaths, as `Built::flatten_prim_lod` makes them
/// at the tolerance `tol`. Exact, not an estimate: a circle of the radius of 60 in a drawing of 120 is 128 segments, which is
/// 256 vertices, and a curve is not "five points" at the finest tolerance of a big drawing. A path of lines only is counted
/// without making its points.
fn stroke_path_verts(ty: &[u8], xy: &[V2], tol: f64) -> u64 {
    if ty.contains(&M_CURVE) {
        return flatten_path(ty, xy, tol, FULL_STEPS).iter().map(|s| 2 * s.len().saturating_sub(1) as u64).sum();
    }
    let (mut total, mut run) = (0u64, 0u64);
    for &t in ty {
        match t {
            M_MOVE => {
                if run > 1 {
                    total += 2 * (run - 1);
                }
                run = 1;
            }
            M_LINE => run += 1,
            _ => {}
        }
    }
    if run > 1 {
        total += 2 * (run - 1);
    }
    total
}

/// Output vertices a primitive makes at full detail. Strokes are exact (see `stroke_path_verts`; a polyline and a set of
/// segments are known from their points). Filled paths are a good estimate, not an exact number: the shapes of letters and of
/// hatches are cut into triangles that nobody counts before they are made, and the demotion in `Built::build_ctl` takes what
/// the estimate got wrong back to what fits. A picture makes none (it is drawn from its pixels); a box recorded in place of a
/// text is its two triangles.
fn full_verts(rec: &Recorder, p: &Prim, kind: GKind, tol: f64) -> u64 {
    match (kind, p.kind) {
        (GKind::S, K_LINES) => u64::from(p.ecount),
        (GKind::S, K_POLY) => 2 * u64::from(p.ecount.saturating_sub(1)),
        (GKind::S, _) => {
            let (ty, xy) = rec.elems(p);
            stroke_path_verts(ty, xy, tol)
        }
        (GKind::P, _) => 3 * u64::from(p.ecount),
        (GKind::I, _) => 0,
        (GKind::F, _) if p.flags & FLAG_BOX != 0 => {
            // a closed ring of n corners is n + 1 elements and n - 2 triangles (a box: five elements, six vertices)
            let (l, _) = count_elems(rec.elems(p).0);
            3 * l.saturating_sub(3).max(1)
        }
        (GKind::F, _) => {
            let (l, c) = count_elems(rec.elems(p).0);
            5 * (l + CURVE_PTS * c)
        }
    }
}

/// What every primitive of a recorder makes at each level of detail: the base of the plan (`assign_lod`) and of the room that
/// the filled shapes of a slice may take (`Built::build_ctl`). The vertices at full detail are worked out once, for all of
/// the primitives: the curved strokes are flattened here (in parallel) at the tolerance of the drawing, so that the count of
/// the strokes is the number that the geometry will have and not a guess. It replaces the five points per curve that a circle
/// of 128 segments was once counted as: texts earlier in the drawing took the room that those circles then needed.
struct Est {
    full: Vec<u64>,
}

impl Est {
    fn new(rec: &Recorder, tol: f64) -> Est {
        let one = |p: &Prim| full_verts(rec, p, rec.groups[p.gid as usize].kind, tol);
        let full: Vec<u64> = if rec.prims.len() > 512 { rec.prims.par_iter().map(one).collect() } else { rec.prims.iter().map(one).collect() };
        #[cfg(test)]
        let full = test_hooks::bias(rec, full);
        Est { full }
    }

    /// Output vertices of the primitive `i` at full detail.
    fn full(&self, i: usize) -> u64 {
        self.full[i]
    }

    /// Output vertices of a primitive at `Lod::Simple`. A filled path is cut into triangles: three vertices for each of its
    /// points, and with one step per curve a curve adds a point (a little less than one, 0.7 to 1.2 measured on texts,
    /// dimensions and paragraphs of MTEXT); five quarters of it are counted, which is 1.0 to 1.4 times what is made. It was
    /// two (2 times what texts make), and a scene with room for the simplified detail was given less of it than it fits.
    fn simple(&self, rec: &Recorder, i: usize) -> u64 {
        let p = &rec.prims[i];
        match rec.groups[p.gid as usize].kind {
            GKind::F => {
                let (l, c) = count_elems(rec.elems(p).0);
                3 * (l + c + c / 4)
            }
            _ => self.full(i),
        }
    }

    /// Output vertices of a primitive at `Lod::Box`, the least it can make: for what cannot be reduced, what it makes anyway.
    fn boxed(&self, rec: &Recorder, i: usize) -> u64 {
        let p = &rec.prims[i];
        let full = self.full(i);
        if can_simplify(rec.groups[p.gid as usize].kind, p) {
            let b = if p.flags & FLAG_TEXT != 0 { BOX_BAR_VERTS } else { BOX_OUTLINE_VERTS };
            b.min(full)
        } else {
            full
        }
    }
}

/// Vertices of the bar that stands in for a line of text at `Lod::Box` (two triangles) and of the outline of the bounding
/// box of any other filled shape (four segments).
pub const BOX_BAR_VERTS: u64 = 6;
pub const BOX_OUTLINE_VERTS: u64 = 8;

/// Primitives whose detail can be reduced: filled paths (glyph outlines, hatches) that are not a box already. Strokes are
/// never reduced: a circle or an arc is as fine as the tolerance of the drawing says, whatever the size of the drawing and
/// the memory of the computer.
fn can_simplify(kind: GKind, p: &Prim) -> bool {
    kind == GKind::F && p.flags & FLAG_BOX == 0
}

/// What `assign_lod` decided.
struct LodPlan {
    /// Level of every primitive (empty: all of them full).
    lod: Vec<Lod>,
    /// Output vertices that all of it is estimated to make with these levels (0 when the whole scene is far below `soft`).
    est: u64,
    /// Output vertices that the scene makes at the least: boxes for what can be reduced, the full detail of the rest.
    min: u64,
}

fn lod_at(lod: &[Lod], i: u32) -> Lod {
    lod.get(i as usize).copied().unwrap_or(Lod::Full)
}

/// Sets the level of a primitive (the levels are made when the first one is not full).
fn set_lod(lod: &mut Vec<Lod>, n: usize, i: u32, to: Lod) {
    if lod.is_empty() {
        lod.resize(n, Lod::Full);
    }
    lod[i as usize] = to;
}

/// Level of detail of every primitive. Everything is drawn in full detail until the output would reach `soft`
/// vertices; the filled shapes recorded after that point (in drawing order) are drawn simplified, and still drawn. A shape
/// is also not given full detail when the shapes after it would then no longer fit `hard` simplified: a letter that is
/// drawn simplified is better than another that is drawn in full next to a text that is only a bar.
/// `Built::build_ctl` takes them down to `Lod::Box` when what it really makes does not fit the hard limit.
fn assign_lod(rec: &Recorder, est: &Est, soft: u64, hard: u64) -> LodPlan {
    // every element of a filled path makes at most this many output vertices (5 per line element, and 5 x 5 / 3 = 8.3 per
    // element of a curve, which is three of them)
    const MAX_PER_ELEM: u64 = 9;
    let kind_of = |p: &Prim| rec.groups[p.gid as usize].kind;
    let min = (0..rec.prims.len()).fold(0u64, |m, i| m.saturating_add(est.boxed(rec, i)));
    // Nothing is reduced when even the most that the shapes can make is under `soft`: the strokes are counted as they are (a
    // curve makes far more than nine vertices per element), the filled shapes at their most.
    let most = rec.prims.iter().enumerate().fold(0u64, |m, (i, p)| {
        m.saturating_add(if kind_of(p) == GKind::S { est.full(i) } else { u64::from(p.ecount).saturating_mul(MAX_PER_ELEM) })
    });
    if most <= soft {
        return LodPlan { lod: Vec::new(), est: 0, min };
    }
    let mut lod = vec![Lod::Full; rec.prims.len()];
    let reduced = |i: usize, p: &Prim| if can_simplify(kind_of(p), p) { est.simple(rec, i) } else { est.full(i) };
    let mut rest = rec.prims.iter().enumerate().fold(0u64, |m, (i, p)| m.saturating_add(reduced(i, p)));
    let (mut used, mut simplified) = (0u64, 0usize);
    for (i, p) in rec.prims.iter().enumerate() {
        let full = est.full(i);
        // what the shapes after this one take at the least short of boxes
        rest = rest.saturating_sub(reduced(i, p));
        if (used.saturating_add(full) > soft || used.saturating_add(full).saturating_add(rest) > hard) && can_simplify(kind_of(p), p) {
            lod[i] = Lod::Simple;
            used = used.saturating_add(est.simple(rec, i));
            simplified += 1;
        } else {
            used = used.saturating_add(full);
        }
    }
    LodPlan { lod: if simplified == 0 { Vec::new() } else { lod }, est: used, min }
}

/// The four corners of the smallest box around `pts` that is turned to the direction in which they are spread the most
/// (the principal axis): the bar of a line of text, whatever its angle. `None` for points with no extent.
fn oriented_box(pts: &[V2]) -> Option<[V2; 4]> {
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    let (mut mx, mut my) = (0.0, 0.0);
    for p in pts {
        mx += p[0];
        my += p[1];
    }
    let (mx, my) = (mx / n, my / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p[0] - mx, p[1] - my);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (s, c) = theta.sin_cos();
    let (mut u0, mut u1, mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for p in pts {
        let (dx, dy) = (p[0] - mx, p[1] - my);
        let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
    }
    if !(u1 > u0 && v1 > v0) || !(u1 - u0).is_finite() || !(v1 - v0).is_finite() {
        return None;
    }
    let at = |u: f64, v: f64| -> V2 { [mx + u * c - v * s, my + u * s + v * c] };
    Some([at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)])
}

/// The four segments (eight points) of the outline of a box.
fn box_outline(b: [f64; 4], out: &mut Vec<V2>) {
    let c = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]];
    for k in 0..4 {
        out.push(c[k]);
        out.push(c[(k + 1) % 4]);
    }
}

/// Bounds of points (control points of a path included); `None` when there are none or they are not finite.
fn points_bounds(pts: &[V2]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

impl Built {
    /// Flattened geometry of one primitive. Strokes: segment pairs; fills: triangles (+ outline for filled paths).
    #[cfg(test)]
    fn flatten_prim(rec: &Recorder, p: &Prim, kind: GKind, tol: f64) -> Flat {
        Self::flatten_prim_lod(rec, p, kind, tol, Lod::Full, None)
    }

    /// `flatten_prim` at a level of detail; `bb` is the bounding box of the primitive (for the tolerance of `Simple`).
    fn flatten_prim_lod(rec: &Recorder, p: &Prim, kind: GKind, tol: f64, lod: Lod, bb: Option<&[f64; 4]>) -> Flat {
        let (ty, xy) = rec.elems(p);
        let mut lines: Vec<V2> = Vec::new();
        let mut tris: Vec<V2> = Vec::new();
        // coarse flattening: one step per curve, a tolerance of 1 % of the smaller side of the shape (a letter 2.5 high is
        // flattened to 0.025, a circle to 22 segments) but not finer than 2 x the tolerance of the drawing
        let boxed = kind == GKind::F && p.flags & FLAG_BOX != 0;
        let simple = (lod == Lod::Simple && can_simplify(kind, p)) || boxed;
        let (tol, steps) = if simple {
            let side = bb.map(|b| (b[2] - b[0]).min(b[3] - b[1])).unwrap_or(0.0);
            ((2.0 * tol).max(0.01 * side), 1)
        } else {
            (tol, FULL_STEPS)
        };
        match (kind, p.kind) {
            (GKind::S, K_LINES) => {
                let mut i = 0;
                while i + 1 < xy.len() {
                    if ty[i + 1] != M_MOVE {
                        lines.push(xy[i]);
                        lines.push(xy[i + 1]);
                    }
                    i += 2;
                }
            }
            (GKind::S, K_POLY) => {
                for w in xy.windows(2) {
                    lines.push(w[0]);
                    lines.push(w[1]);
                }
            }
            (GKind::S, _) => {
                for sub in flatten_path(ty, xy, tol, steps) {
                    for w in sub.windows(2) {
                        lines.push(w[0]);
                        lines.push(w[1]);
                    }
                }
            }
            (GKind::P | GKind::I, _) => {
                tris = tess::triangulate_polygon(xy);
            }
            // the last level: the bar of a line of text, the outline of the box of any other shape
            (GKind::F, _) if lod == Lod::Box && can_simplify(kind, p) => {
                let bar = if p.flags & FLAG_TEXT != 0 { oriented_box(xy) } else { None };
                match bar {
                    Some([a, b, c, d]) => tris = vec![a, b, c, a, c, d],
                    None => {
                        if let Some(b) = bb.copied().or_else(|| points_bounds(xy)) {
                            box_outline(b, &mut lines);
                        }
                    }
                }
            }
            (GKind::F, _) => {
                let subs = flatten_path(ty, xy, tol, steps);
                if !simple {
                    for sub in &subs {
                        for w in sub.windows(2) {
                            lines.push(w[0]);
                            lines.push(w[1]);
                        }
                    }
                }
                tris = tess::triangulate_even_odd(&subs);
            }
        }
        Flat { lines, tris }
    }

    /// Port of `BatchBackend.build`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        rec: Recorder,
        lw_scale: f64,
        layers: Vec<Layer>,
        file_visible: &[bool],
        layout: &str,
        layouts: Vec<String>,
        bg: [u8; 3],
        origin_mode: OriginMode,
    ) -> Built {
        Self::build_ctl(rec, lw_scale, layers, file_visible, layout, layouts, bg, origin_mode, &crate::ctl::Ctl::default())
    }

    /// `build` that looks at `ctl` between the pieces it makes: cancelled, the batches made so far are all there is
    /// (`Built::halt` says so); out of time, the scene stops there and a note says that it is incomplete.
    ///
    /// The scene is refused only when the strokes and the boxes of everything else do not fit. Should it overflow although
    /// they do (an estimate of a filled shape was too low, so the shapes before the overflow took the room that the shapes
    /// after it needed), it is made again with a part of the room kept free of filled shapes, and when that is still not
    /// enough with every filled shape as a box (`BuildStats::passes` says how many passes it took).
    #[allow(clippy::too_many_arguments)]
    pub fn build_ctl(
        rec: Recorder,
        lw_scale: f64,
        layers: Vec<Layer>,
        file_visible: &[bool],
        layout: &str,
        layouts: Vec<String>,
        bg: [u8; 3],
        origin_mode: OriginMode,
        ctl: &crate::ctl::Ctl,
    ) -> Built {
        let mut rec = rec;
        let mut reserve = 0u64;
        let mut pass = 1usize;
        loop {
            let (mut built, shortage) = Self::build_pass(rec, lw_scale, layers.clone(), file_visible, layout, layouts.clone(), bg, origin_mode, ctl, reserve);
            built.stats.passes = pass;
            if !built.over_out || shortage == 0 || built.halt.is_some() || pass > RETRY_PASSES {
                return built;
            }
            // the geometry that was made is of no use: it is freed before the next one is made
            let Built { rec: r, .. } = built;
            rec = r;
            pass += 1;
            reserve = if pass > RETRY_PASSES { u64::MAX / 4 } else { reserve.saturating_add(shortage.saturating_mul(2)) };
        }
    }

    /// One pass of `build_ctl`: `reserve` output vertices are kept free of filled shapes (the room the shapes may take is
    /// that much smaller). The second value is how many vertices the scene was found to lack when it overflowed although the
    /// strokes and the boxes fit (what `reserve` has to grow by), 0 when it did not overflow or cannot be helped.
    #[allow(clippy::too_many_arguments)]
    fn build_pass(
        rec: Recorder,
        lw_scale: f64,
        layers: Vec<Layer>,
        file_visible: &[bool],
        layout: &str,
        layouts: Vec<String>,
        bg: [u8; 3],
        origin_mode: OriginMode,
        ctl: &crate::ctl::Ctl,
        reserve: u64,
    ) -> (Built, u64) {
        let nprims = rec.prims.len();
        let mut scene = Scene2d { layers, layout: layout.to_string(), layouts, ..Default::default() };
        if nprims == 0 {
            let built = Built {
                rec,
                bb: Vec::new(),
                level: Vec::new(),
                scene,
                batch_layers: Vec::new(),
                batch_roles: Vec::new(),
                bg_sensitive: false,
                batch_bounds: Vec::new(),
                lw_scale,
                bg,
                origin: [0.0; 2],
                fallback_levels: false,
                notes: Vec::new(),
                halt: None,
                over_out: false,
                stats: BuildStats::default(),
            };
            return (built, 0);
        }
        // bounding boxes of the elements (control points included)
        let mut bb: Vec<[f64; 4]> = Vec::with_capacity(nprims);
        for p in &rec.prims {
            let (_, xy) = rec.elems(p);
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for q in xy {
                b[0] = b[0].min(q[0]);
                b[1] = b[1].min(q[1]);
                b[2] = b[2].max(q[0]);
                b[3] = b[3].max(q[1]);
            }
            bb.push(b);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for b in &bb {
            x0 = x0.min(b[0]);
            y0 = y0.min(b[1]);
            x1 = x1.max(b[2]);
            y1 = y1.max(b[3]);
        }
        let (fw, fh) = (x1 - x0, y1 - y0);
        let pad = 0.003 * fw.max(fh) + 1e-9;
        let gid: Vec<i32> = rec.prims.iter().map(|p| p.gid as i32).collect();
        let mut fallback = false;
        let level = {
            let cell = levels::grid_cell(&bb, pad, VISIT_BUDGET);
            match levels::assign_levels(&bb, &gid, pad, cell) {
                Ok(l) if l.iter().copied().max().unwrap_or(0) < MAX_LEVELS => l,
                _ => {
                    // the legacy renderer of the Python viewer: one level per run of equal groups, exact drawing order
                    fallback = true;
                    let mut lv = Vec::with_capacity(nprims);
                    let mut cur = 0i32;
                    for i in 0..nprims {
                        if i > 0 && gid[i] != gid[i - 1] {
                            cur += 1;
                        }
                        lv.push(cur);
                    }
                    lv
                }
            }
        };
        let origin = match origin_mode {
            OriginMode::World => [0.0, 0.0],
            OriginMode::Center => [((x0 + x1) * 0.5).round(), ((y0 + y1) * 0.5).round()],
        };
        let tol = (fw * fw + fh * fh).sqrt() * 5e-6;
        let max_out = rec.max_out();
        // the room that the filled shapes are planned into: the limit less what a pass before this one found missing, and the
        // detail is reduced from half of it as it is from half of the limit
        let fill_max = max_out.saturating_sub(reserve);
        let soft_plan = if reserve == 0 { rec.soft_out() } else { rec.soft_out().min(fill_max / 2) };
        let est = Est::new(&rec, tol);
        let plan = assign_lod(&rec, &est, soft_plan, fill_max);
        let (mut lod, est_out, min_out) = (plan.lod, plan.est, plan.min);
        let batch_max = rec.limits.batch_max;
        let mut order: Vec<u32> = (0..nprims as u32).collect();
        order.sort_by_key(|&i| level[i as usize]); // stable: drawing order inside a level
        // Morton keys of the primitive centres
        let mort: Vec<u64> = bb
            .iter()
            .map(|b| {
                let cx = (b[0] + b[2]) * 0.5;
                let cy = (b[1] + b[3]) * 0.5;
                let mx = (((cx - x0) * (65535.0 / fw.max(1e-12))).clamp(0.0, 65535.0)) as u32;
                let my = (((cy - y0) * (65535.0 / fh.max(1e-12))).clamp(0.0, 65535.0)) as u32;
                levels::morton(mx, my)
            })
            .collect();

        let mut batches: Vec<DrawBatch> = Vec::new();
        let mut batch_layers: Vec<Vec<u32>> = Vec::new();
        let mut batch_roles: Vec<Role> = Vec::new();
        let mut batch_bounds: Vec<Option<[f64; 4]>> = Vec::new();
        let mut out_verts = 0u64;
        let mut halt: Option<crate::ctl::Halt> = None;
        // Even with every filled shape reduced to its box the scene would be far above the limit (a million strokes of the
        // finest detail on a computer with little memory): it is refused now, before gigabytes of geometry are made that
        // would be thrown away. The strokes and the boxes are counted as they are made; the polygons (SOLID, wide polylines)
        // are counted at up to three times what they make, so only twice the limit counts here (the check after every slice
        // below is exact).
        let mut over_out = min_out > max_out.saturating_mul(2);
        let mut shortage = 0u64;
        let mut stats = BuildStats { est_out, ..BuildStats::default() };
        // what the rest of the scene takes at the least: the boxes of the shapes that can be reduced, the strokes as they are
        let mut remaining_min = min_out;
        let mut start = 0;
        'levels: while start < order.len() && !over_out {
            let lvl = level[order[start] as usize];
            let mut end = start;
            while end < order.len() && level[order[end] as usize] == lvl {
                end += 1;
            }
            let grp: &[u32] = &order[start..end];
            start = end;
            let gdef = &rec.groups[rec.prims[grp[0] as usize].gid as usize];
            let layer = gdef.layers.last().copied().unwrap_or(0);
            if gdef.kind == GKind::I {
                // pictures keep their place in the z-order: after the batches recorded so far
                for &i in grp {
                    let p = &rec.prims[i as usize];
                    let (_, xy) = rec.elems(p);
                    let Some(pic) = rec.images.get(p.aux as usize) else { continue };
                    if xy.len() != 4 {
                        continue;
                    }
                    scene.images.push(DrawImage {
                        layer,
                        after: batches.len() as u32,
                        width: pic.width,
                        height: pic.height,
                        rgba: pic.rgba.clone(),
                        corners: [to_f32(xy[0], origin), to_f32(xy[1], origin), to_f32(xy[2], origin), to_f32(xy[3], origin)],
                    });
                }
                continue;
            }
            let width = if gdef.kind == GKind::P { 0.0 } else { px_width(gdef.lw as f64, lw_scale) };
            // split into parts
            let parts: Vec<Vec<u32>> = match gdef.kind {
                GKind::S => {
                    let mut idx: Vec<u32> = grp.to_vec();
                    if gdef.color[3] == 255 {
                        idx.sort_by_key(|&i| mort[i as usize]);
                    }
                    let mut parts = Vec::new();
                    let mut cur: Vec<u32> = Vec::new();
                    let mut cum = 0usize;
                    let mut cur_chunk = 0usize;
                    for i in idx {
                        let n = rec.prims[i as usize].ecount as usize;
                        let chunk = cum / CHUNK_ELEMS;
                        if chunk != cur_chunk && !cur.is_empty() {
                            parts.push(std::mem::take(&mut cur));
                        }
                        cur_chunk = chunk;
                        cur.push(i);
                        cum += n;
                    }
                    if !cur.is_empty() {
                        parts.push(cur);
                    }
                    parts
                }
                // fills: one batch per level, drawing order inside
                _ => vec![grp.to_vec()],
            };
            let kind = gdef.kind;
            let color = gdef.color;
            for part in parts {
                let new_batch = || DrawBatch { layer, color, width, lines: Vec::new(), fills: Vec::new() };
                let mut b = new_batch();
                // bounds of the batch: bounding boxes of the path elements (control points included), as the
                // batching scene builder of the Python viewer measured its items
                let mut bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                // The primitives are flattened a slice at a time, in parallel, and appended in order: only one slice
                // of flattened geometry is alive at once. Keeping all of it until the end cost a gigabyte for 10 000
                // texts and made the time grow with the square of their number (the heap of Windows slows down with
                // every block that is kept).
                let mut pos = 0;
                while pos < part.len() {
                    if let Some(h) = ctl.halt() {
                        halt = Some(h);
                        break 'levels;
                    }
                    let mut stop = pos;
                    let mut work = 0usize;
                    while stop < part.len() && stop - pos < SLICE_PRIMS && (work < SLICE_ELEMS || stop == pos) {
                        work += rec.prims[part[stop] as usize].ecount as usize;
                        stop += 1;
                    }
                    let slice = &part[pos..stop];
                    stats.slices += 1;
                    stats.max_slice_prims = stats.max_slice_prims.max(slice.len());
                    stats.max_slice_elems = stats.max_slice_elems.max(work);
                    // What is left of the limit for this slice: the vertices made so far and the least that the primitives after
                    // it make are taken off. A slice that would not fit is taken down to boxes (see `Lod::Box`), as far as
                    // it takes: the earlier primitives keep their level, the later ones are boxes.
                    let slice_min = slice.iter().fold(0u64, |m, &i| m.saturating_add(est.boxed(&rec, i as usize)));
                    let after_min = remaining_min.saturating_sub(slice_min);
                    remaining_min = after_min;
                    let room = fill_max.saturating_sub(out_verts + (b.lines.len() + b.fills.len()) as u64).saturating_sub(after_min);
                    if kind == GKind::F {
                        // far too much for what is left (the estimate is at the most twice the size it makes): a level down for
                        // the primitives that can go down, and another one if that is not enough, without making the geometry
                        // that would be thrown away
                        for _ in 0..2 {
                            let planned = slice.iter().fold(0u64, |m, &i| {
                                m.saturating_add(match lod_at(&lod, i) {
                                    Lod::Full => est.full(i as usize),
                                    Lod::Simple => est.simple(&rec, i as usize),
                                    Lod::Box => est.boxed(&rec, i as usize),
                                })
                            });
                            if planned / 2 <= room {
                                break;
                            }
                            for &i in slice {
                                if can_simplify(kind, &rec.prims[i as usize]) {
                                    match lod_at(&lod, i) {
                                        Lod::Full => set_lod(&mut lod, nprims, i, Lod::Simple),
                                        Lod::Simple => set_lod(&mut lod, nprims, i, Lod::Box),
                                        Lod::Box => {}
                                    }
                                }
                            }
                        }
                    }
                    let one = |&i: &u32| Self::flatten_prim_lod(&rec, &rec.prims[i as usize], kind, tol, lod_at(&lod, i), Some(&bb[i as usize]));
                    let mut flats: Vec<Flat> = if slice.len() > 4 && work > 2000 { slice.par_iter().map(one).collect() } else { slice.iter().map(one).collect() };
                    if kind == GKind::F && flats.iter().map(|f| (f.lines.len() + f.tris.len()) as u64).sum::<u64>() > room {
                        // it is more than the estimate said: from the first primitive that leaves too little for the rest of
                        // the slice at its least, the primitives are boxes
                        let mut suffix = vec![0u64; slice.len() + 1];
                        for k in (0..slice.len()).rev() {
                            suffix[k] = suffix[k + 1].saturating_add(est.boxed(&rec, slice[k] as usize));
                        }
                        let mut taken = 0u64;
                        for k in 0..slice.len() {
                            let i = slice[k];
                            let p = &rec.prims[i as usize];
                            let mut size = (flats[k].lines.len() + flats[k].tris.len()) as u64;
                            // a level down at a time, until it leaves enough for the rest of the slice
                            while can_simplify(kind, p) && lod_at(&lod, i) != Lod::Box && taken.saturating_add(size).saturating_add(suffix[k + 1]) > room {
                                let lower = if lod_at(&lod, i) == Lod::Full { Lod::Simple } else { Lod::Box };
                                flats[k] = Self::flatten_prim_lod(&rec, p, kind, tol, lower, Some(&bb[i as usize]));
                                set_lod(&mut lod, nprims, i, lower);
                                size = (flats[k].lines.len() + flats[k].tris.len()) as u64;
                            }
                            taken = taken.saturating_add(size);
                        }
                    }
                    for (k, f) in flats.iter().enumerate() {
                        if f.lines.is_empty() && f.tris.is_empty() {
                            continue;
                        }
                        let pb = &bb[slice[k] as usize];
                        bounds[0] = bounds[0].min(pb[0]);
                        bounds[1] = bounds[1].min(pb[1]);
                        bounds[2] = bounds[2].max(pb[2]);
                        bounds[3] = bounds[3].max(pb[3]);
                        b.lines.extend(f.lines.iter().map(|p| to_f32(*p, origin)));
                        b.fills.extend(f.tris.iter().map(|p| to_f32(*p, origin)));
                    }
                    pos = stop;
                    if out_verts + (b.lines.len() + b.fills.len()) as u64 > max_out {
                        over_out = true;
                        // When the strokes and the boxes of everything would fit, the scene did not because the shapes made before
                        // this point took room that the ones after it needed: what is missing is what was made and what the rest
                        // takes at the least, over the limit. The next pass keeps that much free of filled shapes.
                        if min_out <= max_out {
                            shortage = (out_verts + (b.lines.len() + b.fills.len()) as u64).saturating_add(remaining_min).saturating_sub(max_out).max(1);
                        }
                        break 'levels;
                    }
                    // A batch never grows past `BATCH_MAX_VERTS`: the next one has the same pen and follows it in the order of
                    // drawing, so the picture is the same. A vector that holds all the fills of a level (60 million corners
                    // for 25 000 texts) doubles its size again and again, and while it does the old and the new memory
                    // exist together, a gigabyte more at the peak.
                    if b.lines.len() + b.fills.len() >= batch_max {
                        out_verts += (b.lines.len() + b.fills.len()) as u64;
                        batches.push(std::mem::replace(&mut b, new_batch()));
                        batch_layers.push(gdef.layers.clone());
                        batch_roles.push(gdef.role);
                        batch_bounds.push(if bounds[0].is_finite() { Some(bounds) } else { None });
                        bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
                    }
                }
                // a primitive that flattens to nothing (e.g. a degenerate fill) adds no batch
                if b.lines.is_empty() && b.fills.is_empty() {
                    continue;
                }
                out_verts += (b.lines.len() + b.fills.len()) as u64;
                batches.push(b);
                batch_layers.push(gdef.layers.clone());
                batch_roles.push(gdef.role);
                batch_bounds.push(if bounds[0].is_finite() { Some(bounds) } else { None });
            }
        }
        // bounds of what is shown in the file
        let vis = LayerVis(file_visible.to_vec());
        let mut sb: Option<[f64; 4]> = None;
        for (i, deps) in batch_layers.iter().enumerate() {
            if !vis.is_shown(deps) {
                continue;
            }
            if let Some(b) = batch_bounds[i] {
                sb = Some(match sb {
                    None => b,
                    Some(s) => [s[0].min(b[0]), s[1].min(b[1]), s[2].max(b[2]), s[3].max(b[3])],
                });
            }
        }
        scene.bounds = sb.map(|b| ([b[0], b[1]], [b[2], b[3]]));
        scene.origin = origin;
        scene.batches = batches;
        // what the levels are in the end: the ones the plan made, less the ones that were taken down while the scene was made
        stats.simplified = lod.iter().filter(|l| **l == Lod::Simple).count();
        stats.boxed = lod.iter().filter(|l| **l == Lod::Box).count() + rec.prims.iter().filter(|p| p.flags & FLAG_BOX != 0).count();
        let mut notes = Vec::new();
        if stats.simplified > 0 {
            notes.push(crate::budget::note_simplified(stats.simplified));
        }
        if stats.boxed > 0 {
            notes.push(crate::budget::note_boxed(stats.boxed));
        }
        let built = Built { rec, bb, level, scene, batch_layers, batch_roles, bg_sensitive: false, batch_bounds, lw_scale, bg, origin, fallback_levels: fallback, notes, halt, over_out, stats };
        (built, shortage)
    }

    // ------------------------------------------------------------------------------------------ queries
    pub fn prim_shown(&self, vis: &LayerVis, p: &Prim) -> bool {
        vis.is_shown(&self.rec.groups[p.gid as usize].layers)
    }

    fn half_width(&self, p: &Prim) -> f64 {
        let g = &self.rec.groups[p.gid as usize];
        match g.kind {
            GKind::P | GKind::I => 0.0,
            _ => g.lw as f64 * MM_TO_PX * self.lw_scale * 0.5,
        }
    }

    /// Topmost visible primitive hit by the square `rect` ([x0, y0, x1, y1]). Strokes are hit within their half width
    /// (taken as drawing units, as the Python scene did); fills by their interior.
    pub fn pick(&self, vis: &LayerVis, rect: [f64; 4]) -> Option<u32> {
        let [rx0, ry0, rx1, ry1] = rect;
        let mut best: Option<u32> = None;
        for (i, p) in self.rec.prims.iter().enumerate().rev() {
            let b = &self.bb[i];
            let g = &self.rec.groups[p.gid as usize];
            let hw = self.half_width(p);
            let mg = hw * 1.5;
            if b[0] - mg > rx1 || b[2] + mg < rx0 || b[1] - mg > ry1 || b[3] + mg < ry0 {
                continue;
            }
            if !self.prim_shown(vis, p) {
                continue;
            }
            let hit = match g.kind {
                GKind::S => self.hit_stroke(p, hw, rect),
                GKind::F => self.hit_fill(p, hw, rect),
                GKind::P | GKind::I => self.hit_fill(p, 0.0, rect),
            };
            if hit {
                best = Some(i as u32);
                break; // latest drawn first
            }
        }
        best
    }

    fn hit_stroke(&self, p: &Prim, hw: f64, rect: [f64; 4]) -> bool {
        let (ty, xy) = self.rec.elems(p);
        let e = Elems { ty: ty.to_vec(), xy: xy.to_vec() };
        let subs: Vec<Vec<V2>> = if p.kind == K_LINES {
            let mut v = Vec::new();
            let mut i = 0;
            while i + 1 < xy.len() {
                v.push(vec![xy[i], xy[i + 1]]);
                i += 2;
            }
            v
        } else if p.kind == K_POLY {
            vec![xy.to_vec()]
        } else {
            let tol = hw.max(1e-9) * 0.25 + 1e-9;
            e.flatten(tol.max(1e-6), 4)
        };
        for sub in &subs {
            let n = sub.len();
            let big = n > BIG_PRIM;
            for (k, w) in sub.windows(2).enumerate() {
                let (mut a, mut b) = (w[0], w[1]);
                if big {
                    // only the segments near the rectangle (cheap reject)
                    let lo = [a[0].min(b[0]) - hw * 1.5, a[1].min(b[1]) - hw * 1.5];
                    let hi = [a[0].max(b[0]) + hw * 1.5, a[1].max(b[1]) + hw * 1.5];
                    if lo[0] > rect[2] || hi[0] < rect[0] || lo[1] > rect[3] || hi[1] < rect[1] {
                        continue;
                    }
                }
                // square caps at the open ends of the sub path
                let ext = |from: V2, to: V2| -> V2 {
                    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                    let l = (dx * dx + dy * dy).sqrt();
                    if l == 0.0 {
                        from
                    } else {
                        [from[0] - dx / l * hw, from[1] - dy / l * hw]
                    }
                };
                if k == 0 {
                    a = ext(a, b);
                }
                if k + 2 == n {
                    b = ext(b, w[0]);
                }
                if seg_rect_dist2(a, b, rect) <= hw * hw {
                    return true;
                }
            }
            if n == 1 && point_rect_dist2(sub[0], rect) <= hw * hw {
                return true;
            }
        }
        false
    }

    fn hit_fill(&self, p: &Prim, hw: f64, rect: [f64; 4]) -> bool {
        let (ty, xy) = self.rec.elems(p);
        let e = Elems { ty: ty.to_vec(), xy: xy.to_vec() };
        let subs = e.flatten(1e-6f64.max(hw * 0.05), 4);
        // the rectangle centre and corners decide the interior test (even-odd over all sub paths)
        let c = [(rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5];
        let corners = [c, [rect[0], rect[1]], [rect[2], rect[1]], [rect[2], rect[3]], [rect[0], rect[3]]];
        for q in corners {
            let mut inside = false;
            for s in &subs {
                if point_in_ring(q, s) {
                    inside = !inside;
                }
            }
            if inside {
                return true;
            }
        }
        // edges near the rectangle
        if hw > 0.0 {
            for s in &subs {
                for w in s.windows(2) {
                    if seg_rect_dist2(w[0], w[1], rect) <= hw * hw {
                        return true;
                    }
                }
            }
        }
        // the rectangle may enclose the whole (small) shape
        for s in &subs {
            if s.iter().any(|q| q[0] >= rect[0] && q[0] <= rect[2] && q[1] >= rect[1] && q[1] <= rect[3]) {
                return true;
            }
        }
        false
    }

    /// Snap candidates: on-curve vertices (move / line elements) inside `rect` of the visible primitives that can
    /// snap. Returns (element index, point, is a plain LINE).
    pub fn snap_vertices(&self, vis: &LayerVis, rect: [f64; 4]) -> Vec<(u32, V2, bool)> {
        let mut out = Vec::new();
        for (i, p) in self.rec.prims.iter().enumerate() {
            if p.flags & FLAG_NOSNAP != 0 {
                continue;
            }
            let b = &self.bb[i];
            if b[0] > rect[2] || b[2] < rect[0] || b[1] > rect[3] || b[3] < rect[1] {
                continue;
            }
            if !self.prim_shown(vis, p) {
                continue;
            }
            let (ty, xy) = self.rec.elems(p);
            for (k, q) in xy.iter().enumerate() {
                if ty[k] <= M_LINE && q[0] >= rect[0] && q[0] <= rect[2] && q[1] >= rect[1] && q[1] <= rect[3] {
                    out.push((p.estart + k as u32, *q, p.flags & FLAG_ISLINE != 0));
                }
            }
        }
        out
    }

    /// Nearest snap vertex. `to_px` maps drawing coordinates to viewport pixels; distances are measured between
    /// whole pixels and ties go to the topmost primitive (latest drawn), as in `Batch.snap`.
    /// `lines` false: vertices of paths and polylines, true: end points of plain LINE primitives.
    pub fn snap(&self, vis: &LayerVis, rect: [f64; 4], to_px: &dyn Fn(V2) -> V2, cursor_px: V2, lines: bool) -> Option<(f64, V2)> {
        let cands: Vec<(u32, V2, bool)> = self.snap_vertices(vis, rect).into_iter().filter(|c| c.2 == lines).collect();
        if cands.is_empty() {
            return None;
        }
        let qround = |v: f64| if v >= 0.0 { (v + 0.5).floor() } else { (v - 0.5).ceil() };
        // element index -> primitive index (binary search on estart)
        let prim_of = |e: u32| -> usize { self.rec.prims.partition_point(|p| p.estart <= e) - 1 };
        let mut best: Option<(f64, usize, u32, V2)> = None;
        for &(e, q, _) in &cands {
            let px = to_px(q);
            let (dx, dy) = (qround(px[0]) - cursor_px[0], qround(px[1]) - cursor_px[1]);
            let d = dx * dx + dy * dy;
            let pr = prim_of(e);
            let better = match best {
                None => true,
                Some((bd, bp, be, _)) => {
                    if d < bd {
                        true
                    } else if d > bd {
                        false
                    } else if pr != bp {
                        pr > bp // topmost primitive
                    } else if lines {
                        // the segments of one primitive were separate items: the later one is met first, start before end
                        let s = self.rec.prims[pr].estart;
                        let (rel, brel) = (e - s, be - s);
                        let (key, bkey) = ((-((rel / 2) as i64), rel % 2), (-((brel / 2) as i64), brel % 2));
                        key < bkey
                    } else {
                        e < be
                    }
                }
            };
            if better {
                best = Some((d, pr, e, q));
            }
        }
        best.map(|(d, _, _, q)| (d, q))
    }

    /// Primitives drawn for a top level entity.
    pub fn prims_of_top(&self, top: EntityId) -> Vec<u32> {
        let Some(t) = self.rec.tops.iter().position(|&x| x == top) else { return Vec::new() };
        self.rec.prims.iter().enumerate().filter(|(_, p)| p.top as usize == t).map(|(i, _)| i as u32).collect()
    }

    /// Selection overlay of a top level entity: orange strokes of width `max(2, lineweight)` per width, one batch each.
    /// Polygons (SOLID, wide polylines) have no pen and show no highlight. The outline is bounded: an entity whose outline
    /// has more than `HIGHLIGHT_MAX_POINTS` points (a block reference of thousands of texts) is outlined up to there and
    /// the primitives after that get the rectangle of their bounds, and when even those are too many, one rectangle for
    /// all that is left (`HIGHLIGHT_MAX_BOXES`). The memory and the time of a click on it stay bounded.
    pub fn highlight(&self, vis: &LayerVis, top: EntityId) -> Vec<DrawBatch> {
        self.highlight_with(vis, top, HIGHLIGHT_MAX_POINTS, HIGHLIGHT_MAX_BOXES)
    }

    pub(crate) fn highlight_with(&self, vis: &LayerVis, top: EntityId, max_points: usize, max_boxes: usize) -> Vec<DrawBatch> {
        let prims = self.prims_of_top(top);
        let fw = self.bb.iter().fold([f64::INFINITY, f64::NEG_INFINITY], |a, b| [a[0].min(b[0]), a[1].max(b[2])]);
        let fh = self.bb.iter().fold([f64::INFINITY, f64::NEG_INFINITY], |a, b| [a[0].min(b[1]), a[1].max(b[3])]);
        let tol = ((fw[1] - fw[0]).powi(2) + (fh[1] - fh[0]).powi(2)).sqrt() * 5e-6;
        let mut by_width: Vec<(f32, Vec<[f32; 2]>)> = Vec::new();
        // points of outlines so far; once an outline does not fit, the primitives from there on are drawn as rectangles
        let (mut points, mut outlines, mut boxes) = (0usize, true, 0usize);
        let mut rest: Option<[f64; 4]> = None;
        let rect = |b: &[f64; 4], out: &mut Vec<[f32; 2]>| {
            let c = [to_f32([b[0], b[1]], self.origin), to_f32([b[2], b[1]], self.origin), to_f32([b[2], b[3]], self.origin), to_f32([b[0], b[3]], self.origin)];
            for k in 0..4 {
                out.push(c[k]);
                out.push(c[(k + 1) % 4]);
            }
        };
        fn slot_of(by_width: &mut Vec<(f32, Vec<[f32; 2]>)>, w: f32) -> &mut Vec<[f32; 2]> {
            let at = match by_width.iter().position(|(ww, _)| *ww == w) {
                Some(a) => a,
                None => {
                    by_width.push((w, Vec::new()));
                    by_width.len() - 1
                }
            };
            &mut by_width[at].1
        }
        for i in prims {
            let p = &self.rec.prims[i as usize];
            let g = &self.rec.groups[p.gid as usize];
            if matches!(g.kind, GKind::P | GKind::I) || !self.prim_shown(vis, p) {
                continue;
            }
            let w = (g.lw as f64 * MM_TO_PX * self.lw_scale).max(2.0) as f32;
            if outlines {
                let flat = Self::outline_prim(&self.rec, p, g.kind, tol);
                if points + flat.len() <= max_points {
                    points += flat.len();
                    slot_of(&mut by_width, w).extend(flat.iter().map(|q| to_f32(*q, self.origin)));
                    continue;
                }
                outlines = false;
            }
            let b = &self.bb[i as usize];
            if boxes < max_boxes && b[0].is_finite() && b[2].is_finite() {
                boxes += 1;
                rect(b, slot_of(&mut by_width, w));
            } else if b[0].is_finite() && b[2].is_finite() {
                rest = Some(match rest {
                    None => *b,
                    Some(s) => [s[0].min(b[0]), s[1].min(b[1]), s[2].max(b[2]), s[3].max(b[3])],
                });
            }
        }
        if let Some(r) = rest {
            rect(&r, slot_of(&mut by_width, 2.0));
        }
        by_width
            .into_iter()
            .filter(|(_, l)| !l.is_empty())
            .map(|(w, lines)| DrawBatch { layer: 0, color: SELECTION_COLOR, width: w, lines, fills: Vec::new() })
            .collect()
    }

    /// The outline of a primitive as segment pairs (what the highlight strokes): the segments of a stroke, the edges of the
    /// sub paths of a filled shape. No triangles are made.
    fn outline_prim(rec: &Recorder, p: &Prim, kind: GKind, tol: f64) -> Vec<V2> {
        let (ty, xy) = rec.elems(p);
        let mut lines: Vec<V2> = Vec::new();
        match (kind, p.kind) {
            (GKind::S, K_LINES) => {
                let mut i = 0;
                while i + 1 < xy.len() {
                    if ty[i + 1] != M_MOVE {
                        lines.push(xy[i]);
                        lines.push(xy[i + 1]);
                    }
                    i += 2;
                }
            }
            (GKind::S, K_POLY) => {
                for w in xy.windows(2) {
                    lines.push(w[0]);
                    lines.push(w[1]);
                }
            }
            (GKind::P | GKind::I, _) => {}
            _ => {
                let e = Elems { ty: ty.to_vec(), xy: xy.to_vec() };
                for sub in e.flatten(tol, 4) {
                    for w in sub.windows(2) {
                        lines.push(w[0]);
                        lines.push(w[1]);
                    }
                }
            }
        }
        lines
    }

    /// Bounds of everything drawn for a top level entity that is shown (including polygons).
    pub fn top_bounds(&self, vis: &LayerVis, top: EntityId) -> Option<[f64; 4]> {
        let mut out: Option<[f64; 4]> = None;
        for i in self.prims_of_top(top) {
            let p = &self.rec.prims[i as usize];
            if !self.prim_shown(vis, p) {
                continue;
            }
            let b = self.bb[i as usize];
            out = Some(match out {
                None => b,
                Some(s) => [s[0].min(b[0]), s[1].min(b[1]), s[2].max(b[2]), s[3].max(b[3])],
            });
        }
        out
    }

    /// Bounds of the shown batches (what `LayerMap.bounding_rect` returned).
    pub fn shown_bounds(&self, vis: &LayerVis) -> Option<([f64; 2], [f64; 2])> {
        let mut sb: Option<[f64; 4]> = None;
        for (i, deps) in self.batch_layers.iter().enumerate() {
            if !vis.is_shown(deps) {
                continue;
            }
            if let Some(b) = self.batch_bounds[i] {
                sb = Some(match sb {
                    None => b,
                    Some(s) => [s[0].min(b[0]), s[1].min(b[1]), s[2].max(b[2]), s[3].max(b[3])],
                });
            }
        }
        sb.map(|b| ([b[0], b[1]], [b[2], b[3]]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OriginMode {
    /// Batch coordinates are drawing coordinates.
    World,
    /// Batch coordinates are relative to the (rounded) centre of the drawing (`Built::origin`).
    Center,
}

fn point_rect_dist2(p: V2, r: [f64; 4]) -> f64 {
    let dx = (r[0] - p[0]).max(0.0).max(p[0] - r[2]);
    let dy = (r[1] - p[1]).max(0.0).max(p[1] - r[3]);
    dx * dx + dy * dy
}

/// Squared distance between the segment `a`-`b` and the rectangle (0 when they intersect).
fn seg_rect_dist2(a: V2, b: V2, r: [f64; 4]) -> f64 {
    // intersects the rectangle?
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    let mut inside = true;
    for (p, q) in [(-d[0], a[0] - r[0]), (d[0], r[2] - a[0]), (-d[1], a[1] - r[1]), (d[1], r[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                inside = false;
                break;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    inside = false;
                    break;
                }
                t0 = t0.max(t);
            } else {
                if t < t0 {
                    inside = false;
                    break;
                }
                t1 = t1.min(t);
            }
        }
    }
    if inside && t0 <= t1 {
        return 0.0;
    }
    let corners = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
    let mut best = point_rect_dist2(a, r).min(point_rect_dist2(b, r));
    for c in corners {
        best = best.min(seg_dist2(c, a, b));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5 000 filled squares in a row: the batch holds exactly what flattening every primitive on its own gives, in the
    /// order of the primitives, however the work was cut into slices.
    #[test]
    fn the_slices_of_a_build_append_up_to_the_geometry_of_the_primitives_in_order() {
        let mut rec = Recorder::new();
        let g = rec.group(GKind::F, &[0], [255, 255, 255, 255], 0.0);
        for i in 0..5000 {
            let x = i as f64 * 3.0;
            let mut e = Elems::new();
            e.move_to([x, 0.0]);
            e.line_to([x + 2.0, 0.0]);
            e.curve_to([x + 2.5, 0.3], [x + 2.5, 1.7], [x + 2.0, 2.0]);
            e.line_to([x, 2.0]);
            e.line_to([x, 0.0]);
            rec.path(g, &e);
        }
        let prims = rec.prims.clone();
        // control points count for the bounds of a primitive, as in the build
        let (x0, x1) = (0.0f64, 4999.0 * 3.0 + 2.5);
        let diag = ((x1 - x0).powi(2) + 2.0f64.powi(2)).sqrt();
        let tol = diag * 5e-6;
        let mut want_fills: Vec<[f32; 2]> = Vec::new();
        let mut want_lines: Vec<[f32; 2]> = Vec::new();
        for p in &prims {
            let f = Built::flatten_prim(&rec, p, GKind::F, tol);
            want_fills.extend(f.tris.iter().map(|q| [q[0] as f32, q[1] as f32]));
            want_lines.extend(f.lines.iter().map(|q| [q[0] as f32, q[1] as f32]));
        }
        let b = Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World);
        assert!(b.stats.slices > 1, "{:?}", b.stats);
        assert_eq!(b.scene.batches.len(), 1);
        assert_eq!(b.scene.batches[0].fills, want_fills);
        assert_eq!(b.scene.batches[0].lines, want_lines);
    }

    /// The batches of one level are cut at the size limit: the same pen, the same order of drawing, the same geometry.
    #[test]
    fn a_big_batch_is_cut_into_batches_of_the_same_pen_that_follow_each_other() {
        let make = |batch_max: Option<usize>| {
            let mut rec = Recorder::new();
            if let Some(m) = batch_max {
                rec.set_batch_max(m);
            }
            let g = rec.group(GKind::F, &[0], [255, 255, 255, 255], 0.0);
            let l = rec.group(GKind::S, &[0], [255, 0, 0, 255], 0.25);
            for i in 0..5000 {
                let x = i as f64 * 3.0;
                let mut e = Elems::new();
                e.move_to([x, 0.0]);
                e.line_to([x + 2.0, 0.0]);
                e.line_to([x + 2.0, 2.0]);
                e.line_to([x, 2.0]);
                e.line_to([x, 0.0]);
                rec.path(g, &e);
            }
            // the lines are far from the fills, so that the two pens are two levels and not 10 000
            for i in 0..5000 {
                let x = i as f64 * 3.0;
                rec.line(l, [x, 1000.0], [x + 2.0, 1001.0]);
            }
            Built::build(rec, 1.0, Vec::new(), &[], "Model", vec!["Model".into()], [0, 0, 0], OriginMode::World)
        };
        let whole = make(None);
        let cut = make(Some(20_000));
        let flat = |b: &Built, fills: bool| -> Vec<[f32; 2]> { b.scene.batches.iter().flat_map(|x| if fills { x.fills.clone() } else { x.lines.clone() }).collect() };
        assert!(cut.scene.batches.len() >= whole.scene.batches.len() + 2, "{} {}", cut.scene.batches.len(), whole.scene.batches.len());
        assert_eq!(flat(&cut, true), flat(&whole, true));
        assert_eq!(flat(&cut, false), flat(&whole, false));
        // every piece is about the size that was asked for (a slice of the geometry ends a batch that has passed it), has its
        // own bounds, and its layers; together they are as far as the whole
        for (i, b) in cut.scene.batches.iter().enumerate() {
            // (the strokes are cut into parts of 6 000 elements anyway, the fills are what grows)
            let last_of_pen = cut.scene.batches.get(i + 1).is_none_or(|n| n.color != b.color);
            assert!(b.fills.is_empty() || b.lines.len() + b.fills.len() >= 20_000 || last_of_pen, "{i}: {}", b.lines.len() + b.fills.len());
            assert_eq!(cut.batch_layers[i], vec![0]);
            assert!(cut.batch_bounds[i].is_some());
        }
        assert_eq!(cut.batch_bounds.len(), cut.scene.batches.len());
        assert_eq!(cut.shown_bounds(&LayerVis(vec![true])), whole.shown_bounds(&LayerVis(vec![true])));
        // the drawing order of the pens is kept: the batches of one pen are next to each other
        let pens: Vec<[u8; 4]> = cut.scene.batches.iter().map(|b| b.color).collect();
        let mut runs = pens.clone();
        runs.dedup();
        assert_eq!(runs, vec![[255, 255, 255, 255], [255, 0, 0, 255]]);
    }

    /// A circle of the given radius as four cubic curves (what `arc_path` records).
    fn circle(cx: f64, cy: f64, r: f64) -> Elems {
        const K: f64 = 0.552_284_749_830_793_4;
        let mut e = Elems::new();
        e.move_to([cx + r, cy]);
        e.curve_to([cx + r, cy + K * r], [cx + K * r, cy + r], [cx, cy + r]);
        e.curve_to([cx - K * r, cy + r], [cx - r, cy + K * r], [cx - r, cy]);
        e.curve_to([cx - r, cy - K * r], [cx - K * r, cy - r], [cx, cy - r]);
        e.curve_to([cx + K * r, cy - r], [cx + r, cy - K * r], [cx + r, cy]);
        e
    }

    /// The vertices of a stroke are counted, not guessed: of every shape of stroke and at every tolerance the count is what
    /// flattening it makes. A circle was once counted as 42 vertices (five points per curve) and makes 256 and more.
    #[test]
    fn the_vertices_of_a_stroke_are_counted_exactly() {
        let mut rec = Recorder::new();
        let g = rec.group(GKind::S, &[0], [255; 4], 0.0);
        rec.path(g, &circle(0.0, 0.0, 60.0));
        rec.path(g, &circle(500.0, 0.0, 0.7));
        rec.path(g, &circle(0.0, 500.0, 4000.0));
        // an arc: one curve of a path that starts with a line, and a path that has a second subpath and a lone point
        let mut e = Elems::new();
        e.move_to([0.0, 0.0]);
        e.line_to([10.0, 0.0]);
        e.curve_to([20.0, 0.0], [30.0, 10.0], [30.0, 20.0]);
        e.move_to([50.0, 50.0]);
        e.move_to([60.0, 60.0]);
        e.line_to([70.0, 65.0]);
        e.curve_to([75.0, 70.0], [80.0, 70.0], [85.0, 65.0]);
        rec.path(g, &e);
        // lines only: two subpaths, a lone point, and a line that has no move before it
        let mut e = Elems::new();
        e.move_to([0.0, 0.0]);
        e.line_to([1.0, 1.0]);
        e.line_to([2.0, 0.0]);
        e.move_to([5.0, 5.0]);
        e.move_to([6.0, 6.0]);
        e.line_to([7.0, 6.0]);
        rec.path(g, &e);
        let mut e = Elems::new();
        e.line_to([1.0, 1.0]);
        e.line_to([2.0, 3.0]);
        rec.path(g, &e);
        rec.polyline(g, &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        rec.line(g, [0.0, 0.0], [3.0, 4.0]);
        rec.line(g, [1.0, 1.0], [1.0, 1.0]);
        rec.solid_lines(g, &[([0.0, 0.0], [1.0, 0.0]), ([2.0, 0.0], [3.0, 0.0])]);
        for tol in [1e-6, 1e-3, 0.0286, 5.0, 1e4] {
            let est = Est::new(&rec, tol);
            for (i, p) in rec.prims.iter().enumerate() {
                let made = Built::flatten_prim(&rec, p, GKind::S, tol).lines.len() as u64;
                assert_eq!(est.full(i), made, "primitive {i} at the tolerance {tol}");
            }
        }
        // and what that is: the circle of 60 is far more than the five points per curve it was counted as
        let est = Est::new(&rec, 0.0286);
        assert!(est.full(0) > 100, "{}", est.full(0));
        assert!(est.full(0) > 42 * 4, "{}", est.full(0));
    }

    #[test]
    fn segment_rect_distance() {
        let r = [0.0, 0.0, 10.0, 10.0];
        assert_eq!(seg_rect_dist2([-5.0, 5.0], [15.0, 5.0], r), 0.0);
        assert!((seg_rect_dist2([12.0, 0.0], [12.0, 10.0], r) - 4.0).abs() < 1e-12);
        assert!((seg_rect_dist2([13.0, 14.0], [20.0, 14.0], r) - 25.0).abs() < 1e-12);
        assert_eq!(seg_rect_dist2([5.0, 5.0], [6.0, 6.0], r), 0.0);
    }
}

/// What the tests change about the plan (the thread of the test only).
#[cfg(test)]
pub(crate) mod test_hooks {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        static STROKE_EST_DIV: Cell<u64> = const { Cell::new(1) };
    }

    /// While the guard lives, the curved strokes of the scenes made on this thread are estimated that many times lower than
    /// they are: the estimate that does not hold, which `Built::build_ctl` makes the scene again for.
    pub(crate) struct Underestimate(u64);

    pub(crate) fn underestimate_curved_strokes(div: u64) -> Underestimate {
        Underestimate(STROKE_EST_DIV.with(|d| d.replace(div)))
    }

    impl Drop for Underestimate {
        fn drop(&mut self) {
            STROKE_EST_DIV.with(|d| d.set(self.0));
        }
    }

    pub(super) fn bias(rec: &Recorder, mut full: Vec<u64>) -> Vec<u64> {
        let div = STROKE_EST_DIV.with(|d| d.get());
        if div > 1 {
            for (i, p) in rec.prims.iter().enumerate() {
                if rec.groups[p.gid as usize].kind == GKind::S && p.kind == K_PATH {
                    full[i] = (full[i] / div).max(1);
                }
            }
        }
        full
    }
}
