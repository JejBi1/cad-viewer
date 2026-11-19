//! The commands of the window: what a toolbar button, a shortcut or a menu entry does (`MainWindow` methods).
use cad_core::fmt::{fmt_len, fmt_num};
use cad_core::materials::Assignments;
use cad_render::{CaptureOptions, Overlays, Section};
use cad_types::NodeId;
use eframe::egui;

use crate::app::{Action, App, MeasureKind, Mode};
use crate::dialogs::{MaterialDlg, SpaceMouseDlg, ThumbDlg};
use crate::drive::LinkAction;
use crate::mass;
use crate::panels::{LayerAct, TreeAct};
use crate::props::{LinkTarget, PropRow, Props, Span};
use crate::strings;
use crate::util::is_supported;

impl App {
    /// Runs everything queued by the interface during the frame.
    pub fn run_queue(&mut self, frame: &eframe::Frame) {
        let actions: Vec<Action> = std::mem::take(&mut self.queue);
        for a in actions {
            self.run_action(a, frame);
        }
    }

    pub fn run_action(&mut self, action: Action, frame: &eframe::Frame) {
        match action {
            Action::Open => self.open_dialog(frame),
            Action::OpenPath(p) => self.open_file(p, None),
            Action::Reload => {
                let target = match self.mode {
                    Mode::ThreeD => self.model.as_ref().map(|m| (m.path.clone(), m.display_name.clone())),
                    Mode::TwoD => self.draw.as_ref().map(|d| (d.path.clone(), d.display_name.clone())),
                    Mode::Empty => None,
                };
                if let Some((p, name)) = target {
                    self.open_file(p, name);
                }
            }
            Action::Fit => self.fit(),
            Action::ZoomSel => self.zoom_selected(),
            Action::View(v) => {
                if self.mode == Mode::ThreeD {
                    self.r3().set_orientation(v);
                }
            }
            Action::Edges => {
                self.opts.edges = !self.opts.edges;
                self.apply_style3();
            }
            Action::Wire => {
                self.opts.wire = !self.opts.wire;
                self.apply_style3();
            }
            Action::Persp => {
                self.opts.persp = !self.opts.persp;
                let on = self.opts.persp;
                self.r3().set_perspective(on);
            }
            Action::Measure => {
                let next = if self.measure == Some(MeasureKind::Dist) { None } else { Some(MeasureKind::Dist) };
                self.set_measure(next);
            }
            Action::Angle => {
                let next = if self.measure == Some(MeasureKind::Angle) { None } else { Some(MeasureKind::Angle) };
                self.set_measure(next);
            }
            Action::CopyMeasure => self.copy_measure(),
            Action::Material => self.assign_material(),
            Action::Mass => self.show_mass(),
            Action::Collide => self.start_collisions(),
            Action::Shot => self.screenshot(frame),
            Action::Hide => self.hide_selected(),
            Action::Isolate => self.isolate_selected(),
            Action::ShowAll => self.show_all(),
            Action::Transp => self.toggle_transparency(),
            Action::Escape => self.clear_selection(),
            Action::Expand => {
                if let Some(m) = self.model.as_mut() {
                    m.expand_all();
                }
            }
            Action::Collapse => {
                if let Some(m) = self.model.as_mut() {
                    m.collapse_all();
                }
            }
            Action::About => self.dlg.about = true,
            Action::PinMeasure => self.pin_measure(),
            Action::ClearPicked => self.clear_picked(),
            Action::EndMeasure => self.set_measure(None),
            Action::CopyNames => {
                if let Some(m) = &self.model {
                    let text = m.names_of(&m.selection());
                    self.ctx.copy_text(text);
                }
            }
            Action::ClearCache => {
                if !self.flow.active() {
                    cad_core::cache::clear(&self.data_dir);
                }
            }
            Action::ThumbSettings => self.dlg.thumbs = Some(ThumbDlg::new()),
            Action::SpaceMouseSettings => self.dlg.spacemouse = Some(SpaceMouseDlg::new(&self.settings.spacemouse)),
            Action::RegisterLink => self.link_action(LinkAction::Register),
            Action::UnregisterLink => self.link_action(LinkAction::Unregister),
            Action::DriveSignOut => self.link_action(LinkAction::SignOut),
        }
    }

    // ------------------------------------------------------------------ opening and saving

    /// `open_dialog`.
    pub fn open_dialog(&mut self, frame: &eframe::Frame) {
        self.prestart_worker(); // imports OCCT while the user is still picking a file
        let mut dlg = rfd::FileDialog::new()
            .set_title(strings::OPEN_TITLE)
            .add_filter(strings::ALL_FILTER, &["step", "stp", "iges", "igs", "stl", "dxf", "dwg"])
            .add_filter(strings::MODEL_FILTER, &["step", "stp", "iges", "igs", "stl"])
            .add_filter(strings::DRAWING_FILTER, &["dxf", "dwg"])
            .add_filter(strings::EVERY_FILTER, &["*"])
            .set_parent(frame);
        let last = std::path::PathBuf::from(&self.settings.lastdir);
        if !self.settings.lastdir.is_empty() && last.is_dir() {
            dlg = dlg.set_directory(last);
        }
        if let Some(path) = dlg.pick_file() {
            self.open_file(path, None);
        }
    }

    /// `screenshot`: the view as a PNG.
    pub fn screenshot(&mut self, frame: &eframe::Frame) {
        let (name, ok_mode) = match self.mode {
            Mode::ThreeD => (self.model.as_ref().map(|m| m.path.clone()), true),
            Mode::TwoD => (self.draw.as_ref().map(|d| d.path.clone()), true),
            Mode::Empty => (None, false),
        };
        let base = name
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pohled".to_string());
        let mut dlg = rfd::FileDialog::new()
            .set_title(strings::SAVE_TITLE)
            .set_file_name(format!("{base}.png"))
            .add_filter("PNG", &["png"])
            .set_parent(frame);
        let last = std::path::PathBuf::from(&self.settings.lastdir);
        if !self.settings.lastdir.is_empty() && last.is_dir() {
            dlg = dlg.set_directory(last);
        }
        let Some(path) = dlg.save_file() else { return };
        if !ok_mode {
            return;
        }
        let path = if path.extension().is_none() { path.with_extension("png") } else { path };
        let (w, h, dpr) = self.view_px;
        let png = match self.mode {
            Mode::ThreeD => {
                let mut o = CaptureOptions::screenshot(w.max(1), h.max(1));
                o.dpr = dpr;
                self.r3().capture_png(&o)
            }
            _ => self.r2().capture_png(w.max(1), h.max(1), false),
        };
        let saved = png.ok().and_then(|bytes| std::fs::write(&path, bytes).ok()).is_some();
        if saved {
            self.status_msg(format!("Uloženo: {}", path.display()), 4000);
        } else {
            self.status_msg("Snímek se nepodařilo uložit", 4000);
        }
    }

    // ------------------------------------------------------------------ view

    pub fn fit(&mut self) {
        match self.mode {
            Mode::TwoD => self.r2().fit(),
            Mode::ThreeD => self.r3().fit_all(),
            Mode::Empty => {}
        }
    }

    pub fn zoom_selected(&mut self) {
        match self.mode {
            Mode::TwoD => self.zoom_selected_2d(),
            Mode::ThreeD => {
                let sel = self.model.as_ref().map(|m| m.selection()).unwrap_or_default();
                if !sel.is_empty() {
                    self.r3().fit_nodes(&sel);
                }
            }
            Mode::Empty => {}
        }
    }

    /// `update_section`: the slider maps linearly over the model box along the chosen axis.
    pub fn update_section(&mut self, announce: bool) {
        let ax = self.opts.section_axis;
        let bounds = self.model.as_ref().and_then(|m| m.model_box);
        let Some(b) = bounds.filter(|_| ax > 0) else {
            self.r3().set_section(None);
            return;
        };
        let (lo, hi) = (b.min[ax - 1], b.max[ax - 1]);
        let pos = lo + (hi - lo) * f64::from(self.opts.section_slider) / 1000.0;
        self.r3().set_section(Some(Section { axis: ax - 1, position: pos, flip: self.opts.section_flip }));
        if announce {
            self.status_msg(format!("Řez {} = {} mm", ["X", "Y", "Z"][ax - 1], fmt_len(pos)), 2000);
        }
    }

    pub fn on_scale_bar(&mut self, on: bool) {
        self.settings.scale_bar = on;
        self.save_settings();
        self.apply_style3();
    }

    // ------------------------------------------------------------------ tree

    /// What the user did in the tree list.
    pub fn tree_act(&mut self, act: TreeAct) {
        match act {
            TreeAct::Click { node, ctrl, shift } => {
                self.focus.tree = true;
                self.focus.view = false;
                if let Some(m) = self.model.as_mut() {
                    m.click_row(node, ctrl, shift);
                }
                self.on_tree_selection();
            }
            TreeAct::Expand(node) => {
                if let Some(m) = self.model.as_mut() {
                    let i = node as usize;
                    m.expanded[i] = !m.expanded[i];
                    m.rows_dirty = true;
                }
            }
            TreeAct::Check { node, on } => {
                if let Some(m) = self.model.as_mut() {
                    if m.set_checked(&[node], on) {
                        self.visibility_changed();
                    }
                }
            }
            TreeAct::Double(node) => {
                if let Some(m) = self.model.as_mut() {
                    if !m.selected.contains(&node) {
                        m.select_only([node]);
                    }
                }
                self.zoom_selected();
            }
            TreeAct::Context { node, pos } => {
                self.focus.tree = true;
                self.focus.view = false;
                let need = self.model.as_ref().is_some_and(|m| !m.selected.contains(&node));
                if need {
                    if let Some(m) = self.model.as_mut() {
                        m.select_only([node]);
                        m.anchor = Some(node);
                    }
                    self.on_tree_selection();
                }
                self.open_context_menu(pos);
            }
        }
    }

    /// What the user did in the layer list.
    pub fn layer_act(&mut self, act: LayerAct) {
        match act {
            LayerAct::Click { id, ctrl, shift } => {
                self.focus.tree = true;
                self.focus.view = false;
                let filter = self.filter.clone();
                if let Some(d) = self.draw.as_mut() {
                    let order: Vec<u32> = d.filtered(&filter).iter().map(|l| l.id).collect();
                    select_ids(&mut d.selected_layers, &mut d.anchor, &order, id, ctrl, shift);
                }
                self.show_layer_props();
            }
            LayerAct::Check { id, on } => self.toggle_layer(id, on),
            LayerAct::Context { id, pos } => {
                let need = self.draw.as_ref().is_some_and(|d| !d.selected_layers.contains(&id));
                if need {
                    if let Some(d) = self.draw.as_mut() {
                        d.selected_layers.clear();
                        d.selected_layers.insert(id);
                        d.anchor = Some(id);
                    }
                    self.show_layer_props();
                }
                self.open_context_menu(pos);
            }
        }
    }

    /// `on_tree_selection`: the selection in the list goes to the view and the panel.
    pub fn on_tree_selection(&mut self) {
        if self.mode == Mode::TwoD {
            self.show_layer_props();
            return;
        }
        if self.dlg.collisions.is_none() {
            self.overlays.cog = Overlays::default(); // centre of gravity of the previous selection
            self.refresh_overlays();
        }
        self.sync_selection_to_renderer();
        self.refresh_props();
    }

    /// Check boxes changed: the renderer hides the unchecked parts.
    pub fn visibility_changed(&mut self) {
        let Some(m) = &self.model else { return };
        let hidden = m.hidden.clone();
        self.r3().set_visibility(&hidden);
    }

    pub fn rebuild_tree_rows(&mut self) {
        if let Some(m) = self.model.as_mut() {
            m.rows_dirty = true;
        }
    }

    /// Tree keys while the tree has the keyboard: arrows move, Left / Right close / open, Space toggles the check box.
    pub fn tree_keys(&mut self, ctx: &egui::Context) {
        if !self.focus.tree || self.mode != Mode::ThreeD || ctx.egui_wants_keyboard_input() || self.modal_open() {
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = ev else { continue };
            let Some(m) = self.model.as_mut() else { return };
            let cur = m.anchor.filter(|a| m.selected.contains(a)).or_else(|| m.selected.iter().next().copied());
            let pos = cur.and_then(|c| m.rows.iter().position(|(r, _)| *r == c));
            match key {
                egui::Key::ArrowDown | egui::Key::ArrowUp => {
                    let n = m.rows.len();
                    if n == 0 {
                        continue;
                    }
                    let next = match (key, pos) {
                        (egui::Key::ArrowDown, Some(p)) => (p + 1).min(n - 1),
                        (egui::Key::ArrowUp, Some(p)) => p.saturating_sub(1),
                        _ => 0,
                    };
                    let node = m.rows[next].0;
                    m.click_row(node, false, modifiers.shift);
                    m.scroll_to = Some(node);
                    self.on_tree_selection();
                }
                egui::Key::ArrowRight => {
                    if let Some(c) = cur {
                        if !m.node(c).children.is_empty() && !m.expanded[c as usize] {
                            m.expanded[c as usize] = true;
                            m.rows_dirty = true;
                        }
                    }
                }
                egui::Key::ArrowLeft => {
                    if let Some(c) = cur {
                        if m.expanded[c as usize] && !m.node(c).children.is_empty() {
                            m.expanded[c as usize] = false;
                            m.rows_dirty = true;
                        } else if let Some(p) = m.node(c).parent {
                            m.select_only([p]);
                            m.anchor = Some(p);
                            m.scroll_to = Some(p);
                            self.on_tree_selection();
                        }
                    }
                }
                egui::Key::Space => {
                    if let Some(c) = cur {
                        let on = m.check(c) != crate::model3::Check::On;
                        if m.set_checked(&[c], on) {
                            self.visibility_changed();
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `STEPVIEW_SELECT` and "mat:<idx>" links: select one node as the tree would.
    pub fn select_node(&mut self, node: NodeId) {
        if let Some(m) = self.model.as_mut() {
            m.select_only([node]);
            m.anchor = Some(node);
            m.expand_ancestors(node);
            m.rows_dirty = true;
        }
        self.on_tree_selection();
    }

    // ------------------------------------------------------------------ visibility

    /// Nodes the commands act on: the selection of the list.
    fn selected_nodes(&self) -> Vec<NodeId> {
        self.model.as_ref().map(|m| m.selection()).unwrap_or_default()
    }

    pub fn hide_selected(&mut self) {
        if self.mode == Mode::TwoD {
            let ids = self.selected_layer_ids();
            if !ids.is_empty() {
                self.set_layers_visible(&ids, false, None);
            }
            return;
        }
        let sel = self.selected_nodes();
        let Some(m) = self.model.as_mut() else { return };
        m.selected.clear();
        if m.set_checked(&sel, false) {
            self.visibility_changed();
        }
        self.on_tree_selection();
    }

    pub fn isolate_selected(&mut self) {
        if self.mode == Mode::TwoD {
            let ids = self.selected_layer_ids();
            if !ids.is_empty() {
                self.set_layers_visible(&ids, true, Some(false));
            }
            return;
        }
        let sel = self.selected_nodes();
        if sel.is_empty() {
            return;
        }
        let Some(m) = self.model.as_mut() else { return };
        let roots = m.tree.roots.clone();
        m.set_checked(&roots, false);
        m.set_checked(&sel, true);
        self.visibility_changed();
        self.on_tree_selection();
        self.zoom_selected();
    }

    pub fn show_all(&mut self) {
        if self.mode == Mode::TwoD {
            self.set_layers_visible(&[], true, Some(true));
            return;
        }
        let Some(m) = self.model.as_mut() else { return };
        let roots = m.tree.roots.clone();
        if m.set_checked(&roots, true) {
            self.visibility_changed();
        }
    }

    /// `toggle_transp_selected`: 75 % transparency on, or off when any selected part has it.
    pub fn toggle_transparency(&mut self) {
        if self.mode != Mode::ThreeD {
            return;
        }
        let Some(m) = self.model.as_mut() else { return };
        let leaves = m.unique_leaves(&m.selection());
        if leaves.is_empty() {
            return;
        }
        let on = {
            let r = self.r3.lock().unwrap_or_else(|e| e.into_inner());
            !leaves.iter().any(|l| r.transparency(*l) > 0.0)
        };
        self.r3().set_transparency(&leaves, if on { 0.75 } else { 0.0 });
        if let Some(m) = self.model.as_mut() {
            m.selected.clear();
        }
        self.on_tree_selection();
    }

    /// `clear_selection` (Esc): ends the measure tool first, otherwise clears the selection.
    pub fn clear_selection(&mut self) {
        if self.measure.is_some() {
            self.set_measure(None);
            return;
        }
        if self.mode == Mode::TwoD {
            if let Some(d) = self.draw.as_mut() {
                d.selected_layers.clear();
            }
            self.set_entity(None);
            return;
        }
        if let Some(m) = self.model.as_mut() {
            m.selected.clear();
        }
        self.on_tree_selection();
    }

    // ------------------------------------------------------------------ materials and mass

    /// `assign_material`.
    pub fn assign_material(&mut self) {
        if self.mode != Mode::ThreeD {
            return;
        }
        let Some(m) = &self.model else { return };
        if !m.has_nodes() {
            return;
        }
        let sel = m.selection();
        let leaves = m.unique_leaves(&sel);
        let default = m.mats.as_ref().and_then(|a| a.default.clone());
        let dlg = if let Some(first) = leaves.first() {
            let cur = m.material_of(*first).map(|a| (a.name, a.density));
            MaterialDlg::new(format!("Materiál – {} dílů", leaves.len()), leaves.clone(), cur, true, default)
        } else {
            MaterialDlg::new("Výchozí materiál pro díly bez materiálu".to_string(), Vec::new(), default, false, None)
        };
        self.dlg.material = Some(dlg);
    }

    /// OK of the material dialog.
    pub fn apply_material(&mut self, dlg: MaterialDlg) {
        let material = dlg.result_material();
        let default = dlg.result_default();
        let data_dir = self.data_dir.clone();
        let Some(m) = self.model.as_mut() else { return };
        let names: Vec<String> = dlg.leaves.iter().map(|l| m.proto_name_of(*l)).collect();
        let sel = m.selection();
        if m.mats.is_none() {
            m.mats = Assignments::load(&data_dir, &m.path).ok();
        }
        let mut failed = false;
        if let Some(a) = m.mats.as_mut() {
            let mat = material.as_ref().map(|(n, d)| (n.as_str(), *d));
            if !dlg.leaves.is_empty() {
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                failed |= a.set(&refs, mat).is_err();
                failed |= a.set_default(default.as_ref().map(|(n, d)| (n.as_str(), *d))).is_err();
            } else {
                failed |= a.set_default(mat).is_err();
            }
        }
        if failed {
            self.warn("Materiály se nepodařilo uložit.");
        }
        if sel.len() == 1 {
            self.refresh_props();
        } else {
            self.show_mass();
        }
    }

    /// `show_mass`: mass, volume and centre of gravity of the selection, or of the whole model.
    pub fn show_mass(&mut self) {
        if self.mode != Mode::ThreeD {
            return;
        }
        let Some(m) = self.model.as_mut() else { return };
        if !m.has_nodes() {
            return;
        }
        let sel = m.selection();
        let leaves = m.unique_leaves(&if sel.is_empty() { m.tree.roots.clone() } else { sel.clone() });
        let d = m.mass_totals(&leaves);
        let mut rows = vec![
            PropRow::heading(mass::HEADING),
            PropRow::text(
                "Rozsah",
                if sel.is_empty() {
                    format!("celý model, {} dílů", leaves.len())
                } else {
                    format!("{} vybraných položek, {} dílů", sel.len(), leaves.len())
                },
            ),
        ];
        rows.extend(mass::rows(&d, None));
        rows.push(PropRow::spans("", vec![Span::Link("Materiály…".into(), LinkTarget::MaterialAll)]));
        self.props = Props::rows(None, rows);
        let mut o = Overlays::default();
        if let Some(c) = d.cog {
            o.center_of_gravity(c, &format!("  T  {} kg", fmt_num(d.mass, 3)));
        } else if d.volume > 0.0 {
            o.center_of_gravity(d.vcog, "  T (objem)");
        }
        self.overlays.cog = o;
        self.refresh_overlays();
        self.status_msg(strings::COG_STATUS, 5000);
    }

    /// `on_props_link`.
    pub fn on_props_link(&mut self, link: LinkTarget) {
        match link {
            LinkTarget::Material(n) => {
                self.select_node(n);
                self.assign_material();
            }
            LinkTarget::MaterialAll => self.assign_material(),
            LinkTarget::Calc(n) => {
                if let Some(m) = self.model.as_mut() {
                    let leaves = m.leaves_of(n);
                    let _ = m.mass_totals(&leaves);
                    self.props = m.node_props(n);
                }
            }
        }
    }

    /// Files from drag and drop and from the recent list.
    pub fn open_dropped(&mut self, files: &[std::path::PathBuf]) {
        if let Some(p) = files.iter().find(|p| is_supported(p)) {
            self.open_file(p.clone(), None);
        }
    }
}

/// Qt's `ExtendedSelection` over an ordered list of ids.
pub fn select_ids(
    selected: &mut std::collections::BTreeSet<u32>,
    anchor: &mut Option<u32>,
    order: &[u32],
    id: u32,
    ctrl: bool,
    shift: bool,
) {
    if shift {
        if let Some(a) = *anchor {
            let (i, j) = (order.iter().position(|x| *x == a), order.iter().position(|x| *x == id));
            if let (Some(i), Some(j)) = (i, j) {
                if !ctrl {
                    selected.clear();
                }
                selected.extend(order[i.min(j)..=i.max(j)].iter().copied());
                return;
            }
        }
    }
    if ctrl {
        if !selected.remove(&id) {
            selected.insert(id);
        }
    } else {
        selected.clear();
        selected.insert(id);
    }
    *anchor = Some(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn extended_selection_over_layers() {
        let order = vec![10, 11, 12, 13];
        let mut sel = BTreeSet::new();
        let mut anchor = None;
        select_ids(&mut sel, &mut anchor, &order, 11, false, false);
        assert_eq!(sel.iter().copied().collect::<Vec<_>>(), vec![11]);
        select_ids(&mut sel, &mut anchor, &order, 13, true, false);
        assert_eq!(sel.iter().copied().collect::<Vec<_>>(), vec![11, 13]);
        select_ids(&mut sel, &mut anchor, &order, 11, true, false);
        assert_eq!(sel.iter().copied().collect::<Vec<_>>(), vec![13]);
        select_ids(&mut sel, &mut anchor, &order, 10, false, false);
        select_ids(&mut sel, &mut anchor, &order, 12, false, true);
        assert_eq!(sel.iter().copied().collect::<Vec<_>>(), vec![10, 11, 12]);
    }
}
