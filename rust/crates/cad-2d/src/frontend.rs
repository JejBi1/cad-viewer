//! Drawing frontend: decomposes DXF entities into the primitives the batching scene builder records
//! (ezdxf `Frontend` + `RenderPipeline2d` + `fastscene.BatchBackend`).
use crate::bspline::*;
use crate::clip::ClipShape;
use crate::colors::*;
use crate::ctl::{Ctl, Halt};
use crate::dxf::*;
use crate::font::FontOptions;
use crate::geom::*;
use crate::props::*;
use crate::raster::{self, Raster};
use crate::scene::*;
use std::collections::HashMap;
use std::sync::Arc;

// constants of the ezdxf drawing add-on configuration
pub const INFINITE_LINE_LENGTH: f64 = 20.0;
pub const MIN_DASH_LENGTH: f64 = 0.1;
pub const MAX_FLATTENING_DISTANCE: f64 = 0.01;
pub const CIRCLE_APPROXIMATION_COUNT: usize = 128;
pub const MAX_BLOCK_DEPTH: usize = 100;
/// A line that a dash pattern would cut into more pieces than this is drawn solid.
pub const MAX_DASH_PIECES: f64 = 1_000_000.0;
pub const VSF_NON_RECTANGULAR_CLIPPING: i32 = 0x10000;

#[derive(Clone, Debug)]
pub struct Config {
    pub pdmode: i32,
    pub pdsize: f64,
    pub show_defpoints: bool,
    pub lineweight_scaling: f64,
}

struct StackE {
    layer: u32,
    blind: bool,
}

pub struct Frontend<'a> {
    pub doc: &'a Document,
    pub ctx: RenderContext<'a>,
    pub rec: Recorder,
    pub cfg: Config,
    pub fonts: FontOptions,
    pub block_index: HashMap<String, u32>,
    pub layout_keys: Vec<String>,
    pub(crate) ctm: Xform,
    cur_xf: u32,
    /// Clipping shapes (viewport outline, XCLIP), innermost last. Shared: a ring of millions of edges is not copied for
    /// every line that is clipped.
    clip: Vec<Arc<ClipShape>>,
    pub(crate) vp_scale: f64,
    vp_hidden: Vec<u32>,
    stack: Vec<StackE>,
    depth: usize,
    cur_container: Container,
    cur_attr: Option<u32>,
    pub model_space: Option<&'a Vec<Entity>>,
    /// Raster images may be looked up (ImagePolicy.DISPLAY): a missing file is replaced by its name. False: frames only.
    pub images: bool,
    pub doc_dir: Option<std::path::PathBuf>,
    /// File each IMAGEDEF spelling of a name leads to (`None`: not found). Different spellings of one file are one picture.
    raster_paths: HashMap<String, Option<std::path::PathBuf>>,
    /// Decoded pictures by file (`None`: damaged, too large or refused).
    raster_cache: HashMap<std::path::PathBuf, Option<Arc<Raster>>>,
    /// Pixels that may still be decoded in this scene (`raster::MAX_SCENE_PIXELS`), counted before a file is decoded.
    decode_px_left: u64,
    /// Pixels still available for pictures in this scene (`raster::MAX_SCENE_PIXELS`).
    image_px_left: u64,
    /// Cancel flag and time limit of the scene; looked at between entities and in the long loops.
    pub ctl: Ctl,
    tick: u32,
    /// Path elements that dashed lines recorded so far (`DASH_SOFT_ELEMS`).
    pub(crate) dash_elems: usize,
    /// Lines drawn solid because their dashes were too many.
    pub dash_solid: usize,
    /// A picture was blended with the background colour (the fade of an IMAGE): the scene depends on the background.
    pub bg_sensitive: bool,
    /// Hatches filled solid because the pattern lines were too dense or took too long.
    pub hatch_solid: usize,
    /// Time one HATCH may take for all its pattern lines (`hatchdraw::HATCHING_TIMEOUT`).
    pub hatch_timeout: std::time::Duration,
}

/// Port of `ezdxf.render._linetypes._LineTypeRenderer`.
pub struct DashRenderer {
    dashes: Vec<f64>,
    idx: usize,
    cur: f64,
    is_dash: bool,
}

impl DashRenderer {
    pub fn new(dashes: &[f64]) -> DashRenderer {
        DashRenderer { dashes: dashes.to_vec(), idx: 0, cur: dashes.first().copied().unwrap_or(0.0), is_dash: true }
    }

    fn cycle(&mut self) {
        self.idx = (self.idx + 1) % self.dashes.len();
        self.cur = self.dashes[self.idx];
        self.is_dash = !self.is_dash;
    }

    fn render(&mut self, mut length: f64, out: &mut Vec<(bool, f64)>) {
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 10_000_000 {
                return;
            }
            if length <= self.cur {
                self.cur -= length;
                out.push((self.is_dash, length));
                if self.cur == 0.0 {
                    self.cycle();
                }
                return;
            }
            while length > self.cur {
                length -= self.cur;
                let c = self.cur;
                // the recursive call consumes exactly the rest of the current dash
                self.cur = 0.0;
                out.push((self.is_dash, c));
                self.cycle();
                guard += 1;
                if guard > 10_000_000 {
                    return;
                }
            }
            if length > 0.0 {
                continue;
            }
            return;
        }
    }

    /// Number of dash and gap pieces the line `a` -> `b` is cut into (a count, in f64: it may not fit an integer).
    pub fn pieces(&self, a: V2, b: V2) -> f64 {
        let l = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let cycle: f64 = self.dashes.iter().sum();
        if !(cycle > 0.0) {
            return f64::INFINITY;
        }
        l / cycle * self.dashes.len() as f64 + 2.0
    }

    /// The dashes (not the gaps) of the line `a` -> `b`, or false (and nothing) when the line would be cut into more
    /// than `max_pieces` pieces: the caller draws such a line solid, which looks the same at any scale that shows it.
    pub fn line_segment_within(&mut self, a: V2, b: V2, out: &mut Vec<(V2, V2)>, max_pieces: f64) -> bool {
        if !(self.pieces(a, b) <= max_pieces) {
            return false;
        }
        self.line_segment(a, b, out);
        true
    }

    /// The dashes (not the gaps) of the line `a` -> `b`.
    pub fn line_segment(&mut self, a: V2, b: V2, out: &mut Vec<(V2, V2)>) {
        if v2_close(a, b) {
            out.push((a, b));
            return;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt();
        let dir = [dx / l, dy / l];
        let mut parts = Vec::new();
        self.render(l, &mut parts);
        let mut start = a;
        for (is_dash, len) in parts {
            let end = [start[0] + dir[0] * len, start[1] + dir[1] * len];
            if is_dash {
                out.push((start, end));
            }
            start = end;
        }
    }
}

impl<'a> Frontend<'a> {
    pub fn new(doc: &'a Document, light_bg: bool, is_model: bool, fonts: FontOptions, layout_keys: Vec<String>) -> Frontend<'a> {
        let ctx = RenderContext::new(doc, is_model, light_bg);
        let mut pdsize = ctx.pdsize;
        if pdsize <= 0.0 {
            pdsize = 1.0; // relative point sizes are not supported
        }
        let cfg = Config { pdmode: ctx.pdmode, pdsize, show_defpoints: false, lineweight_scaling: 1.0 };
        let block_index = doc.block_order.iter().enumerate().map(|(i, n)| (n.clone(), i as u32)).collect();
        Frontend {
            doc,
            ctx,
            rec: Recorder::new(),
            cfg,
            fonts,
            block_index,
            layout_keys,
            ctm: Xform::IDENTITY,
            cur_xf: 0,
            clip: Vec::new(),
            vp_scale: 1.0,
            vp_hidden: Vec::new(),
            stack: Vec::new(),
            depth: 0,
            cur_container: Container::Layout(0),
            cur_attr: None,
            model_space: doc.layout_entities.get("model"),
            images: false,
            doc_dir: None,
            raster_paths: HashMap::new(),
            raster_cache: HashMap::new(),
            decode_px_left: raster::MAX_SCENE_PIXELS,
            image_px_left: raster::MAX_SCENE_PIXELS,
            ctl: Ctl::default(),
            tick: 0,
            dash_elems: 0,
            dash_solid: 0,
            bg_sensitive: false,
            hatch_solid: 0,
            hatch_timeout: std::time::Duration::from_secs_f64(crate::hatchdraw::HATCHING_TIMEOUT),
        }
    }

    /// Looks at the cancel flag and the time limit; true when the drawing has to stop (the recorder remembers why).
    /// The flag is read every time, the clock on the first call and then every 64 calls.
    pub(crate) fn stop_requested(&mut self) -> bool {
        if self.rec.halt().is_some() {
            return true;
        }
        self.tick = self.tick.wrapping_add(1);
        let halt = if self.ctl.is_cancelled() {
            Some(Halt::Cancelled)
        } else if self.tick % 64 == 1 && self.ctl.is_expired() {
            Some(Halt::TimedOut)
        } else {
            None
        };
        match halt {
            Some(h) => {
                self.rec.set_halt(h);
                true
            }
            None => false,
        }
    }

    /// Czech notes about what was drawn with less detail than the file asks for.
    pub fn notes(&self) -> Vec<String> {
        let mut n = Vec::new();
        if self.dash_solid > 0 {
            n.push(crate::budget::note_dashes(self.dash_solid));
        }
        if self.hatch_solid > 0 {
            n.push(crate::budget::note_hatches(self.hatch_solid));
        }
        n
    }

    // ------------------------------------------------------------------------------------------ pipeline
    fn xy(&self, p: V3) -> V2 {
        let q = self.ctm.apply(p);
        [q[0], q[1]]
    }

    fn layer_key(&self) -> Vec<u32> {
        let mut key: Vec<u32> = Vec::new();
        for e in &self.stack {
            if !e.blind && !key.contains(&e.layer) {
                key.push(e.layer);
            }
        }
        key
    }

    fn gid(&mut self, kind: GKind, props: &Properties) -> u32 {
        let key = self.layer_key();
        self.rec.group_role(kind, &key, props.color, props.lineweight, props.role)
    }

    fn ltype_scale(&self) -> f64 {
        1.0 / self.vp_scale.max(0.0001)
    }

    fn dash_pattern(&self, props: &Properties) -> Vec<f64> {
        if props.pattern.len() < 2 {
            return Vec::new();
        }
        let adj = self.ltype_scale();
        let scale = props.linetype_scale * adj;
        let min_dash = MIN_DASH_LENGTH * adj;
        let mut p: Vec<f64> = props.pattern.iter().map(|e| (e * scale).max(min_dash)).collect();
        if p.len() % 2 == 1 {
            p.pop();
        }
        p
    }

    fn clip_active(&self) -> bool {
        !self.clip.is_empty()
    }

    pub fn out_point(&mut self, p: V3, props: &Properties) {
        let q = self.xy(p);
        for c in &self.clip {
            if !c.contains(q) {
                return;
            }
        }
        let g = self.gid(GKind::S, props);
        self.rec.point(g, q);
    }

    fn lt_line(&mut self, a: V2, b: V2, props: &Properties, pat: &[f64]) {
        let g = self.gid(GKind::S, props);
        if pat.len() < 2 {
            self.rec.line(g, a, b);
        } else {
            let mut r = DashRenderer::new(pat);
            let mut out = Vec::new();
            if r.line_segment_within(a, b, &mut out, self.max_dash_pieces()) {
                self.dash_elems = self.dash_elems.saturating_add(out.len() * 2);
                self.rec.solid_lines(g, &out);
            } else {
                // too many dashes for the scene: one solid line, the entity stays visible
                self.dash_solid += 1;
                self.rec.line(g, a, b);
            }
        }
    }

    /// Most pieces one line may be cut into by its dash pattern: `MAX_DASH_PIECES`, what is left of the dash budget of
    /// the scene (`DASH_SOFT_ELEMS`) and what the scene can still take. A line over it is drawn solid.
    fn max_dash_pieces(&self) -> f64 {
        let budget = self.rec.soft_dash().saturating_sub(self.dash_elems);
        MAX_DASH_PIECES.min(budget as f64 / 2.0).min(self.rec.elems_left() as f64 / 2.0)
    }

    pub fn out_line(&mut self, a: V3, b: V3, props: &Properties) {
        let (a, b) = (self.xy(a), self.xy(b));
        let pat = self.dash_pattern(props);
        let mut segs = vec![(a, b)];
        for c in self.clip.clone().iter().rev() {
            segs = segs.into_iter().flat_map(|(s, e)| c.clip_line(s, e)).collect();
        }
        for (s, e) in segs {
            self.lt_line(s, e, props, &pat);
        }
    }

    /// Lines that are already in their final (WCS) positions, drawn solid.
    pub fn out_solid_lines(&mut self, lines: &[(V3, V3)], props: &Properties) {
        let mut segs: Vec<(V2, V2)> = lines.iter().map(|&(a, b)| (self.xy(a), self.xy(b))).collect();
        for c in self.clip.clone().iter().rev() {
            segs = segs.into_iter().flat_map(|(s, e)| c.clip_line(s, e)).collect();
        }
        if segs.is_empty() {
            return;
        }
        let g = self.gid(GKind::S, props);
        self.rec.solid_lines(g, &segs);
    }

    pub fn out_polyline(&mut self, pts: &[V3], props: &Properties) {
        let xy: Vec<V2> = pts.iter().map(|&p| self.xy(p)).collect();
        self.out_polyline_xy(&xy, props);
    }

    fn out_polyline_xy(&mut self, xy: &[V2], props: &Properties) {
        if xy.len() < 2 {
            return;
        }
        let pat = self.dash_pattern(props);
        let mut parts: Vec<Vec<V2>> = vec![xy.to_vec()];
        for c in self.clip.clone().iter().rev() {
            parts = parts.iter().flat_map(|p| c.clip_polyline(p)).collect();
        }
        for p in parts {
            if pat.len() < 2 {
                let g = self.gid(GKind::S, props);
                self.rec.polyline(g, &p);
            } else {
                let mut r = DashRenderer::new(&pat);
                let mut out = Vec::new();
                let mut left = self.max_dash_pieces();
                let mut solid = false;
                for w in p.windows(2) {
                    if r.line_segment_within(w[0], w[1], &mut out, left) {
                        left -= r.pieces(w[0], w[1]).min(left);
                    } else {
                        // too fine to cut: the segment stays solid
                        solid = true;
                        out.push((w[0], w[1]));
                    }
                }
                if solid {
                    self.dash_solid += 1;
                }
                self.dash_elems = self.dash_elems.saturating_add(out.len() * 2);
                let g = self.gid(GKind::S, props);
                self.rec.solid_lines(g, &out);
            }
        }
    }

    /// `draw_path` of the pipeline: a path in WCS (before the block / viewport transformation).
    pub fn out_path(&mut self, path: &Path, props: &Properties) {
        if path.is_empty() {
            return;
        }
        let pat = self.dash_pattern(props);
        // A curve whose dashes are hopelessly many (a circle of a billion units with a dash of one) is drawn as the curve it
        // is: flattening it first would make a million segments per circle only to draw them solid, and with forty such
        // circles that was gigabytes. The control polygon is never shorter than the curve, so the estimate of the pieces is
        // never too small, and a margin of eight keeps every curve that fits its budget dashed.
        if pat.len() >= 2 && !self.clip_active() {
            let e = path.to_elems(&self.ctm);
            let period: f64 = pat.iter().sum();
            let polygon: f64 = e.xy.windows(2).zip(e.ty.iter().skip(1)).filter(|(_, t)| **t != M_MOVE).map(|(w, _)| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
            if period > 0.0 && polygon / period > 8.0 * self.max_dash_pieces().max(1.0) {
                self.dash_solid += 1;
                let g = self.gid(GKind::S, props);
                self.rec.path(g, &e);
                return;
            }
        }
        let bounds = if self.clip_active() { path.to_elems(&self.ctm).bounds() } else { None };
        // a path whose control points all lie away from a clipping shape is not drawn at all
        if let Some(b) = bounds {
            if self.clip.iter().any(|c| c.excludes_box(b)) {
                return;
            }
        }
        let inside = if self.clip_active() && pat.len() < 2 {
            // a path whose control points all lie inside the clipping shapes is not clipped (and stays curved)
            bounds.map(|b| self.clip.iter().all(|c| c.contains_box(b))).unwrap_or(false)
        } else {
            false
        };
        if (self.clip_active() && !inside) || pat.len() >= 2 {
            let mut e = path.to_elems(&self.ctm);
            let subs = {
                let (d, seg) = if pat.len() >= 2 { (MAX_FLATTENING_DISTANCE, 16) } else { (MAX_FLATTENING_DISTANCE, 4) };
                e.flatten(d, seg)
            };
            e.xy.clear();
            for s in subs {
                self.out_polyline_xy(&s, props);
            }
            return;
        }
        let e = path.to_elems(&self.ctm);
        let g = self.gid(GKind::S, props);
        self.rec.path(g, &e);
    }

    /// Filled paths given as Qt elements in final coordinates.
    pub fn out_filled_elems(&mut self, elems: Vec<Elems>, props: &Properties) {
        self.out_filled_elems_flags(elems, props, 0);
    }

    /// `out_filled_elems` for a primitive with flags (`scene::FLAG_TEXT`: the outlines of a line of text, `FLAG_BOX`: the box
    /// that stands in for them).
    pub fn out_filled_elems_flags(&mut self, mut elems: Vec<Elems>, props: &Properties, flags: u8) {
        if elems.is_empty() {
            return;
        }
        let all_inside = {
            let mut b: Option<[f64; 4]> = None;
            for e in &elems {
                if let Some(eb) = e.bounds() {
                    b = Some(match b {
                        None => eb,
                        Some(s) => [s[0].min(eb[0]), s[1].min(eb[1]), s[2].max(eb[2]), s[3].max(eb[3])],
                    });
                }
            }
            // a shape that lies away from a clipping shape is not drawn at all
            if let Some(b) = b {
                if self.clip.iter().any(|c| c.excludes_box(b)) {
                    return;
                }
            }
            b.map(|b| self.clip.iter().all(|c| c.contains_box(b))).unwrap_or(false)
        };
        if self.clip_active() && !all_inside {
            if self.clip.iter().any(|c| c.is_non_convex()) {
                self.out_filled_pieces(&elems, props);
                return;
            }
            let mut rings: Vec<Vec<V2>> = Vec::new();
            for e in &elems {
                for mut ring in e.flatten(MAX_FLATTENING_DISTANCE, 4) {
                    if let Some(f) = ring.first().copied() {
                        ring.push(f);
                    }
                    let mut parts = vec![ring];
                    for c in self.clip.iter().rev() {
                        parts = parts.iter().flat_map(|r| c.clip_polygon(r)).collect();
                    }
                    rings.extend(parts);
                }
            }
            elems = rings
                .into_iter()
                .map(|r| {
                    let mut e = Elems::new();
                    for (i, p) in r.iter().enumerate() {
                        if i == 0 {
                            e.move_to(*p)
                        } else {
                            e.line_to(*p)
                        }
                    }
                    // a clipped ring is closed again (its last edge is part of the outline)
                    if let (Some(f), Some(l)) = (r.first(), r.last()) {
                        if !v2_close(*f, *l) {
                            e.line_to(*f);
                        }
                    }
                    e
                })
                .collect();
            if elems.is_empty() {
                return;
            }
        }
        let mut all = Elems::new();
        for e in &elems {
            all.extend(e);
        }
        let g = self.gid(GKind::F, props);
        self.rec.path_flags(g, &all, flags);
    }

    /// Filled paths under a clipping shape that is concave or inverted: the fill region is cut into triangles, every
    /// triangle is clipped (exact, the triangle is convex) and drawn as a fill without outline. The outline is the
    /// clipped edges of the rings; the part of the outline that lies along the clipping path is left out (it has the colour of
    /// the fill anyway).
    fn out_filled_pieces(&mut self, elems: &[Elems], props: &Properties) {
        let mut rings: Vec<Vec<V2>> = Vec::new();
        for e in elems {
            for mut ring in e.flatten(MAX_FLATTENING_DISTANCE, 4) {
                if let Some(f) = ring.first().copied() {
                    ring.push(f);
                }
                rings.push(ring);
            }
        }
        let tris = crate::tess::triangulate_even_odd(&rings);
        let mut pieces: Vec<Vec<V2>> = tris.chunks_exact(3).map(|t| t.to_vec()).collect();
        for c in self.clip.iter().rev() {
            pieces = pieces.iter().flat_map(|r| c.clip_polygon(r)).collect();
        }
        let g = self.gid(GKind::P, props);
        for p in &pieces {
            self.rec.filled_polygon(g, p);
        }
        let mut outline = props.clone();
        outline.pattern = std::rc::Rc::new(Vec::new());
        for r in &rings {
            self.out_polyline_xy(r, &outline);
        }
    }

    pub fn out_filled_paths(&mut self, paths: &[Path], props: &Properties) {
        let elems: Vec<Elems> = paths.iter().map(|p| p.to_elems(&self.ctm)).collect();
        self.out_filled_elems(elems, props);
    }

    pub fn out_polygon(&mut self, pts: &[V3], props: &Properties) {
        let xy: Vec<V2> = pts.iter().map(|&p| self.xy(p)).collect();
        self.out_polygon_xy(xy, props);
    }

    pub fn out_polygon_xy(&mut self, xy: Vec<V2>, props: &Properties) {
        if xy.is_empty() {
            return;
        }
        if self.clip_active() {
            let b = xy.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
                [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])]
            });
            if b.iter().all(|v| v.is_finite()) && self.clip.iter().any(|c| c.excludes_box(b)) {
                return;
            }
        }
        let mut parts = vec![xy];
        for c in self.clip.clone().iter().rev() {
            parts = parts.iter().flat_map(|r| c.clip_polygon(r)).collect();
        }
        for p in parts {
            let g = self.gid(GKind::P, props);
            self.rec.filled_polygon(g, &p);
        }
    }

    // ------------------------------------------------------------------------------------------ entity loop
    fn block_container(&self, name: &str) -> Option<(u32, &'a BlockDef)> {
        let key = name.to_lowercase();
        let idx = *self.block_index.get(&key)?;
        self.doc.blocks.get(&key).map(|b| (idx, b))
    }

    /// Entities in drawing order: the SORTENTSTABLE of the layout when there is one, else file order.
    pub fn redraw_order(&self, key: &str, ents: &[Entity]) -> Vec<usize> {
        let table = self
            .doc
            .layouts
            .iter()
            .find(|l| (l.is_model && key == "model") || (!l.is_model && l.name.to_lowercase() == key))
            .and_then(|l| self.doc.sort_tables.get(&l.block_record));
        let mut order: Vec<usize> = (0..ents.len()).collect();
        if let Some(t) = table {
            if !t.is_empty() {
                let sort_handle = |e: &Entity| -> u64 {
                    let h = *t.get(&e.c.handle).unwrap_or(&e.c.handle);
                    if h == 0 {
                        u64::MAX
                    } else {
                        h
                    }
                };
                order.sort_by_key(|&i| (sort_handle(&ents[i]), i));
            }
        }
        order
    }

    pub fn draw_layout(&mut self, key: &str, layout_idx: u32) {
        let Some(ents) = self.doc.layout_entities.get(key) else { return };
        let order = self.redraw_order(key, ents);
        self.cur_container = Container::Layout(layout_idx);
        self.draw_entities(ents, Container::Layout(layout_idx), Some(&order), None);
    }

    pub fn draw_entities(&mut self, ents: &[Entity], container: Container, order: Option<&[usize]>, vp_model: Option<u32>) {
        // Viewports are drawn only at the top level of a paper space layout: the model space shown inside a viewport (or the
        // model space layout itself) may contain VIEWPORT entities, and drawing those would recurse without end.
        let top_level_paper = vp_model.is_none() && !matches!(container, Container::Layout(i) if self.layout_keys.get(i as usize).map(|k| k == "model").unwrap_or(false));
        let mut viewports: Vec<(usize, &Entity)> = Vec::new();
        let n = ents.len();
        let mut i = 0;
        while i < n {
            if self.rec.is_stopped() || self.stop_requested() {
                return;
            }
            let idx = match order {
                Some(o) => o[i],
                None => i,
            };
            i += 1;
            let e = &ents[idx];
            if let Data::Viewport(_) = &e.d {
                if top_level_paper {
                    viewports.push((idx, e));
                }
                continue;
            }
            self.draw_one(e, idx, container, vp_model);
        }
        self.draw_viewports(container, viewports);
    }

    fn draw_one(&mut self, e: &Entity, idx: usize, container: Container, _vp: Option<u32>) {
        if self.rec.is_stopped() || self.stop_requested() {
            return;
        }
        let mut props = self.ctx.resolve(e);
        if self.vp_hidden.contains(&props.layer) && !matches!(e.d, Data::Insert(_) | Data::Face3d { .. }) {
            props.is_visible = false;
        }
        if !props.is_visible {
            return;
        }
        let id = EntityId { container, index: idx as u32 };
        if self.stack.is_empty() {
            self.rec.set_top(id);
        }
        self.rec.set_leaf(Leaf { id, attr: None, xf: self.cur_xf });
        self.cur_attr = None;
        let blind = matches!(&e.d, Data::Insert(i) if !i.table) || matches!(e.d, Data::Face3d { .. } | Data::Viewport(_));
        self.stack.push(StackE { layer: props.layer, blind });
        self.draw_entity(e, &props, id);
        self.stack.pop();
    }

    fn draw_entity(&mut self, e: &Entity, props: &Properties, id: EntityId) {
        let ocs = Ocs::new(e.c.extrusion);
        match &e.d {
            Data::Line { start, end } => self.out_line(*start, *end, props),
            Data::Xline { start, dir, ray } => {
                let d = mul(normalize_or_x(*dir), INFINITE_LINE_LENGTH);
                if *ray {
                    self.out_line(*start, add(*start, d), props);
                } else {
                    self.out_line(sub(*start, mul(d, 0.5)), add(*start, mul(d, 0.5)), props);
                }
            }
            Data::Point { loc, angle } => self.draw_point(e, *loc, *angle, props),
            Data::Circle { center, radius } => {
                let p = arc_path(*center, *radius, e.c.extrusion, 0.0, 360.0);
                self.out_path(&p, props);
            }
            Data::Arc { center, radius, start, end } => {
                let p = arc_path(*center, *radius, e.c.extrusion, *start, *end);
                self.out_path(&p, props);
            }
            Data::Ellipse { center, major, ratio, start, end } => {
                if let Some(p) = ellipse_path(*center, *major, e.c.extrusion, *ratio, *start, *end) {
                    self.out_path(&p, props);
                }
            }
            Data::Spline(s) => {
                if let Some(p) = spline_path(s) {
                    self.out_path(&p, props);
                }
            }
            Data::LwPolyline(p) => self.draw_lwpolyline(e, p, &ocs, props),
            Data::Polyline(p) => self.draw_polyline(e, p, &ocs, props),
            Data::Solid { v, .. } => {
                let mut pts = vec![v[0], v[1], v[2]];
                if v[3] != v[2] {
                    pts.push(v[3]);
                }
                if pts.len() > 3 {
                    pts.swap(2, 3);
                }
                let pts: Vec<V3> = pts.iter().map(|&p| ocs.to_wcs(p)).collect();
                self.out_polygon(&pts, props);
            }
            Data::Face3d { v, edge_flags } => {
                let pts = [v[0], v[1], v[2], v[3], v[0]];
                let vis: Vec<bool> = (0..4).map(|i| edge_flags & (1 << i) == 0).collect();
                if vis.iter().all(|&b| b) {
                    let p = path_from_vertices(&pts, true);
                    self.out_path(&p, props);
                } else {
                    for i in 0..4 {
                        if vis[i] {
                            self.out_line(pts[i], pts[i + 1], props);
                        }
                    }
                }
            }
            Data::Text(t) => self.draw_text_entity(e, t, props),
            Data::MText(m) => self.draw_mtext_entity(e, m, props),
            Data::Insert(ins) => self.draw_insert(e, ins, props, id),
            Data::Dimension(d) => self.draw_dimension(e, d, &ocs, props),
            Data::Leader(l) => self.draw_leader(e, l, props),
            Data::MLeader(m) => self.draw_mleader(e, m, props),
            Data::MLine(m) => self.draw_mline(e, m, &ocs, props),
            Data::Hatch(h) => self.draw_hatch(e, h, &ocs, props),
            Data::Image(im) => self.draw_image(im, props, e.c.transparency),
            Data::Wipeout(im) => {
                let pts = image_boundary(im);
                if pts.len() > 2 {
                    let mut p = props.clone();
                    p.color = rgba(self.ctx.bg, 255);
                    p.role = crate::props::Role::Bg;
                    self.out_polygon(&pts[..pts.len() - 1], &p);
                }
            }
            Data::Mesh(m) => {
                for f in &m.faces {
                    let pts: Vec<V3> = f.iter().filter_map(|&i| m.vertices.get(i as usize).copied()).collect();
                    if pts.len() > 1 {
                        let p = path_from_vertices(&pts, true);
                        self.out_path(&p, props);
                    }
                }
            }
            Data::Viewport(_) | Data::Other(_) => {}
        }
    }

    // ------------------------------------------------------------------------------------------ simple entities
    fn draw_point(&mut self, e: &Entity, loc: V3, angle: f64, props: &Properties) {
        let name = self.doc.layer_names.name(props.layer).to_lowercase();
        let mut pdmode = self.cfg.pdmode;
        if name == "defpoints" {
            if !self.cfg.show_defpoints {
                return;
            }
            pdmode = 0;
        }
        if pdmode == 0 {
            self.out_point(loc, props);
            return;
        }
        let pdsize = self.cfg.pdsize;
        let ocs = Ocs::new(e.c.extrusion);
        let rot = Xform::rot_z(-angle.to_radians());
        let uxf = rot.then(&Xform::from_axes(ocs.ux, ocs.uy, ocs.uz, loc));
        let radius = pdsize * 0.5;
        let style = pdmode & 7;
        let sym = |s: &mut Self, a: V3, b: V3| {
            let (a, b) = (uxf.apply(a), uxf.apply(b));
            if v3_close(a, b) {
                s.out_point(a, props);
            } else {
                s.out_line(a, b, props);
            }
        };
        match style {
            0 => sym(self, NULL3, NULL3),
            2 => {
                sym(self, [-pdsize, 0.0, 0.0], [pdsize, 0.0, 0.0]);
                sym(self, [0.0, -pdsize, 0.0], [0.0, pdsize, 0.0]);
            }
            3 => {
                sym(self, [-pdsize, -pdsize, 0.0], [pdsize, pdsize, 0.0]);
                sym(self, [-pdsize, pdsize, 0.0], [pdsize, -pdsize, 0.0]);
            }
            4 => sym(self, NULL3, [0.0, radius, 0.0]),
            _ => {}
        }
        if pdmode & 64 != 0 {
            let c = [[-radius, -radius], [radius, -radius], [radius, radius], [-radius, radius]];
            for i in 0..4 {
                let (a, b) = (c[i], c[(i + 1) % 4]);
                sym(self, [a[0], a[1], 0.0], [b[0], b[1], 0.0]);
            }
        }
        if pdmode & 32 != 0 {
            let (center, ext) = if e.c.extrusion != Z_AXIS { (ocs.from_wcs(loc), e.c.extrusion) } else { (loc, Z_AXIS) };
            let p = arc_path(center, radius, ext, 0.0, 360.0);
            self.out_path(&p, props);
        }
    }

    fn draw_lwpolyline(&mut self, e: &Entity, p: &LwPoly, ocs: &Ocs, props: &Properties) {
        let has_width = p.const_width != 0.0 || p.points.iter().any(|v| v.start_width != 0.0 || v.end_width != 0.0);
        if has_width {
            let pts: Vec<(V2, f64, f64, f64)> = p
                .points
                .iter()
                .map(|v| {
                    if p.const_width != 0.0 {
                        ([v.x, v.y], p.const_width, p.const_width, v.bulge)
                    } else {
                        ([v.x, v.y], v.start_width, v.end_width, v.bulge)
                    }
                })
                .collect();
            self.draw_banded(&pts, p.closed, ocs, p.elevation, props, e);
            return;
        }
        let pts: Vec<(f64, f64, f64)> = p.points.iter().map(|v| (v.x, v.y, v.bulge)).collect();
        let path = polyline_path(&pts, p.closed, ocs, p.elevation);
        if !path.is_empty() {
            self.out_path(&path, props);
        }
    }

    fn draw_polyline(&mut self, e: &Entity, p: &PolylineData, ocs: &Ocs, props: &Properties) {
        if p.is_polymesh() || p.is_polyface() {
            for face in polyface_faces(p) {
                if face.len() > 1 {
                    let path = path_from_vertices(&face, true);
                    self.out_path(&path, props);
                }
            }
            return;
        }
        if p.vertices.is_empty() {
            return;
        }
        let has_width = p.default_start_width != 0.0
            || p.default_end_width != 0.0
            || p.vertices.iter().any(|v| v.start_width != 0.0 || v.end_width != 0.0);
        if has_width && !p.is_3d() {
            let elev = if p.has_elevation { p.elevation[2] } else { p.vertices[0].loc[2] };
            let pts: Vec<(V2, f64, f64, f64)> = p
                .vertices
                .iter()
                .filter(|v| v.flags & (1 | 16) == 0)
                .map(|v| {
                    let sw = if v.start_width != 0.0 { v.start_width } else { p.default_start_width };
                    let ew = if v.end_width != 0.0 { v.end_width } else { p.default_end_width };
                    ([v.loc[0], v.loc[1]], sw, ew, v.bulge)
                })
                .collect();
            self.draw_banded(&pts, p.is_closed(), ocs, elev, props, e);
            return;
        }
        if p.is_3d() {
            let pts: Vec<V3> = p.vertices.iter().map(|v| v.loc).collect();
            let path = path_from_vertices(&pts, p.is_closed());
            self.out_path(&path, props);
            return;
        }
        let elevation = if p.has_elevation { p.elevation[2] } else { p.vertices[0].loc[2] };
        let pts: Vec<(f64, f64, f64)> = p.vertices.iter().map(|v| (v.loc[0], v.loc[1], v.bulge)).collect();
        let path = polyline_path(&pts, p.is_closed(), ocs, elevation);
        if !path.is_empty() {
            self.out_path(&path, props);
        }
    }

    /// Polylines with width: one filled quadrilateral per segment with mitred joints (ezdxf `TraceBuilder`).
    fn draw_banded(&mut self, pts: &[(V2, f64, f64, f64)], closed: bool, ocs: &Ocs, elevation: f64, props: &Properties, _e: &Entity) {
        if pts.len() < 2 {
            return;
        }
        // stations: (point, start width of the following segment, end width)
        let mut st: Vec<(V2, f64, f64)> = Vec::new();
        let mut list: Vec<(V2, f64, f64, f64)> = pts.to_vec();
        if closed && !v2_close(list[0].0, list[list.len() - 1].0) {
            list.push(list[0]);
        }
        let n = list.len();
        for i in 0..n {
            let (p, sw, ew, bulge) = list[i];
            if bulge != 0.0 && i + 1 < n {
                let q = list[i + 1].0;
                let (c, a0, a1, r) = bulge_to_arc(p, q, bulge);
                if r > 0.0 {
                    let span = arc_span_rad(a0, a1);
                    let segs = ((CIRCLE_APPROXIMATION_COUNT as f64 * span / std::f64::consts::TAU).ceil() as usize).max(3);
                    let mut arc: Vec<V2> = (0..=segs)
                        .map(|k| {
                            let a = fmod(a0, std::f64::consts::TAU) + span * k as f64 / segs as f64;
                            [c[0] + r * a.cos(), c[1] + r * a.sin()]
                        })
                        .collect();
                    if bulge < 0.0 {
                        arc.reverse();
                    }
                    if v2_close(arc[0], q) {
                        arc.reverse();
                    }
                    for (k, a) in arc.iter().enumerate().take(segs) {
                        let t0 = k as f64 / segs as f64;
                        let t1 = (k + 1) as f64 / segs as f64;
                        st.push((*a, sw + (ew - sw) * t0, sw + (ew - sw) * t1));
                    }
                    continue;
                }
            }
            st.push((p, sw, ew));
        }
        // merge stations that are too close
        let mut cl: Vec<(V2, f64, f64)> = Vec::new();
        for s in st {
            if let Some(l) = cl.last() {
                if v2_close(l.0, s.0) {
                    cl.pop();
                }
            }
            cl.push(s);
        }
        if cl.len() < 2 {
            return;
        }
        let m = cl.len() - 1;
        let is_closed = v2_close(cl[0].0, cl[m].0);
        // offset points of every segment
        let seg: Vec<[V2; 4]> = (0..m)
            .map(|i| {
                let (a, sw, ew) = cl[i];
                let b = cl[i + 1].0;
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l = (dx * dx + dy * dy).sqrt();
                let o = [-dy / l, dx / l];
                let (s, e) = ([o[0] * sw / 2.0, o[1] * sw / 2.0], [o[0] * ew / 2.0, o[1] * ew / 2.0]);
                [[a[0] + s[0], a[1] + s[1]], [b[0] + e[0], b[1] + e[1]], [a[0] - s[0], a[1] - s[1]], [b[0] - e[0], b[1] - e[1]]]
            })
            .collect();
        let ray = |i: usize, side: usize| -> (V2, V2) {
            // side 0: up (left) ray of segment i, side 1: down (right)
            let (p, q) = if side == 0 { (seg[i][0], seg[i][1]) } else { (seg[i][2], seg[i][3]) };
            if v2_close(p, q) {
                let a = cl[i].0;
                let b = cl[i + 1].0;
                (p, [p[0] + (b[0] - a[0]), p[1] + (b[1] - a[1])])
            } else {
                (p, q)
            }
        };
        let inter = |r1: (V2, V2), r2: (V2, V2), default: V2| -> V2 {
            let d1 = [r1.1[0] - r1.0[0], r1.1[1] - r1.0[1]];
            let d2 = [r2.1[0] - r2.0[0], r2.1[1] - r2.0[1]];
            let l1 = (d1[0] * d1[0] + d1[1] * d1[1]).sqrt();
            let l2 = (d2[0] * d2[0] + d2[1] * d2[1]).sqrt();
            if l1 == 0.0 || l2 == 0.0 {
                return default;
            }
            let cosang = ((d1[0] * d2[0] + d1[1] * d2[1]) / (l1 * l2)).clamp(-1.0, 1.0);
            let ang = cosang.acos();
            if ang < 0.031415 || (std::f64::consts::PI - ang).abs() < 0.031415 {
                return default;
            }
            let den = d1[0] * d2[1] - d1[1] * d2[0];
            if den.abs() < 1e-300 {
                return default;
            }
            let t = ((r2.0[0] - r1.0[0]) * d2[1] - (r2.0[1] - r1.0[1]) * d2[0]) / den;
            [r1.0[0] + d1[0] * t, r1.0[1] + d1[1] * t]
        };
        let to_w = |p: V2| ocs.to_wcs([p[0], p[1], elevation]);
        for i in 0..m {
            let [up1, up2, down1, down2] = seg[i];
            let (v0, v1) = if i == 0 {
                if is_closed {
                    (inter(ray(m - 1, 0), ray(0, 0), up1), inter(ray(m - 1, 1), ray(0, 1), down1))
                } else {
                    (up1, down1)
                }
            } else {
                (inter(ray(i - 1, 0), ray(i, 0), up1), inter(ray(i - 1, 1), ray(i, 1), down1))
            };
            let (v2, v3) = if i + 1 < m {
                (inter(ray(i + 1, 1), ray(i, 1), down2), inter(ray(i + 1, 0), ray(i, 0), up2))
            } else if is_closed {
                (inter(ray(0, 1), ray(i, 1), down2), inter(ray(0, 0), ray(i, 0), up2))
            } else {
                (down2, up2)
            };
            let quad = [to_w(v0), to_w(v1), to_w(v2), to_w(v3)];
            self.out_polygon(&quad, props);
        }
    }

    // ------------------------------------------------------------------------------------------ INSERT
    fn draw_insert(&mut self, e: &Entity, ins: &InsertData, props: &Properties, id: EntityId) {
        if ins.from_inventor {
            return; // unknown entity, replaced by a plain INSERT next to it
        }
        let Some((bidx, block)) = self.block_container(&ins.name) else { return };
        if self.depth >= MAX_BLOCK_DEPTH {
            return;
        }
        let ocs = Ocs::new(e.c.extrusion);
        self.ctx.block_stack.push(props.clone());
        self.depth += 1;
        let saved_ctm = self.ctm;
        let saved_xf = self.cur_xf;
        let multi = ins.col_count > 1 || ins.row_count > 1;
        // only the locations of the array that differ (a zero spacing puts a whole row or column on one spot), see
        // `InsertData::grid`; the set catches the rest (ezdxf `multi_insert`)
        let (cols, rows) = ins.grid();
        if cols.saturating_mul(rows) > crate::budget::MAX_EXPANDED_ENTITIES {
            // the entity budget refuses such a drawing before it is drawn; this guards a caller that skipped it
            self.rec.mark_over();
        }
        let mut done: std::collections::HashSet<[u64; 2]> = std::collections::HashSet::new();
        'grid: for row in 0..rows {
            for col in 0..cols {
                if self.rec.is_stopped() || self.stop_requested() {
                    break 'grid;
                }
                let mut off = [col as f64 * ins.col_spacing, row as f64 * ins.row_spacing];
                if multi {
                    // -0.0 and 0.0 are one location
                    let key = [(off[0] + 0.0).to_bits(), (off[1] + 0.0).to_bits()];
                    if !done.insert(key) {
                        continue;
                    }
                    if ins.rotation != 0.0 {
                        let (s, c) = ins.rotation.to_radians().sin_cos();
                        off = [off[0] * c - off[1] * s, off[0] * s + off[1] * c];
                    }
                }
                let insert_pt = [ins.insert[0] + off[0], ins.insert[1] + off[1], ins.insert[2]];
                let m = insert_matrix(&ocs, insert_pt, ins.scale, ins.rotation, block.base_point);
                self.ctm = m.then(&saved_ctm);
                self.cur_xf = self.rec.push_xform(self.ctm);
                let clipped = self.push_xclip(ins);
                let ents: &[Entity] = &block.entities;
                for (j, be) in ents.iter().enumerate() {
                    if matches!(&be.d, Data::Text(t) if t.kind == TextKind::Attdef) {
                        continue;
                    }
                    self.draw_one(be, j, Container::Block(bidx), None);
                }
                if clipped {
                    self.clip.pop();
                    self.draw_xclip_frame(ins, props, id);
                }
            }
        }
        // ATTRIBs are located outside of the block reference: parent coordinates
        self.ctm = saved_ctm;
        self.cur_xf = saved_xf;
        for (k, a) in ins.attribs.iter().enumerate() {
            let mut p = self.ctx.resolve(a);
            if self.vp_hidden.contains(&p.layer) {
                p.is_visible = false;
            }
            if !p.is_visible {
                continue;
            }
            self.rec.set_leaf(Leaf { id, attr: Some(k as u32), xf: self.cur_xf });
            self.stack.push(StackE { layer: p.layer, blind: false });
            self.draw_entity(a, &p, id);
            self.stack.pop();
        }
        self.depth -= 1;
        self.ctx.block_stack.pop();
    }

    /// The polygon that is left of an XCLIP boundary after the inner polygon of an inverted clip was chosen: the smaller
    /// (by bounding box) of the clipping polygon and the inverted clip (ezdxf `ClippingPath.inner_polygon`). `f` maps
    /// block coordinates to the coordinates wanted.
    fn xclip_inner(c: &XClip, f: &dyn Fn(&[f64; 2]) -> V2) -> Vec<V2> {
        let poly: Vec<V2> = c.polygon.iter().map(f).collect();
        if !c.inverted {
            return poly;
        }
        let alt: Vec<V2> = c.inverted_clip.iter().map(f).collect();
        let area = |p: &[V2]| {
            if p.is_empty() {
                return 0.0;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for q in p {
                x0 = x0.min(q[0]);
                y0 = y0.min(q[1]);
                x1 = x1.max(q[0]);
                y1 = y1.max(q[1]);
            }
            (x1 - x0) * (y1 - y0)
        };
        if alt.len() >= 3 && area(&alt) < area(&poly) {
            alt
        } else {
            poly
        }
    }

    /// Pushes the clipping shape of the block clipping path (XCLIP) of an INSERT, in the coordinates of the instance that
    /// `self.ctm` places. False when the INSERT has no enabled clipping path.
    fn push_xclip(&mut self, ins: &InsertData) -> bool {
        let Some(c) = ins.clip.as_ref().filter(|c| c.enabled) else { return false };
        let ctm = self.ctm;
        let to_xy = move |p: &[f64; 2]| -> V2 {
            let q = ctm.apply([p[0], p[1], 0.0]);
            [q[0], q[1]]
        };
        let shape = if c.inverted && c.inverted_compare.len() >= 2 {
            let compare: Vec<V2> = c.inverted_compare.iter().map(to_xy).collect();
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for q in &compare {
                b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
            }
            ClipShape::inverted(Self::xclip_inner(c, &to_xy), b)
        } else {
            ClipShape::polygon(c.polygon.iter().map(to_xy).collect())
        };
        match shape {
            Some(s) => {
                self.clip.push(Arc::new(s));
                true
            }
            None => false,
        }
    }

    /// The frame of the clipping path, drawn when `$XCLIPFRAME` is not 0 (ezdxf draws it with the clipping shape still
    /// pushed, where it survives because collinear edges count as inside; it is drawn unclipped here).
    fn draw_xclip_frame(&mut self, ins: &InsertData, props: &Properties, id: EntityId) {
        let Some(c) = ins.clip.as_ref().filter(|c| c.enabled) else { return };
        if self.doc.header.num("$XCLIPFRAME").unwrap_or(2.0) == 0.0 {
            return;
        }
        let pts: Vec<V3> = Self::xclip_inner(c, &|p: &[f64; 2]| [p[0], p[1]]).iter().map(|p| [p[0], p[1], 0.0]).collect();
        if pts.len() < 2 {
            return;
        }
        let path = path_from_vertices(&pts, true);
        self.rec.set_leaf(Leaf { id, attr: None, xf: self.cur_xf });
        self.stack.push(StackE { layer: props.layer, blind: false });
        self.out_path(&path, props);
        self.stack.pop();
    }

    // ------------------------------------------------------------------------------------------ DIMENSION
    fn draw_dimension(&mut self, e: &Entity, d: &DimData, ocs: &Ocs, _props: &Properties) {
        let Some((bidx, block)) = self.block_container(&d.block) else { return };
        if self.depth >= MAX_BLOCK_DEPTH {
            return;
        }
        let _ = e;
        let saved = (self.ctm, self.cur_xf);
        let mut m = Xform::IDENTITY;
        if ocs.transform {
            let elevation = d.text_midpoint[2];
            m = Xform::scale([1.0, 1.0, 0.0]).then(&Xform::translate([0.0, 0.0, elevation])).then(&ocs.xform());
        }
        if let Some(ins) = d.insert {
            let w = ocs.to_wcs(ins);
            m = m.then(&Xform::translate(w));
        }
        self.depth += 1;
        self.ctm = m.then(&saved.0);
        self.cur_xf = self.rec.push_xform(self.ctm);
        let ents: &[Entity] = &block.entities;
        for (j, be) in ents.iter().enumerate() {
            // content entities of a dimension are copies: their ATTDEF children (if any) are not drawn either
            self.draw_one(be, j, Container::Block(bidx), None);
        }
        self.depth -= 1;
        self.ctm = saved.0;
        self.cur_xf = saved.1;
    }

    // ------------------------------------------------------------------------------------------ viewports
    fn draw_viewports(&mut self, container: Container, mut vps: Vec<(usize, &Entity)>) {
        vps.retain(|(_, e)| matches!(&e.d, Data::Viewport(v) if v.status > 0));
        vps.sort_by_key(|(_, e)| match &e.d {
            Data::Viewport(v) => v.status,
            _ => 0,
        });
        if vps.is_empty() {
            return;
        }
        // the "active" viewport (status 1) is the layout as a whole
        if matches!(&vps[0].1.d, Data::Viewport(v) if v.status == 1) {
            vps.remove(0);
        }
        for (idx, e) in vps {
            // the clipping outline of a viewport can be a ring of millions of edges: stop before making the next one
            if self.rec.is_stopped() || self.stop_requested() {
                break;
            }
            if let Data::Viewport(v) = &e.d {
                self.draw_viewport(v, container, idx);
            }
        }
    }

    fn draw_viewport(&mut self, vp: &ViewportData, _container: Container, _idx: usize) {
        if !vp.is_top_view() {
            return;
        }
        let msp = match self.model_space {
            Some(m) => m,
            None => return,
        };
        if vp.width == 0.0 || vp.height == 0.0 {
            return;
        }
        // scale and transformation model space -> paper space
        let scale = if vp.view_height.abs() < 1e-12 { 0.0 } else { vp.height / vp.view_height };
        let vc = [vp.view_center[0], vp.view_center[1], 0.0];
        let offset = sub(vp.center, mul(vc, scale));
        let mut m = Xform::translate(mul(vp.view_target, -1.0)).then(&Xform::scale([scale, scale, scale]));
        if vp.twist_angle != 0.0 {
            m = m.then(&Xform::rot_z(vp.twist_angle.to_radians()));
        }
        m = m.then(&Xform::translate([offset[0], offset[1], 0.0]));
        // clipping path in paper space: rectangle or the boundary entity
        let (cx, cy, w2, h2) = (vp.center[0], vp.center[1], vp.width / 2.0, vp.height / 2.0);
        let mut ring: Vec<V2> = vec![[cx - w2, cy - h2], [cx + w2, cy - h2], [cx + w2, cy + h2], [cx - w2, cy + h2]];
        if vp.flags & VSF_NON_RECTANGULAR_CLIPPING != 0 && vp.clip_handle != 0 {
            if let Some(pts) = self.clip_entity_path(vp.clip_handle) {
                ring = pts;
            }
        }
        let Some(shape) = ClipShape::polygon(ring) else { return };
        let frozen: Vec<u32> = vp.frozen_layers.iter().filter_map(|h| self.doc.layer_by_handle.get(h).copied()).collect();
        let saved = (self.ctm, self.cur_xf, self.vp_scale, std::mem::take(&mut self.vp_hidden));
        self.clip.push(Arc::new(shape));
        self.vp_scale = scale;
        self.vp_hidden = frozen;
        self.ctm = m;
        self.cur_xf = self.rec.push_xform(m);
        let model_idx = self.layout_keys.iter().position(|k| k == "model").unwrap_or(0) as u32;
        self.draw_entities(msp, Container::Layout(model_idx), None, Some(model_idx));
        self.clip.pop();
        self.ctm = saved.0;
        self.cur_xf = saved.1;
        self.vp_scale = saved.2;
        self.vp_hidden = saved.3;
    }

    fn clip_entity_path(&self, handle: u64) -> Option<Vec<V2>> {
        let (key, idx) = self.doc.handle_index.get(&handle)?;
        let e = self.doc.layout_entities.get(key)?.get(*idx)?;
        let ocs = Ocs::new(e.c.extrusion);
        let path = match &e.d {
            Data::LwPolyline(p) => {
                let pts: Vec<(f64, f64, f64)> = p.points.iter().map(|v| (v.x, v.y, v.bulge)).collect();
                polyline_path(&pts, p.closed, &ocs, p.elevation)
            }
            Data::Polyline(p) => {
                let pts: Vec<(f64, f64, f64)> = p.vertices.iter().map(|v| (v.loc[0], v.loc[1], v.bulge)).collect();
                polyline_path(&pts, p.is_closed(), &ocs, if p.has_elevation { p.elevation[2] } else { 0.0 })
            }
            Data::Circle { center, radius } => arc_path(*center, *radius, e.c.extrusion, 0.0, 360.0),
            _ => return None,
        };
        let pts: Vec<V2> = path.flatten(0.1, 4).iter().map(|p| [p[0], p[1]]).collect();
        if pts.len() >= 3 {
            Some(pts)
        } else {
            None
        }
    }

    // ------------------------------------------------------------------------------------------ IMAGE
    /// IMAGE entity as ezdxf's Qt backend draws it (`ImagePolicy.DISPLAY`): the picture when the file can be read, else
    /// its file name in a box. The frame is drawn above the picture in both cases. Remote names never reach this point
    /// with `images` set: the whole drawing then gets frames only, as the Python viewer did.
    fn draw_image(&mut self, im: &ImageData, props: &Properties, transparency: Option<i64>) {
        if self.images && im.flags & 1 != 0 {
            if let Some(def) = self.doc.image_defs.get(&im.def_handle) {
                let filename = def.filename.clone();
                if !self.place_picture(im, &filename, transparency) {
                    self.draw_missing_image_name(im, &filename, props);
                }
            }
        }
        let pts = image_boundary(im);
        let lines: Vec<(V3, V3)> = pts.windows(2).map(|w| (w[0], w[1])).collect();
        self.out_solid_lines(&lines, props);
    }

    /// File name of a picture that cannot be shown, centred in the image box (ezdxf `show_filename_if_missing`).
    fn draw_missing_image_name(&mut self, im: &ImageData, filename: &str, props: &Properties) {
        let name = if filename.trim().is_empty() { "<no filename>".to_string() } else { filename.to_string() };
        let font = crate::font::default_font(&self.fonts);
        let cap = 20.0;
        let tw = font.text_width(&name, cap, 1.0);
        if tw > 0.0 {
            let desired = im.size[0] * 0.75;
            let s = desired / tw;
            let tx = Xform::scale([s, s, 1.0])
                .then(&Xform::translate([(im.size[0] - desired) / 2.0, (im.size[1] - cap * s) / 2.0, 0.0]))
                .then(&Xform::from_axes(im.u, im.v, [0.0, 0.0, 1.0], im.insert));
            self.draw_text_line(&name, &tx, props, cap, &font, 1.0, false);
        }
    }

    /// Decoded picture of an IMAGEDEF file name. Every file is decoded once per scene, however many spellings of its name
    /// the drawing uses (`pic.png`, `./pic.png`, `PIC.PNG`), and only while the pixels of all decoded files stay within
    /// `raster::MAX_SCENE_PIXELS`: the size is read from the header before the picture is decoded.
    fn raster_of(&mut self, filename: &str) -> Option<Arc<Raster>> {
        if !self.raster_paths.contains_key(filename) {
            let path = raster::resolve_path(self.doc_dir.as_deref(), filename, &mut |p| p.is_file()).map(|p| std::fs::canonicalize(&p).unwrap_or(p));
            self.raster_paths.insert(filename.to_string(), path);
        }
        let path = self.raster_paths.get(filename)?.clone()?;
        if let Some(r) = self.raster_cache.get(&path) {
            return r.clone();
        }
        let found = raster::load_file_within(&path, self.decode_px_left).map(|r| {
            self.decode_px_left = self.decode_px_left.saturating_sub(u64::from(r.width) * u64::from(r.height));
            Arc::new(r)
        });
        self.raster_cache.insert(path, found.clone());
        found
    }

    /// Records the picture of an IMAGE entity with its effects (ezdxf `draw_image_entity`): brightness, contrast, fade,
    /// transparency and the clipping mask. Clipping by a viewport or block reference clips the picture boundary first,
    /// which turns the mask on (ezdxf `_clip_image_polygon`). False when the picture cannot be used.
    fn place_picture(&mut self, im: &ImageData, filename: &str, transparency: Option<i64>) -> bool {
        let Some(picture) = self.raster_of(filename) else { return false };
        let (w, h) = (picture.width, picture.height);
        let px = u64::from(w) * u64::from(h);
        if px > self.image_px_left {
            return false;
        }
        self.image_px_left -= px;
        let mut rgba = picture.rgba.clone();
        raster::adjust_colors(&mut rgba, im.brightness, im.contrast, im.fade, self.ctx.bg);
        if im.fade != 0 {
            self.bg_sensitive = true;
        }
        // the entity's own transparency (DXF 440): the alpha is in the low byte; "by block" means opaque
        let factor = match transparency {
            Some(raw) if raw != TRANSPARENCY_BYBLOCK => f64::from((raw & 0xFF) as u32) / 255.0,
            _ => 1.0,
        };
        raster::apply_alpha(&mut rgba, im.flags & 8 != 0, factor);

        let map = self.picture_map(im, h);
        let full = [[-0.5, -0.5], [f64::from(w) - 0.5, -0.5], [f64::from(w) - 0.5, f64::from(h) - 0.5], [-0.5, f64::from(h) - 0.5]];
        let mut keep_inside: Vec<Vec<V2>> = vec![picture_boundary(im)];
        let mut visible: Option<Vec<Vec<V2>>> = None;
        let mut forced = false;
        if map.invertible() && !self.clip.is_empty() {
            let ring: Vec<V2> = keep_inside[0].iter().map(|&p| map.to_xy(p)).collect();
            let parts = self.clip_xy(ring.clone());
            let unchanged = parts.len() == 1 && parts[0].len() == ring.len() && parts[0].iter().zip(&ring).all(|(a, b)| v2_close(*a, *b));
            if !unchanged {
                forced = true;
                keep_inside = parts.iter().map(|r| r.iter().map(|&q| map.to_pixel(q)).collect()).collect();
            }
            if im.clip_mode != 0 {
                let outer: Vec<V2> = full.iter().map(|&p| map.to_xy(p)).collect();
                visible = Some(self.clip_xy(outer).iter().map(|r| r.iter().map(|&q| map.to_pixel(q)).collect()).collect());
            }
        }
        if im.flags & 4 != 0 || forced {
            // ezdxf `_mask_image`: clip mode 0 keeps the inside of the boundary, clip mode 1 removes it
            let inside = raster::polygons_mask(w, h, &keep_inside);
            let keep: Vec<u8> = match (&visible, im.clip_mode) {
                (_, 0) => inside,
                (Some(parts), _) => {
                    let vis = raster::polygons_mask(w, h, parts);
                    vis.iter().zip(&inside).map(|(&v, &i)| u8::from(v != 0 && i == 0)).collect()
                }
                (None, _) => inside.iter().map(|&i| u8::from(i == 0)).collect(),
            };
            for (p, k) in rgba.chunks_exact_mut(4).zip(&keep) {
                if *k == 0 {
                    p[3] = 0;
                }
            }
        }
        let corners = [map.to_xy(full[0]), map.to_xy(full[1]), map.to_xy(full[2]), map.to_xy(full[3])];
        let key = self.layer_key();
        let gid = self.rec.group(GKind::I, &key, [255, 255, 255, 255], 0.0);
        self.rec.image(gid, corners, PlacedImage { width: w, height: h, rgba: Arc::from(rgba) });
        true
    }

    /// Picture pixels (DXF pixel coordinates, top-left origin, pixel centres at integers) to the drawing coordinates
    /// of the entity after its block transform. Same placement as ezdxf's `get_wcs_transform` with its image flip.
    fn picture_map(&self, im: &ImageData, h: u32) -> PixelMap {
        let at = |p: V2| {
            let v3 = add(im.insert, add(mul(im.u, p[0] + 0.5), mul(im.v, f64::from(h) - p[1] - 0.5)));
            self.xy(v3)
        };
        let o = at([0.0, 0.0]);
        let ex = sub2(at([1.0, 0.0]), o);
        let ey = sub2(at([0.0, 1.0]), o);
        PixelMap { o, ex, ey }
    }

    /// A closed ring in drawing coordinates clipped by the clipping shapes of the viewports being drawn.
    fn clip_xy(&self, ring: Vec<V2>) -> Vec<Vec<V2>> {
        let mut parts = vec![ring];
        for c in self.clip.iter().rev() {
            parts = parts.iter().flat_map(|r| c.clip_polygon(r)).collect();
        }
        parts
    }

    // ------------------------------------------------------------------------------------------ LEADER / MLEADER / MLINE
    fn arrow_head(&mut self, tip: V3, toward: V3, size: f64, props: &Properties) {
        // closed filled arrow: tip, two points `size` back with the half angle of 18.924644 degrees
        let d = sub(tip, toward);
        let ang = d[1].atan2(d[0]);
        let h = (18.924644f64 / 2.0).to_radians().sin() * size;
        let local = [[-size, h], [0.0, 0.0], [-size, -h]];
        let (s, c) = ang.sin_cos();
        let pts: Vec<V3> = local.iter().map(|p| [tip[0] + p[0] * c - p[1] * s, tip[1] + p[0] * s + p[1] * c, tip[2]]).collect();
        self.out_polygon(&pts, props);
    }

    fn draw_leader(&mut self, e: &Entity, l: &LeaderData, props: &Properties) {
        let verts: Vec<V3> = l.vertices.clone();
        if verts.len() < 2 {
            return;
        }
        let measurement = self.doc.header.num("$MEASUREMENT").unwrap_or(0.0) != 0.0;
        // style attributes: DIMSTYLE of the leader (dimscale, dimasz, dimclrd)
        let style = self.doc.dimstyles.get(&l.dimstyle.to_lowercase());
        let mut dimscale = style.map(|s| s.scale).unwrap_or(1.0);
        if dimscale == 0.0 {
            dimscale = 1.0;
        }
        let asz = style.map(|s| s.arrow_size).unwrap_or(if measurement { 2.5 } else { 0.1875 });
        let mut p = props.clone();
        if let Some(st) = style {
            // the dimension line colour of the style replaces the colour of the entity; BYBLOCK means the block colour (7)
            let aci = if st.line_color == 0 { 7 } else { st.line_color };
            let mut c = e.c.clone();
            c.color = aci;
            c.true_color = None;
            p.color = self.ctx.resolve_color(&c, props.layer);
            p.role = self.ctx.color_role(&c, props.layer);
        }
        if l.spline_path && verts.len() > 2 {
            let pts = fit_points_curve(&verts, Some((sub(verts[1], verts[0]), sub(verts[verts.len() - 1], verts[verts.len() - 2]))), 16);
            self.out_polyline(&pts, &p);
        } else {
            self.out_polyline(&verts, &p);
        }
        if l.arrow_head {
            self.arrow_head(verts[0], verts[1], asz * dimscale, &p);
        }
    }

    fn draw_mleader(&mut self, e: &Entity, m: &MLeaderData, props: &Properties) {
        let arrow = if m.arrow_size > 0.0 { m.arrow_size } else { 4.0 };
        if m.leader_type != 0 {
            for leader in &m.leaders {
                for line in &leader.vertices {
                    let mut verts = line.clone();
                    // horizontal attachment: the line goes on to the landing point and along the dogleg
                    let dl = if leader.has_dogleg_vector { normalize_or_x(leader.dogleg) } else { X_AXIS };
                    if m.has_context {
                        if m.dogleg && m.leader_type != 2 {
                            verts.push(leader.last_point);
                        }
                        verts.push(add(leader.last_point, mul(dl, leader.dogleg_length)));
                    }
                    if verts.len() < 2 {
                        continue;
                    }
                    let seg = sub(verts[1], verts[0]);
                    let length = len(seg);
                    // an arrow head is only drawn when the first segment is longer than two arrow lengths
                    if length > arrow * 2.0 {
                        self.arrow_head(verts[0], verts[1], arrow, props);
                        let dir = mul(seg, 1.0 / length);
                        verts[0] = add(verts[0], mul(dir, arrow));
                    }
                    if m.leader_type == 2 && verts.len() > 2 {
                        let pts = fit_points_curve(&verts, Some((sub(verts[1], verts[0]), dl)), 16);
                        self.out_polyline(&pts, props);
                    } else {
                        self.out_polyline(&verts, props);
                    }
                }
            }
        }
        match m.content_type {
            2 if !m.text.is_empty() => {
                let width = m.text_width;
                let td = MTextData {
                    insert: m.text_location,
                    char_height: if m.text_height > 0.0 { m.text_height } else { 2.5 },
                    width,
                    attachment: if m.text_attachment >= 1 && m.text_attachment <= 9 { m.text_attachment } else { 1 },
                    flow_direction: 1,
                    text: m.text.clone(),
                    style: self.doc.standard_style,
                    text_direction: Some(m.text_direction),
                    rotation: 0.0,
                    line_spacing_factor: 1.0,
                    bg_fill: 0,
                    bg_color: None,
                    bg_true_color: None,
                    box_fill_scale: 1.5,
                    column_type: 0,
                    column_count: 0,
                    column_width: 0.0,
                    column_gutter: 0.0,
                    column_heights: Vec::new(),
                    auto_height: false,
                    defined_height: 0.0,
                };
                let ent = Entity { c: e.c.clone(), d: Data::MText(Box::new(td.clone())) };
                self.draw_mtext_entity(&ent, &td, props);
            }
            1 => {
                if let Some(b) = self.doc.block_by_record.get(&m.block_handle).filter(|_| m.block_handle != 0).and_then(|k| self.doc.blocks.get(k)) {
                    let name = b.name.clone();
                    let ins = InsertData {
                        name,
                        insert: m.block_location,
                        scale: m.block_scale,
                        rotation: m.block_rotation.to_degrees(),
                        col_count: 1,
                        row_count: 1,
                        col_spacing: 0.0,
                        row_spacing: 0.0,
                        attribs: Vec::new(),
                        table: false,
                        from_inventor: false,
                        clip: None,
                        xdict: 0,
                    };
                    let id = EntityId { container: self.cur_container, index: 0 };
                    let ent = Entity { c: e.c.clone(), d: Data::Insert(Box::new(ins)) };
                    if let Data::Insert(i2) = &ent.d {
                        self.draw_insert(&ent, i2, props, id);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_mline(&mut self, _e: &Entity, m: &MLineData, ocs: &Ocs, props: &Properties) {
        if m.vertices.len() < 2 {
            return;
        }
        let nel = m.vertices[0].elements.len();
        let mut miters: Vec<Vec<V3>> = Vec::new();
        for v in &m.vertices {
            if v.elements.len() != nel {
                return;
            }
            let pts: Vec<V3> = v.elements.iter().map(|el| add(v.location, mul(v.miter, el.first().copied().unwrap_or(0.0)))).collect();
            miters.push(pts);
        }
        if m.closed {
            let f = miters[0].clone();
            miters.push(f);
        }
        let _ = ocs;
        for k in 0..nel {
            let pts: Vec<V3> = miters.iter().map(|mv| mv[k]).collect();
            // each element line is a separate LINE per segment in ezdxf
            for w in pts.windows(2) {
                self.out_line(w[0], w[1], props);
            }
        }
    }
}

pub fn normalize_or_x(v: V3) -> V3 {
    if is_null(v) {
        X_AXIS
    } else {
        normalize(v)
    }
}

/// Transformation block -> parent of an INSERT (ezdxf `Insert.matrix44`).
pub fn insert_matrix(ocs: &Ocs, insert: V3, scale: V3, rotation_deg: f64, base_point: V3) -> Xform {
    let ux = mul(ocs.ux, scale[0]);
    let uy = mul(ocs.uy, scale[1]);
    let uz = mul(ocs.uz, scale[2]);
    let mut m = Xform::from_axes(ux, uy, uz, NULL3);
    if rotation_deg != 0.0 {
        m = m.then(&Xform::axis_rotate(ocs.uz, rotation_deg.to_radians()));
    }
    let ins = ocs.to_wcs(insert);
    let t = sub(ins, m.apply_dir(base_point));
    m.t = t;
    m
}

pub fn ellipse_path(center: V3, major: V3, extrusion: V3, ratio: f64, start: f64, end: f64) -> Option<Path> {
    if is_null(major) {
        return None;
    }
    let ext = normalize_or_z(extrusion);
    let minor = minor_axis(major, ext, ratio);
    let mut path = Path::default();
    add_ellipse(&mut path, center, major, minor, start, end, 1, true);
    Some(path)
}

/// Path of a SPLINE entity (ezdxf `add_spline` on the construction tool).
pub fn spline_path(s: &SplineData) -> Option<Path> {
    let mut path = Path::default();
    if !s.control_points.is_empty() {
        let bs = BSpline::new(s.control_points.clone(), s.degree.max(1) as usize, s.knots.clone(), s.weights.clone())?;
        path.start = bs.point(bs.domain().0);
        if bs.degree == 3 && !bs.is_rational() && bs.is_clamped() {
            if let Some(segs) = bs.bezier_decomposition() {
                let curves: Vec<[V3; 4]> = segs.into_iter().filter(|c| c.len() == 4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
                add_bezier4p(&mut path, curves);
                return Some(path);
            }
        }
        let pts = bs.sample(16);
        for p in pts.iter().skip(1) {
            path.line_to(*p);
        }
        return Some(path);
    }
    if s.fit_points.len() >= 2 {
        let tangents = match (s.start_tangent, s.end_tangent) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        path.start = s.fit_points[0];
        let curves = fit_points_beziers(&s.fit_points, tangents);
        if curves.is_empty() {
            for p in s.fit_points.iter().skip(1) {
                path.line_to(*p);
            }
        } else {
            add_bezier4p(&mut path, curves);
        }
        return Some(path);
    }
    None
}

/// Clipping boundary of a picture in its pixel coordinates: a rectangle of two corners or a polygon, without a repeated
/// last point. Without a boundary it is the image rectangle (ezdxf `reset_boundary_path`).
pub fn picture_boundary(im: &ImageData) -> Vec<V2> {
    let mut pts: Vec<V2> = if im.clip_vertices.len() < 2 {
        vec![[-0.5, -0.5], [im.size[0] - 0.5, im.size[1] - 0.5]]
    } else {
        im.clip_vertices.clone()
    };
    if pts.len() == 2 {
        let (p0, p1) = (pts[0], pts[1]);
        pts = vec![p0, [p1[0], p0[1]], p1, [p0[0], p1[1]]];
    }
    if pts.len() > 1 && v2_close(pts[0], pts[pts.len() - 1]) {
        pts.pop();
    }
    pts
}

/// Picture pixels to drawing coordinates: `o + p.x * ex + p.y * ey` (the block transform is affine).
struct PixelMap {
    o: V2,
    ex: V2,
    ey: V2,
}

impl PixelMap {
    fn det(&self) -> f64 {
        self.ex[0] * self.ey[1] - self.ey[0] * self.ex[1]
    }

    /// False for a picture seen edge on: its drawing coordinates cannot be mapped back to pixels.
    fn invertible(&self) -> bool {
        let d = self.det();
        d.is_finite() && d.abs() > 1e-300
    }

    fn to_xy(&self, p: V2) -> V2 {
        [self.o[0] + p[0] * self.ex[0] + p[1] * self.ey[0], self.o[1] + p[0] * self.ex[1] + p[1] * self.ey[1]]
    }

    fn to_pixel(&self, q: V2) -> V2 {
        let d = self.det();
        let (dx, dy) = (q[0] - self.o[0], q[1] - self.o[1]);
        [(dx * self.ey[1] - self.ey[0] * dy) / d, (self.ex[0] * dy - dx * self.ex[1]) / d]
    }
}

fn sub2(a: V2, b: V2) -> V2 {
    [a[0] - b[0], a[1] - b[1]]
}

/// Closed boundary of an IMAGE / WIPEOUT in WCS (last point repeats the first).
pub fn image_boundary(im: &ImageData) -> Vec<V3> {
    let u = im.u;
    let v = im.v;
    let origin = add(im.insert, sub(mul(u, 0.5), mul(v, 0.5)));
    let height = im.size[1];
    let mut px: Vec<V2> = im.clip_vertices.clone();
    if px.len() < 2 {
        px = vec![[-0.5, -0.5], [im.size[0] - 0.5, height - 0.5]];
    }
    if px.len() == 2 {
        let (p0, p1) = (px[0], px[1]);
        px = vec![p0, [p1[0], p0[1]], p1, [p0[0], p1[1]]];
    }
    if !v2_close(px[0], px[px.len() - 1]) {
        px.push(px[0]);
    }
    px.iter().map(|p| add(origin, add(mul(u, p[0]), mul(v, height - p[1])))).collect()
}

/// Faces of a polygon mesh / polyface mesh as vertex rings.
pub fn polyface_faces(p: &PolylineData) -> Vec<Vec<V3>> {
    let mut out = Vec::new();
    if p.is_polyface() {
        let locs: Vec<&Vertex> = p.vertices.iter().filter(|v| v.flags & 64 != 0 && v.flags & 128 != 0 || v.idx == [0; 4] && v.flags & 64 != 0).collect();
        let _ = locs;
        let points: Vec<V3> = p.vertices.iter().filter(|v| v.flags & 128 != 0 && v.flags & 64 != 0).map(|v| v.loc).collect();
        for v in p.vertices.iter().filter(|v| v.flags & 128 != 0 && v.flags & 64 == 0) {
            let mut face = Vec::new();
            for &i in &v.idx {
                if i == 0 {
                    break;
                }
                if let Some(pt) = points.get((i.unsigned_abs() as usize).wrapping_sub(1)) {
                    face.push(*pt);
                }
            }
            if face.len() > 1 {
                out.push(face);
            }
        }
    } else {
        let (m, n) = (p.m_count.max(0) as usize, p.n_count.max(0) as usize);
        let pts: Vec<V3> = p.vertices.iter().map(|v| v.loc).collect();
        if m * n <= pts.len() && m > 1 && n > 1 {
            let closed_m = p.flags & 1 != 0;
            let closed_n = p.flags & 32 != 0;
            let mm = if closed_m { m } else { m - 1 };
            let nn = if closed_n { n } else { n - 1 };
            for i in 0..mm {
                for j in 0..nn {
                    let (i1, j1) = ((i + 1) % m, (j + 1) % n);
                    out.push(vec![pts[i * n + j], pts[i * n + j1], pts[i1 * n + j1], pts[i1 * n + j]]);
                }
            }
        }
    }
    out
}

