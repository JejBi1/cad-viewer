//! The 3D measuring tool (`Měřit` and `Úhel`): what the cursor can pick (B-rep vertices, edges and faces, exact points
//! on faces, points and outlines of STL meshes), the overlays of the picks, the measurement panel, Enter / copy.
use cad_core::fmt::fmt_len;
use cad_core::meshmeasure::{Loop, Mesh};
use cad_render::colors::{
    HOVER_COLOR, MEASURE_LINE_COLOR, PICK_COLOR, SNAPPED_MARKER_COLOR,
};
use cad_render::overlay::{angle_between, angle_label};
use cad_render::{Highlight, OverlayMesh, Overlays, PickFilter};
use cad_types::{EntityKind, EntityRef, NodeId, P3};
use occt_bridge::{LoopCircle, MeasureItem, MeshLoop, MeshPatch, Measurement};

use crate::app::{App, MeasureKind, Mode};
use crate::props::{PropRow, Props, Span};
use crate::strings;

/// One picked thing of the measure tool (`PickItem`).
#[derive(Clone, Debug, PartialEq)]
pub enum Picked {
    /// A vertex, edge or face of a placed instance.
    Brep(EntityRef),
    /// A sharp-edge outline of an STL mesh (index into the mesh's loops).
    Loop { node: NodeId, index: usize },
    /// A point on a face with an optional normal; mesh points may snap to a mesh vertex and carry a flat patch.
    Point { point: P3, normal: Option<P3>, mesh: bool, snapped: bool, patch: Option<usize> },
}

impl Picked {
    /// `PickItem.same`.
    pub fn same(&self, other: &Picked) -> bool {
        match (self, other) {
            (Picked::Brep(a), Picked::Brep(b)) => a == b,
            (Picked::Loop { node: n1, index: i1 }, Picked::Loop { node: n2, index: i2 }) => n1 == n2 && i1 == i2,
            (Picked::Point { point: a, .. }, Picked::Point { point: b, .. }) => dist(*a, *b) < 1e-9,
            _ => false,
        }
    }
}

fn dist(a: P3, b: P3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// What the hover shows, to avoid recomputing for the same thing.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverKey(pub Option<Picked>, pub bool);

#[derive(Default)]
pub struct Measure3 {
    pub picked: Vec<Picked>,
    /// Rows of the last measurement for Ctrl+C (label, value).
    pub rows: Option<Vec<(String, String)>>,
    /// Kept measurements (Enter).
    pub pinned: Overlays,
    /// Markers, highlights, label and relation of the current picks.
    pub current: Overlays,
    /// Hover highlight of a flat mesh face.
    pub hover: Overlays,
    pub hover_key: Option<HoverKey>,
    /// Angle tool: points of the measurement in progress and everything drawn so far.
    pub angle_pts: Vec<P3>,
    pub angle: Overlays,
}

fn mesh_loop_of(lp: &Loop) -> MeshLoop {
    MeshLoop {
        points: lp.pts.clone(),
        closed: lp.closed,
        length: lp.length,
        kind: lp.kind().to_string(),
        circle: lp.circle.map(|c| LoopCircle { center: c.centre, normal: c.normal, radius: c.radius, rms: c.rms }),
        coverage: lp.coverage,
    }
}

/// `OverlayMesh` of the facets of a flat mesh patch.
fn patch_mesh(mesh: &Mesh, facets: &[u32], color: [u8; 3], alpha: f32) -> OverlayMesh {
    let mut positions = Vec::with_capacity(facets.len() * 3);
    let mut indices = Vec::with_capacity(facets.len() * 3);
    for &f in facets {
        for &n in &mesh.tris[f as usize] {
            indices.push(positions.len() as u32);
            positions.push(mesh.nodes[n as usize]);
        }
    }
    OverlayMesh { positions, indices, color, alpha }
}

impl App {
    fn stl_mesh_mut(&mut self) -> Option<&mut Mesh> {
        self.model.as_mut()?.stl.as_mut().map(|s| &mut s.mesh)
    }

    /// The item for the measure tool and the engine: `MeasureItem` of a pick.
    pub fn measure_item(&mut self, p: &Picked) -> Option<MeasureItem> {
        Some(match p {
            Picked::Brep(r) => MeasureItem::Entity(*r),
            Picked::Loop { index, .. } => {
                let mesh = self.stl_mesh_mut()?;
                let lp = mesh.loops().get(*index)?.clone();
                MeasureItem::Loop(mesh_loop_of(&lp))
            }
            Picked::Point { point, normal, mesh, snapped, patch } => {
                let patch = match (patch, self.stl_mesh_mut()) {
                    (Some(id), Some(m)) => m.patch(*id).map(|pt| MeshPatch {
                        id: *id as u64,
                        facets: pt.facets.len(),
                        area: pt.area,
                        centroid: pt.centroid,
                    }),
                    _ => None,
                };
                MeasureItem::Point { point: *point, normal: *normal, mesh: *mesh, snapped: *snapped, patch }
            }
        })
    }

    /// `set_measure`: the two toolbar toggles are mutually exclusive.
    pub fn set_measure(&mut self, kind: Option<MeasureKind>) {
        if self.measure == kind {
            return;
        }
        self.measure = kind;
        if self.mode == Mode::TwoD {
            self.meas2.clear();
            self.refresh_overlays_2d();
        } else {
            // everything of the previous measurement goes (`clear_measure`)
            self.meas = Default::default();
            self.r3().set_measure_mode(kind.is_some());
            self.r3().set_picked(&[]);
            self.r3().set_hover(None);
            if kind.is_some() {
                // measuring starts from a clean view (`ctx.ClearSelected`)
                self.r3().set_selection(&[]);
            }
            self.refresh_overlays();
            if kind == Some(MeasureKind::Dist) {
                self.show_measure_help();
            }
        }
        match (kind, self.mode == Mode::TwoD) {
            (Some(MeasureKind::Dist), false) => self.status.model = strings::MEASURE_HINT_3D.to_string(),
            (Some(MeasureKind::Dist), true) => self.status.model = strings::MEASURE_HINT_2D.to_string(),
            (Some(MeasureKind::Angle), _) => self.status.model = strings::ANGLE_HINT.to_string(),
            (None, _) => {
                self.status.coord.clear();
                self.update_status();
            }
        }
    }

    /// The panel text while nothing is picked.
    pub fn show_measure_help(&mut self) {
        let is_mesh = self.model.as_ref().is_some_and(|m| m.stl.is_some());
        let mut rows = vec![PropRow::heading("Měření")];
        rows.extend(strings::MEASURE_HELP.iter().map(|t| PropRow::note(*t)));
        if is_mesh {
            rows.push(PropRow::text("STL", strings::MEASURE_HELP_STL));
        }
        self.props = Props::rows(None, rows);
    }

    /// Everything the 3D view draws on top of the model, in one set.
    pub fn refresh_overlays(&mut self) {
        let mut o = Overlays::default();
        o.extend(self.overlays.cog.clone());
        o.extend(self.overlays.collision.clone());
        o.extend(self.meas.pinned.clone());
        o.extend(self.meas.angle.clone());
        o.extend(self.meas.current.clone());
        o.extend(self.meas.hover.clone());
        self.r3().set_overlays(o);
    }

    // ------------------------------------------------------------------ picking

    /// What the cursor is on as a pick (`_detected_item`): `x`, `y` in physical pixels of the view.
    pub fn detected_item(&mut self, x: f32, y: f32, shift: bool) -> Option<Picked> {
        let radius = cad_render::interaction::pick_tolerance(true, self.r3().dpr());
        let is_stl_part = |app: &App, node: NodeId| app.model.as_ref().is_some_and(|m| m.is_stl_leaf(node));
        let mut hit = self.r3().pick_with(x, y, radius, PickFilter::ALL).ok().flatten()?;
        if is_stl_part(self, hit.entity.node) {
            if shift && hit.entity.kind == EntityKind::Edge {
                // Shift = only a point, no snapping to the sharp edges
                hit = self.r3().pick_with(x, y, radius, PickFilter::FACES).ok().flatten()?;
            }
            return self.mesh_item(hit.entity, hit.point, shift);
        }
        if shift {
            let mut normal = None;
            if hit.entity.kind == EntityKind::Face {
                if let Some(doc) = self.model.as_ref().and_then(|m| m.doc.as_ref()) {
                    normal = doc.face_normal_at(hit.entity, hit.point);
                }
            }
            return Some(Picked::Point { point: hit.point, normal, mesh: false, snapped: false, patch: None });
        }
        Some(Picked::Brep(hit.entity))
    }

    /// A pick on an STL mesh: an outline, or a point snapped to a mesh vertex with its flat face.
    fn mesh_item(&mut self, e: EntityRef, point: P3, shift: bool) -> Option<Picked> {
        if e.kind == EntityKind::Edge {
            return Some(Picked::Loop { node: e.node, index: e.index as usize });
        }
        let wpp = self.r3().world_per_pixel();
        let tol = cad_render::interaction::MESH_SNAP_PX * wpp;
        let mesh = self.stl_mesh_mut()?;
        if !shift {
            // OCCT prefers the mesh face to the feature line drawn on it
            if let Some(i) = loop_index_at(mesh, point, tol) {
                return Some(Picked::Loop { node: e.node, index: i });
            }
        }
        let t = mesh.nearest_triangle(point);
        let n = mesh.normals[t];
        let normal = (n.iter().any(|c| *c != 0.0)).then(|| [f64::from(n[0]), f64::from(n[1]), f64::from(n[2])]);
        let near = mesh.nearest_vertex(point, Some(tol));
        let (snapped, p) = match near {
            Some((v, d)) if d <= tol => (true, v),
            _ => (false, point),
        };
        let pid = mesh.flat_patch(t);
        let patch = mesh.patch(pid).filter(|pt| pt.is_face).map(|_| pid);
        Some(Picked::Point { point: p, normal, mesh: true, snapped, patch })
    }

    /// Click in measure mode: `pick_entity(add=True)`.
    pub fn measure_pick(&mut self, x: f32, y: f32, shift: bool) {
        let it = self.detected_item(x, y, shift);
        match it {
            None => self.meas.picked.clear(),
            Some(it) => {
                if self.meas.picked.iter().any(|p| p.same(&it)) {
                    self.meas.picked.retain(|p| !p.same(&it));
                } else {
                    self.meas.picked.push(it);
                }
            }
        }
        self.picks_changed();
    }

    /// Mouse moved in measure mode: highlight and one-line description of what is under the cursor.
    pub fn measure_hover(&mut self, x: f32, y: f32, shift: bool) {
        let it = self.detected_item(x, y, shift);
        let key = HoverKey(it.clone(), shift);
        if self.meas.hover_key.as_ref() == Some(&key) {
            return;
        }
        self.meas.hover_key = Some(key);
        let mut hover = Overlays::default();
        let mut highlight = None;
        let mut text = String::new();
        if let Some(it) = &it {
            match it {
                Picked::Brep(r) => highlight = Some(Highlight::Entity(*r)),
                Picked::Loop { node, index } => {
                    highlight = Some(Highlight::Entity(EntityRef { node: *node, kind: EntityKind::Edge, index: *index as u32 }));
                }
                Picked::Point { patch: Some(pid), .. } => {
                    if let Some(m) = self.stl_mesh_mut() {
                        if let Some(pt) = m.patch(*pid) {
                            let facets = pt.facets.clone();
                            // OCCT transparency 0.55 is an opacity of 0.45
                            hover.meshes.push(patch_mesh(m, &facets, HOVER_COLOR, 0.45));
                        }
                    }
                }
                Picked::Point { .. } => {}
            }
            if let Some(item) = self.measure_item(it) {
                text = self
                    .model
                    .as_ref()
                    .and_then(|m| m.doc.as_ref())
                    .and_then(|d| d.entity(&item).ok())
                    .map(|e| e.short())
                    .unwrap_or_default();
            }
        }
        self.r3().set_hover(highlight);
        if self.measure == Some(MeasureKind::Dist) {
            self.status.coord = text;
        }
        self.meas.hover = hover;
        self.refresh_overlays();
    }

    // ------------------------------------------------------------------ results

    /// `on_entities`: properties of 1 entity, relation of 2, totals of 3 or more.
    pub fn picks_changed(&mut self) {
        let picks = self.meas.picked.clone();
        let items: Vec<MeasureItem> = picks.iter().filter_map(|p| self.measure_item(p)).collect();
        let brep: Vec<EntityRef> = picks.iter().filter_map(|p| if let Picked::Brep(r) = p { Some(*r) } else { None }).collect();
        self.r3().set_picked(&brep);
        let mut cur = Overlays::default();
        // markers of points, highlights of outlines and patches (B-rep entities are highlighted by the renderer)
        for p in &picks {
            match p {
                Picked::Point { point, snapped, patch, .. } => {
                    cur.marker(*point, if *snapped { SNAPPED_MARKER_COLOR } else { PICK_COLOR });
                    if let (Some(pid), Some(m)) = (patch, self.stl_mesh_mut()) {
                        if let Some(pt) = m.patch(*pid) {
                            let facets = pt.facets.clone();
                            // OCCT transparency 0.45 is an opacity of 0.55
                            cur.meshes.push(patch_mesh(m, &facets, PICK_COLOR, 0.55));
                        }
                    }
                }
                Picked::Loop { index, .. } => {
                    if let Some(lp) = self.stl_mesh_mut().and_then(|m| m.loops().get(*index).cloned()) {
                        cur.lines.push(cad_render::OverlayLine {
                            points: lp.pts.clone(),
                            closed: lp.closed,
                            color: PICK_COLOR,
                            width: 4.0,
                            dashed: false,
                        });
                    }
                }
                Picked::Brep(_) => {}
            }
        }
        if items.is_empty() {
            self.meas.rows = None;
            self.meas.current = cur;
            self.refresh_overlays();
            self.show_measure_help();
            return;
        }
        let m: Measurement = match self.model.as_ref().and_then(|m| m.doc.as_ref()) {
            Some(doc) => doc.measure_items(&items),
            None => {
                // the interference check owns the document: the value is said to be computed, and the panel is built
                // again when the document is back (`refresh_numbers` calls this function)
                if let Some(model) = self.model.as_mut() {
                    model.unmeasured = true;
                }
                self.props = Props::rows(None, vec![PropRow::heading("Měření"), PropRow::text("Hodnota", strings::COMPUTING_AFTER_CHECK)]);
                self.meas.rows = None;
                self.meas.current = cur;
                self.refresh_overlays();
                return;
            }
        };
        if let Some(err) = &m.error {
            self.props = Props::rows(None, vec![PropRow::text("Měření", format!("nelze vyhodnotit: {err}"))]);
            self.meas.rows = None;
            self.meas.current = cur;
            self.refresh_overlays();
            return;
        }
        match m.entities.len() {
            1 => {
                if let Some((p, txt)) = &m.label {
                    cur.measure_label(*p, txt);
                }
            }
            2 => cur.relation(m.min_points, m.center_points, m.distance_label.as_deref()),
            _ => {}
        }
        if let Some(st) = &m.status {
            self.status.model = st.clone();
        }
        self.meas.rows = Some(m.rows.iter().map(|r| (r.label.clone(), r.value.clone())).collect());
        let mut rows = vec![PropRow::heading("Měření")];
        for r in &m.rows {
            rows.push(if r.bold { PropRow::bold(r.label.clone(), r.value.clone()) } else { PropRow::text(r.label.clone(), r.value.clone()) });
        }
        rows.push(PropRow::spans("", vec![Span::Muted(strings::MEASURE_FOOTER.to_string())]));
        self.props = Props::rows(None, rows);
        self.meas.current = cur;
        self.refresh_overlays();
    }

    /// Enter: keep the measurement on screen and start a new one (`pin_measure`).
    pub fn pin_measure(&mut self) {
        if self.measure != Some(MeasureKind::Dist) || (self.meas.picked.is_empty() && self.meas.current.is_empty()) {
            return;
        }
        let mut pin = std::mem::take(&mut self.meas.current);
        for p in self.meas.picked.clone() {
            if let Picked::Brep(r) = p {
                pin.extend(self.brep_overlay(r));
            }
        }
        self.meas.pinned.extend(pin);
        self.meas.picked.clear();
        self.r3().set_picked(&[]);
        self.meas.rows = None;
        self.refresh_overlays();
        self.show_measure_help();
    }

    /// "Vyčistit výběr".
    pub fn clear_picked(&mut self) {
        self.meas.picked.clear();
        self.picks_changed();
    }

    /// A picked B-rep entity as an overlay (the pinned form of the highlight the renderer draws for live picks).
    fn brep_overlay(&self, r: EntityRef) -> Overlays {
        let mut o = Overlays::default();
        let Some(m) = &self.model else { return o };
        let Some(doc) = &m.doc else { return o };
        let node = m.node(r.node);
        let Some(proto) = node.proto else { return o };
        let mesh = doc.proto_mesh(proto);
        let w = |p: &[f32; 3]| crate::mass::transform_point(&node.location, [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]);
        match r.kind {
            EntityKind::Face => {
                if let Some(f) = mesh.faces.get(r.index as usize) {
                    let mut positions = Vec::new();
                    let mut indices = Vec::new();
                    let (a, b) = (f.first_index as usize, (f.first_index + f.index_count) as usize);
                    for &i in &mesh.indices[a.min(mesh.indices.len())..b.min(mesh.indices.len())] {
                        indices.push(positions.len() as u32);
                        positions.push(w(&mesh.positions[i as usize]));
                    }
                    // OCCT transparency 0.35 is an opacity of 0.65
                    o.meshes.push(OverlayMesh { positions, indices, color: PICK_COLOR, alpha: 0.65 });
                }
            }
            EntityKind::Edge => {
                if let Some(s) = mesh.edges.get(r.index as usize) {
                    let pts: Vec<P3> = mesh.edge_points[s.first_point as usize..(s.first_point + s.point_count) as usize].iter().map(w).collect();
                    if pts.len() >= 2 {
                        o.lines.push(cad_render::OverlayLine { points: pts, closed: false, color: PICK_COLOR, width: 4.0, dashed: false });
                    }
                }
            }
            EntityKind::Vertex => {
                if let Some(v) = mesh.vertices.get(r.index as usize) {
                    o.points.push(cad_render::OverlayPoint::ball(w(v), PICK_COLOR, 3.0));
                }
            }
        }
        o
    }

    /// Ctrl+C: the result of the measurement to the clipboard (`copy_measure`).
    pub fn copy_measure(&mut self) {
        if self.measure.is_none() {
            return;
        }
        let Some(rows) = &self.meas.rows else { return };
        let text = clipboard_text(rows);
        self.ctx.copy_text(text);
        self.status_msg(strings::COPIED, 3000);
    }

    // ------------------------------------------------------------------ angle tool

    /// A vertex was clicked with the angle tool.
    pub fn angle_click(&mut self, p: P3) {
        self.meas.angle_pts.push(p);
        self.meas.angle.marker(p, MEASURE_LINE_COLOR);
        if self.meas.angle_pts.len() == 3 {
            let (a, v, c) = (self.meas.angle_pts[0], self.meas.angle_pts[1], self.meas.angle_pts[2]);
            self.meas.angle.line(v, a, MEASURE_LINE_COLOR, 2.5, false);
            self.meas.angle.line(v, c, MEASURE_LINE_COLOR, 2.5, false);
            if let Some(deg) = angle_between(a, v, c) {
                self.meas.angle.measure_label(v, &angle_label(deg));
                self.status.model = format!("Úhel: {}°   – další 3 kliky = nové měření, Esc = konec", fmt_len(deg));
            }
            self.meas.angle_pts.clear();
        }
        self.refresh_overlays();
    }

    /// Click with the angle tool: only vertices (and mesh points) are taken.
    pub fn angle_pick(&mut self, x: f32, y: f32) {
        let radius = cad_render::interaction::pick_tolerance(true, self.r3().dpr());
        let filter = PickFilter { faces: false, edges: false, vertices: true };
        let hit = self.r3().pick_with(x, y, radius, filter).ok().flatten();
        let hit = match hit {
            Some(h) => Some(h),
            // meshes have no vertices of their own: take the surface point and snap it to a mesh vertex
            None if self.model.as_ref().is_some_and(|m| m.stl.is_some()) => {
                let face = self.r3().pick_with(x, y, radius, PickFilter::FACES).ok().flatten();
                let wpp = self.r3().world_per_pixel();
                face.map(|mut h| {
                    let tol = cad_render::interaction::MESH_SNAP_PX * wpp;
                    if let Some(m) = self.stl_mesh_mut() {
                        if let Some((v, d)) = m.nearest_vertex(h.point, Some(tol)) {
                            if d <= tol {
                                h.point = v;
                            }
                        }
                    }
                    h
                })
            }
            None => None,
        };
        if let Some(h) = hit {
            self.angle_click(h.point);
        }
    }
}

/// Index of the loop passing within `tol` of a point.
fn loop_index_at(mesh: &mut Mesh, p: P3, tol: f64) -> Option<usize> {
    let hit = mesh.loop_at(p, tol)?.clone();
    mesh.loops().iter().position(|l| *l == hit)
}

/// The rows as the clipboard text: label, a tab, value; empty rows are skipped.
pub fn clipboard_text(rows: &[(String, String)]) -> String {
    rows.iter()
        .filter(|(k, v)| !k.is_empty() || !v.is_empty())
        .map(|(k, v)| format!("{k}\t{v}").trim().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_picks() {
        let e = EntityRef { node: 3, kind: EntityKind::Face, index: 1 };
        assert!(Picked::Brep(e).same(&Picked::Brep(e)));
        assert!(!Picked::Brep(e).same(&Picked::Brep(EntityRef { index: 2, ..e })));
        let p = |x: f64| Picked::Point { point: [x, 0.0, 0.0], normal: None, mesh: false, snapped: false, patch: None };
        assert!(p(1.0).same(&p(1.0 + 1e-10)));
        assert!(!p(1.0).same(&p(1.001)));
        assert!(Picked::Loop { node: 1, index: 4 }.same(&Picked::Loop { node: 1, index: 4 }));
        assert!(!Picked::Loop { node: 1, index: 4 }.same(&Picked::Loop { node: 1, index: 5 }));
        assert!(!Picked::Brep(e).same(&p(1.0)));
    }

    #[test]
    fn clipboard_text_skips_empty_rows_and_trims() {
        let rows = vec![
            ("Entita 1".to_string(), "hrana – úsečka".to_string()),
            (String::new(), String::new()),
            ("Poznámka".to_string(), String::new()),
            (String::new(), "jen hodnota".to_string()),
        ];
        assert_eq!(clipboard_text(&rows), "Entita 1\thrana – úsečka\nPoznámka\njen hodnota");
    }

    #[test]
    fn mesh_loops_convert_with_their_circle() {
        let pts: Vec<P3> = (0..24)
            .map(|i| {
                let a = f64::from(i) / 24.0 * std::f64::consts::TAU;
                [5.0 * a.cos(), 5.0 * a.sin(), 1.0]
            })
            .collect();
        let lp = Loop::new(pts, true);
        let ml = mesh_loop_of(&lp);
        assert_eq!(ml.kind, "kružnice");
        assert!(ml.closed);
        let c = ml.circle.unwrap();
        assert!((c.radius - 5.0).abs() < 1e-6);
        assert!((ml.coverage - lp.coverage).abs() < 1e-12);
    }

    #[test]
    fn patch_overlay_mesh_uses_the_facets() {
        let mesh = Mesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            vec![[0, 1, 2], [1, 3, 2]],
        );
        let m = patch_mesh(&mesh, &[1], [1, 2, 3], 0.5);
        assert_eq!(m.positions.len(), 3);
        assert_eq!(m.positions[0], [1.0, 0.0, 0.0]);
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.alpha, 0.5);
    }
}
