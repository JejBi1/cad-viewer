//! State of the open drawing (DXF / DWG) and the 2D measuring tool; the layer list and the properties panel of the
//! 2D mode.
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cad_2d::{Drawing, EntityInfo, InfoRow};
use cad_core::fmt::fmt_num;
use cad_render::view2d::Rect2;
use cad_render::Overlays2d;
use cad_types::{LayerId, Scene2d};

use crate::app::App;
use crate::props::{PropRow, Props, Span};
use crate::scene2d::SceneSpace;

/// Layer toggles redraw after this pause (`_rerender.setInterval(150)`).
pub const RERENDER_DELAY: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, PartialEq)]
pub struct LayerRow {
    pub id: LayerId,
    pub name: String,
    pub color: [u8; 3],
    /// Entities of the layout on this layer.
    pub entities: u32,
    /// Switched off or frozen in the file itself.
    pub off_in_file: bool,
}

pub struct Draw2 {
    pub path: PathBuf,
    pub display_name: Option<String>,
    pub drawing: Arc<Drawing>,
    pub layout: String,
    pub space: SceneSpace,
    /// Sorted by lower-case name.
    pub layers: Vec<LayerRow>,
    pub selected_layers: BTreeSet<LayerId>,
    pub anchor: Option<LayerId>,
    pub picked: Option<EntityInfo>,
    pub from_cache: bool,
    /// Bounds of what is shown, in drawing coordinates.
    pub content: Option<Rect2>,
    /// Layout and text of the last "too complex" warning (shown once per layout).
    pub warned: Option<(String, String)>,
    pub rerender_at: Option<Instant>,
    /// Layout and white background option of the scene that is on the screen. A change that is being built is not on the
    /// screen yet: cancelling it goes back to these.
    pub shown_layout: String,
    pub shown_light: bool,
}

impl Draw2 {
    pub fn new(path: PathBuf, display_name: Option<String>, drawing: Arc<Drawing>, layout: String, from_cache: bool) -> Draw2 {
        Draw2 {
            path,
            display_name,
            drawing,
            shown_layout: layout.clone(),
            shown_light: false,
            layout,
            space: SceneSpace::default(),
            layers: Vec::new(),
            selected_layers: BTreeSet::new(),
            anchor: None,
            picked: None,
            from_cache,
            content: None,
            warned: None,
            rerender_at: None,
        }
    }

    /// The layer list of the layout (`build_layer_items`): name, colour, number of entities.
    pub fn rebuild_layers(&mut self, scene: &Scene2d) {
        let table = self.drawing.layer_table();
        let color_of = |name: &str| {
            table.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|(_, c, _)| *c).unwrap_or([200, 200, 200])
        };
        let mut rows: Vec<LayerRow> = scene
            .layers
            .iter()
            .map(|l| LayerRow {
                id: l.id,
                name: l.name.clone(),
                color: color_of(&l.name),
                entities: l.entities,
                off_in_file: l.off_in_file,
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        let keep: BTreeSet<LayerId> = rows.iter().map(|r| r.id).collect();
        self.selected_layers.retain(|l| keep.contains(l));
        self.layers = rows;
    }

    pub fn visible(&self, id: LayerId) -> bool {
        self.drawing.layer_visible(id)
    }

    /// Nothing is picked (the highlight of the renderer goes with the scene or with `App::set_entity(None)`).
    pub fn clear_pick(&mut self) {
        self.picked = None;
    }

    /// Layers shown in the list for a filter text (`apply_filter` in 2D: match on the name).
    pub fn filtered(&self, filter: &str) -> Vec<&LayerRow> {
        let t = filter.trim().to_lowercase();
        self.layers.iter().filter(|l| t.is_empty() || l.name.to_lowercase().contains(&t)).collect()
    }

    /// Info column text: the number of entities, empty for none.
    pub fn count_text(row: &LayerRow) -> String {
        if row.entities > 0 { row.entities.to_string() } else { String::new() }
    }
}

/// Value of a row of the drawing panel. Three rows are taken from here instead of from the drawing:
/// * `Složka` and `Velikost` are those of the file that was opened (`original`). The drawing knows the file it was read
///   from, which in portable mode is the local copy in the cache folder (`main.py` shows `self.path`);
/// * `Hladin / bloků` counts the layers of the list on the left, so the panel and the list agree (a layer that only
///   entities use, or the layer 0 that every drawing has, is in the list but not always in the layer table of the file).
pub fn info_value(label: &str, value: String, original: &std::path::Path, listed_layers: usize, blocks: usize) -> String {
    match label {
        "Složka" => original.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or(value),
        "Velikost" => std::fs::metadata(original).map(|m| crate::util::size_mb(m.len())).unwrap_or(value),
        "Hladin / bloků" if listed_layers > 0 => format!("{listed_layers} / {blocks}"),
        _ => value,
    }
}

/// The 2D measuring tool: points in drawing coordinates and what is drawn for them.
#[derive(Default)]
pub struct Measure2 {
    pub pts: Vec<[f64; 2]>,
    pub dots: Vec<[f64; 2]>,
    pub lines: Vec<([f64; 2], [f64; 2])>,
    pub texts: Vec<([f64; 2], String)>,
    pub snap: Option<[f64; 2]>,
    pub rubber: Option<([f64; 2], [f64; 2])>,
}

impl Measure2 {
    pub fn clear(&mut self) {
        *self = Measure2::default();
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.dots.is_empty() && self.lines.is_empty() && self.texts.is_empty() && self.snap.is_none() && self.rubber.is_none()
    }

    /// The overlays of the renderer, in drawing coordinates.
    pub fn overlays(&self) -> Overlays2d {
        let mut o = Overlays2d::default();
        for (a, b) in &self.lines {
            o.measure_line(*a, *b, false);
        }
        if let Some((a, b)) = &self.rubber {
            o.measure_line(*a, *b, true);
        }
        for d in &self.dots {
            o.measure_dot(*d);
        }
        for (p, t) in &self.texts {
            o.measure_text(*p, t);
        }
        if let Some(s) = &self.snap {
            o.snap_square(*s);
        }
        o
    }
}

/// Distance of two points and the texts of the status line (`on_measured2d`).
pub fn distance_status(a: [f64; 2], b: [f64; 2]) -> String {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d = dx.hypot(dy);
    let ang = dy.atan2(dx).to_degrees().rem_euclid(360.0);
    format!(
        "Vzdálenost: {}   (ΔX {}, ΔY {}, úhel {}°)   – další klik = nové měření, Esc = konec",
        cad_core::fmt::fmt_len(d),
        cad_core::fmt::fmt_len(dx.abs()),
        cad_core::fmt::fmt_len(dy.abs()),
        cad_core::fmt::fmt_len(ang)
    )
}

/// Angle at the vertex between the two arms, 0..180 degrees (`_measure_click` in 2D).
pub fn angle_at(a: [f64; 2], v: [f64; 2], c: [f64; 2]) -> f64 {
    let a1 = (a[1] - v[1]).atan2(a[0] - v[0]);
    let a2 = (c[1] - v[1]).atan2(c[0] - v[0]);
    let mut ang = (a2 - a1).to_degrees().abs() % 360.0;
    if ang > 180.0 {
        ang = 360.0 - ang;
    }
    ang
}

impl App {
    /// `show_drawing_info`: the panel of the drawing when nothing is picked.
    pub fn show_drawing_info(&mut self) {
        let Some(d) = &self.draw else { return };
        let bounds = d.content;
        let mut rows = Vec::new();
        let blocks = d.drawing.info().blocks;
        for r in d.drawing.info_rows(&d.layout, bounds, Some(self.load_secs)) {
            let r = InfoRow { value: info_value(&r.label, r.value, &d.path, d.layers.len(), blocks), ..r };
            let value = if r.warning {
                vec![Span::Warn(r.value)]
            } else if r.label == "Rozměr výkresu" {
                vec![Span::Bold(r.value)]
            } else {
                vec![Span::Text(r.value)]
            };
            rows.push(PropRow::spans(r.label, value));
        }
        self.props = Props::rows(Some(crate::model3::file_label(&d.path, &d.display_name)), rows);
    }

    /// `on_entity_picked`: the rows of the picked entity (and of its part inside a block).
    pub fn show_entity(&mut self) {
        let Some(d) = &self.draw else { return };
        let Some(e) = &d.picked else {
            self.show_drawing_info();
            return;
        };
        let mut rows: Vec<PropRow> = e.rows.iter().map(|(k, v)| PropRow::text(k.clone(), v.clone())).collect();
        if !e.block_rows.is_empty() {
            rows.push(PropRow::heading(cad_2d::IN_BLOCK_HEADING));
            rows.extend(e.block_rows.iter().map(|(k, v)| PropRow::text(k.clone(), v.clone())));
        }
        self.props = Props::rows(None, rows);
    }

    /// Panel of the selected layer (`on_tree_selection` in 2D).
    pub fn show_layer_props(&mut self) {
        let Some(d) = &self.draw else { return };
        match d.selected_layers.len() {
            1 => {
                let id = *d.selected_layers.iter().next().unwrap_or(&0);
                if let Some(l) = d.layers.iter().find(|l| l.id == id) {
                    let rows = d.drawing.layer_rows(&l.name).into_iter().map(|(k, v)| PropRow::text(k, v)).collect();
                    self.props = Props::rows(None, rows);
                }
            }
            0 => self.show_drawing_info(),
            _ => {}
        }
    }

    /// Selects a pick and draws its highlight. The highlight is made once, here, and kept on the graphics card by the
    /// renderer (`Renderer2d::set_highlight`): the frames that follow draw it from there and convert nothing. How much of
    /// a big entity is outlined is bounded by the drawing (`cad_2d::scene::HIGHLIGHT_MAX_POINTS`).
    pub fn set_entity(&mut self, e: Option<EntityInfo>) {
        let batches = match (&e, &self.draw) {
            (Some(info), Some(d)) => d.drawing.highlight(info.id),
            _ => Vec::new(),
        };
        if let Some(d) = self.draw.as_mut() {
            d.picked = e;
        }
        {
            let mut r = self.r2();
            if batches.is_empty() {
                r.clear_highlight();
            } else {
                r.set_highlight(&batches);
            }
        }
        drop(batches);
        self.warn_gpu_2d();
        self.refresh_overlays_2d();
        self.show_entity();
    }

    pub fn selected_layer_ids(&self) -> Vec<LayerId> {
        self.draw.as_ref().map(|d| d.selected_layers.iter().copied().collect()).unwrap_or_default()
    }

    /// `set_layers_visible`: `vis` for the given layers, `others` (when set) for every other one.
    pub fn set_layers_visible(&mut self, ids: &[LayerId], vis: bool, others: Option<bool>) {
        let Some(d) = &self.draw else { return };
        for l in &d.layers {
            let v = if ids.contains(&l.id) { Some(vis) } else { others };
            if let Some(v) = v {
                d.drawing.set_layer_visible(l.id, v);
            }
        }
        self.refresh_layers();
    }

    /// A check box of the layer list was clicked.
    pub fn toggle_layer(&mut self, id: LayerId, on: bool) {
        if let Some(d) = self.draw.as_mut() {
            d.drawing.set_layer_visible(id, on);
            d.rerender_at = Some(Instant::now() + RERENDER_DELAY);
            self.ctx.request_repaint_after(RERENDER_DELAY);
        }
    }

    /// `refresh_layers`: the new layer states are drawn, pan and zoom stay. Nothing is built or uploaded again: the batches
    /// that depend on a layer that is off are left out of the drawing (a mask), and pictures follow their layer.
    pub fn refresh_layers(&mut self) {
        let Some(d) = self.draw.as_mut() else { return };
        d.rerender_at = None;
        d.clear_pick();
        let mask = d.drawing.visible_batches();
        let layers: Vec<(LayerId, bool)> = d.layers.iter().map(|l| (l.id, d.drawing.layer_visible(l.id))).collect();
        let content = {
            let mut r = self.r2();
            r.set_batch_mask(&mask);
            for (id, shown) in layers {
                r.set_layer_visible(id, shown);
            }
            r.clear_highlight();
            r.visible_bounds()
        };
        if let Some(d) = self.draw.as_mut() {
            d.content = content;
        }
        self.meas2.clear();
        self.refresh_overlays_2d();
        self.update_status();
        self.show_drawing_info();
    }

    /// Debounced redraw after layer clicks.
    pub fn tick_layer_redraw(&mut self) {
        let Some(at) = self.draw.as_ref().and_then(|d| d.rerender_at) else { return };
        let now = Instant::now();
        if now >= at {
            self.refresh_layers();
        } else {
            // a frame before the time (mouse move) must not swallow the redraw: ask for another one at the time
            self.ctx.request_repaint_after(at - now);
        }
    }

    /// `on_layout_changed`.
    pub fn on_layout_changed(&mut self, name: String) {
        let Some(d) = self.draw.as_mut() else { return };
        if d.layout == name {
            return;
        }
        d.layout = name;
        self.render_drawing(false);
    }

    /// `on_light_bg`. A layout of paper space is white whatever the option says, so only the model space is made again.
    pub fn on_light_bg(&mut self, on: bool) {
        self.settings.light_bg = on;
        self.save_settings();
        let Some(d) = self.draw.as_ref() else { return };
        if !d.drawing.light_option_matters(&d.layout) {
            // the sheet is as it was: nothing is built, only the option that is remembered for the model space changes
            if let Some(d) = self.draw.as_mut() {
                d.shown_light = on;
            }
        } else if d.drawing.colors_follow_background() {
            // the pens that follow the background and the background itself are drawn in other colours, the scene stays as it is
            // (pan, zoom, the pick and the layers too)
            d.drawing.set_light_background(on);
            let (fg, bg) = (d.drawing.layout_foreground(&d.layout, on), d.drawing.layout_background(&d.layout, on));
            self.r2().set_palette(fg, bg);
            if let Some(d) = self.draw.as_mut() {
                d.shown_light = on;
            }
        } else {
            // a picture was blended with the background: the scene has to be made again
            self.render_drawing(true);
        }
    }

    pub fn on_rulers(&mut self, on: bool) {
        self.settings.rulers = on;
        self.save_settings();
    }

    pub fn zoom_selected_2d(&mut self) {
        let Some(d) = &self.draw else { return };
        let Some(e) = &d.picked else { return };
        if let Some(b) = d.drawing.entity_bounds(e.id) {
            self.r2().zoom_to_rect(&b);
        }
    }

    /// Cursor position in drawing coordinates from a pixel of the 2D view.
    pub fn world_at_2d(&self, px: [f64; 2]) -> [f64; 2] {
        let r = self.r2();
        let vp = r.viewport();
        r.view.to_world(px, vp)
    }

    /// `X 12,34   Y 56,78` of the status bar.
    pub fn coord_text(p: [f64; 2]) -> String {
        format!("X {}   Y {}", cad_core::fmt::fmt_len(p[0]), cad_core::fmt::fmt_len(p[1]))
    }

    /// Rebuilds the overlays of the 2D view (selection + measurement).
    pub fn refresh_overlays_2d(&mut self) {
        if self.draw.is_none() {
            return;
        }
        // the highlight of the picked entity is in the renderer, not among these (they are made again for each frame)
        self.r2().set_overlays(self.meas2.overlays());
    }
}

/// Text of a measured distance label in 2D (`_p(length, 2)`).
pub fn measure_label(len: f64) -> String {
    fmt_num(len, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_text_matches_the_python_status_line() {
        let s = distance_status([0.0, 0.0], [3.0, 4.0]);
        assert_eq!(s, "Vzdálenost: 5,00   (ΔX 3,00, ΔY 4,00, úhel 53,13°)   – další klik = nové měření, Esc = konec");
        // the angle is taken from the first to the second point and normalised to 0..360
        let s = distance_status([0.0, 0.0], [-1.0, -1.0]);
        assert!(s.contains("úhel 225,00°"), "{s}");
        assert!(distance_status([10.0, 5.0], [10.0, 5.0]).starts_with("Vzdálenost: 0,00"));
    }

    #[test]
    fn angle_between_arms() {
        assert!((angle_at([1.0, 0.0], [0.0, 0.0], [0.0, 1.0]) - 90.0).abs() < 1e-9);
        assert!((angle_at([1.0, 0.0], [0.0, 0.0], [-1.0, 0.0]) - 180.0).abs() < 1e-9);
        // never above 180
        assert!((angle_at([1.0, 0.0], [0.0, 0.0], [1.0, -1.0]) - 45.0).abs() < 1e-9);
        assert!((angle_at([0.0, 1.0], [0.0, 0.0], [1.0, -0.001]) - 90.05729577951308).abs() < 1e-6);
        assert!(angle_at([1.0, 0.0], [0.0, 0.0], [-1.0, -0.001]) <= 180.0);
    }

    #[test]
    fn the_drawing_panel_shows_the_opened_file_and_the_listed_layers() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("Výkres.dxf");
        std::fs::write(&original, vec![b'0'; 1024 * 1024 * 3 / 2]).unwrap();
        // portable mode: the drawing was read from a copy in the cache folder; the panel names the opened file
        let from = |label: &str| info_value(label, "z kopie".into(), &original, 5, 2);
        assert_eq!(from("Složka"), dir.path().to_string_lossy());
        assert_eq!(from("Velikost"), "1.50 MB", "the Python format: f\"{{size:.2f}} MB\" with a decimal point");
        // the layer row counts what the list shows: 5 layers, not the 4 of the table of the file
        assert_eq!(from("Hladin / bloků"), "5 / 2");
        // other rows are untouched
        assert_eq!(from("Verze"), "z kopie");
        // a file that cannot be read keeps the value of the drawing; an empty list (the scene failed) too
        let gone = dir.path().join("none.dxf");
        assert_eq!(info_value("Velikost", "0.01 MB".into(), &gone, 0, 0), "0.01 MB");
        assert_eq!(info_value("Hladin / bloků", "4 / 0".into(), &original, 0, 0), "4 / 0");
    }

    #[test]
    fn measure_overlays_are_in_drawing_coordinates() {
        let mut m = Measure2::default();
        m.dots.push([101.0, 202.0]);
        m.lines.push(([101.0, 202.0], [103.0, 202.0]));
        m.texts.push(([102.0, 202.0], "2,00".into()));
        m.snap = Some([103.0, 202.0]);
        m.rubber = Some(([103.0, 202.0], [104.0, 203.0]));
        let o = m.overlays();
        assert_eq!(o.dots[0].pos, [101.0, 202.0]);
        assert_eq!(o.lines.len(), 2);
        assert!(o.lines.iter().any(|l| l.dashed));
        assert_eq!(o.texts[0].text, "2,00");
        assert_eq!(o.squares[0].pos, [103.0, 202.0]);
        assert!(!m.is_empty());
        m.clear();
        assert!(m.is_empty());
    }
}
