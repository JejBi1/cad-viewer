//! The 3D view: mouse and keyboard state machine (`OccView` of `viewer3d.py`, SPEC section 7).
//!
//! * left click selects, Ctrl+click adds / removes, a click on empty space clears, double click zooms to the
//!   selection; in the measure tools it picks;
//! * right button drag orbits about the point under the cursor at press time (else the view centre), a right click
//!   without a drag opens the context menu;
//! * middle button, or Ctrl + right button, pans; the wheel zooms about the cursor by `1.2 ** notches`;
//! * arrow keys orbit by 15 degrees, with Shift by 90; Enter pins a measurement;
//! * a click on the view cube turns the view.
use std::time::{Duration, Instant};

use cad_render::interaction::{arrow_orbit, drag_started, orbit_angles, wheel_factor, ArrowKey, WHEEL_IGNORE_AFTER_3D_MOUSE_SECS};
use cad_render::{Highlight, Renderer3d};
use cad_types::NodeId;
use eframe::egui::{self, Event, Key, PointerButton, Pos2, Sense};

use crate::app::{App, MeasureKind};
use crate::menus::ContextMenu;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    None,
    Rotate,
    Pan,
}

pub struct Press {
    pub button: PointerButton,
    pub pos: Pos2,
    pub last_px: [f64; 2],
    pub dragging: bool,
    pub mode: DragMode,
    pub pivot: Option<[f64; 3]>,
}

#[derive(Default)]
pub struct Mouse3 {
    pub press: Option<Press>,
    /// Last cursor position in physical pixels of the view while no button is down.
    pub hover_px: Option<[f32; 2]>,
    pub hover_dirty: bool,
    pub shift: bool,
    /// 3D mouse: time of the last motion and the pivot picked when a motion starts.
    pub sm_last: Option<Instant>,
    pub sm_pivot: Option<[f64; 3]>,
}

/// Wheel delta of an egui event in Qt "angle delta" units (120 per notch).
pub fn wheel_angle_delta(unit: egui::MouseWheelUnit, delta_y: f32) -> f64 {
    let notches = match unit {
        egui::MouseWheelUnit::Line => f64::from(delta_y),
        egui::MouseWheelUnit::Page => f64::from(delta_y) * 10.0,
        // touchpads: about 50 points per notch (egui scales a line to 40 points)
        egui::MouseWheelUnit::Point => f64::from(delta_y) / 50.0,
    };
    notches * 120.0
}

impl App {
    /// The central area in 3D mode: paints the renderer and handles the mouse.
    pub fn show_view3d(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        self.view_rect = rect;
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32);
        self.view_px = (w, h, f64::from(ppp));
        self.r3().set_viewport(w, h, f64::from(ppp));
        ui.painter().add(Renderer3d::paint_callback(&self.r3, rect));
        if self.model.is_none() {
            return;
        }
        self.view3_input(ui, rect, &resp, ppp);
        // the view animation of the view cube
        let animating = self.r3().is_animating();
        if animating {
            let dt = ui.ctx().input(|i| i.stable_dt) as f64;
            self.r3().advance_animation(dt);
            ui.ctx().request_repaint();
        }
    }

    fn to_px(rect: egui::Rect, pos: Pos2, ppp: f32) -> [f32; 2] {
        [(pos.x - rect.min.x) * ppp, (pos.y - rect.min.y) * ppp]
    }

    fn view3_input(&mut self, ui: &egui::Ui, rect: egui::Rect, resp: &egui::Response, ppp: f32) {
        let ctx = ui.ctx().clone();
        let layer = ui.layer_id();
        let over = |pos: Pos2| rect.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == layer);
        let events = ctx.input(|i| i.events.clone());
        let modal = self.modal_open();
        for ev in events {
            match ev {
                Event::PointerButton { pos, button, pressed: true, modifiers } if over(pos) && !modal => {
                    self.focus.view = true;
                    self.focus.tree = false;
                    let px = Self::to_px(rect, pos, ppp);
                    let mut press = Press {
                        button,
                        pos,
                        last_px: [f64::from(px[0]), f64::from(px[1])],
                        dragging: false,
                        mode: DragMode::None,
                        pivot: None,
                    };
                    // Onshape: RMB drag = rotate about the point under the cursor, MMB / Ctrl+RMB = pan, LMB = select
                    if button == PointerButton::Secondary && !modifiers.ctrl {
                        press.mode = DragMode::Rotate;
                        let mut r = self.r3();
                        let c = r.camera.center.to_array();
                        press.pivot = Some(r.pivot_at(px[0], px[1]).unwrap_or(c));
                    } else if matches!(button, PointerButton::Middle | PointerButton::Secondary) {
                        press.mode = DragMode::Pan;
                    }
                    self.close_context_menu();
                    self.mouse3.press = Some(press);
                }
                Event::PointerMoved(pos) => {
                    let px = Self::to_px(rect, pos, ppp);
                    if let Some(press) = self.mouse3.press.as_mut() {
                        let (manhattan_x, manhattan_y) = (f64::from((pos.x - press.pos.x).abs()), f64::from((pos.y - press.pos.y).abs()));
                        if !press.dragging {
                            if !drag_started(manhattan_x, manhattan_y) {
                                continue;
                            }
                            press.dragging = true;
                        }
                        let (lx, ly) = (press.last_px[0], press.last_px[1]);
                        let (x, y) = (f64::from(px[0]), f64::from(px[1]));
                        let (mode, pivot) = (press.mode, press.pivot);
                        press.last_px = [x, y];
                        let mut r = self.r3();
                        match mode {
                            DragMode::Rotate => {
                                let dpr = r.dpr();
                                let (ah, av) = orbit_angles(x - lx, y - ly, dpr);
                                let pv = pivot.map(glam_vec);
                                r.camera.orbit(ah, av, pv);
                            }
                            DragMode::Pan => {
                                let vp = r.viewport();
                                r.camera.pan_pixels(x - lx, ly - y, vp);
                            }
                            DragMode::None => {}
                        }
                    } else if over(pos) && !modal {
                        self.mouse3.hover_px = Some(px);
                        self.mouse3.hover_dirty = true;
                    } else {
                        self.mouse3.hover_px = None;
                        self.mouse3.hover_dirty = true;
                    }
                }
                Event::PointerGone => {
                    self.mouse3.hover_px = None;
                    self.mouse3.hover_dirty = true;
                }
                Event::PointerButton { pos, button, pressed: false, modifiers } => {
                    let Some(press) = self.mouse3.press.take() else { continue };
                    if press.button != button || press.dragging {
                        continue;
                    }
                    let px = Self::to_px(rect, pos, ppp);
                    match button {
                        PointerButton::Primary => {
                            self.view_click(px, modifiers.ctrl, modifiers.shift);
                            let double = ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                            if double && self.measure.is_none() {
                                self.zoom_selected();
                            }
                        }
                        PointerButton::Secondary => {
                            self.dlg.context = Some(ContextMenu { pos, opened: false });
                        }
                        _ => {}
                    }
                }
                Event::MouseWheel { unit, delta, .. } => {
                    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { continue };
                    if !over(pos) || modal {
                        continue;
                    }
                    let d = wheel_angle_delta(unit, delta.y);
                    // 3DxWare may emulate the wheel while the cap moves
                    let recent = self
                        .mouse3
                        .sm_last
                        .is_some_and(|t| t.elapsed() < Duration::from_secs_f64(WHEEL_IGNORE_AFTER_3D_MOUSE_SECS));
                    if d == 0.0 || recent {
                        continue;
                    }
                    let px = Self::to_px(rect, pos, ppp);
                    let mut r = self.r3();
                    let vp = r.viewport();
                    r.camera.zoom_at(f64::from(px[0]), f64::from(px[1]), wheel_factor(d), vp);
                }
                Event::Key { key, pressed: true, modifiers, .. } if self.focus.view && !ctx.egui_wants_keyboard_input() && !modal => {
                    let arrow = match key {
                        Key::ArrowLeft => Some(ArrowKey::Left),
                        Key::ArrowRight => Some(ArrowKey::Right),
                        Key::ArrowUp => Some(ArrowKey::Up),
                        Key::ArrowDown => Some(ArrowKey::Down),
                        _ => None,
                    };
                    if let Some(a) = arrow {
                        let other = modifiers.ctrl || modifiers.alt || modifiers.mac_cmd;
                        if let Some((h, v)) = arrow_orbit(a, modifiers.shift, other) {
                            self.r3().camera.orbit(h, v, None);
                        }
                    } else if key == Key::Enter && self.measure == Some(MeasureKind::Dist) {
                        self.pin_measure();
                    }
                }
                _ => {}
            }
        }
        self.mouse3.shift = ctx.input(|i| i.modifiers.shift);
        // hover: one pick per frame, only while no button is down
        if self.mouse3.press.is_none() && self.mouse3.hover_dirty {
            self.mouse3.hover_dirty = false;
            self.view_hover();
        }
        if self.measure.is_some() && resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// Hover highlight (`MoveTo`): a whole part, or in the measure tool the entity under the cursor.
    fn view_hover(&mut self) {
        let Some(px) = self.mouse3.hover_px else {
            self.r3().set_hover(None);
            self.r3().set_view_cube_hover(None);
            if self.measure == Some(MeasureKind::Dist) {
                self.status.coord.clear();
            }
            return;
        };
        let (x, y) = (px[0], px[1]);
        let region = self.r3().view_cube_hit(f64::from(x), f64::from(y));
        self.r3().set_view_cube_hover(region);
        if region.is_some() {
            self.r3().set_hover(None);
            return;
        }
        match self.measure {
            Some(MeasureKind::Dist) => {
                let shift = self.mouse3.shift;
                self.measure_hover(x, y, shift);
            }
            Some(MeasureKind::Angle) => {}
            None => {
                let hit = self.r3().pick_hit(x, y);
                self.r3().set_hover(hit.map(|h| Highlight::Node(h.entity.node)));
            }
        }
    }

    /// Left click without a drag (`mouseReleaseEvent`).
    fn view_click(&mut self, px: [f32; 2], ctrl: bool, shift: bool) {
        let (x, y) = (px[0], px[1]);
        let region = self.r3().view_cube_hit(f64::from(x), f64::from(y));
        if let Some(region) = region {
            self.r3().start_view_animation(region);
            return;
        }
        match self.measure {
            Some(MeasureKind::Dist) => self.measure_pick(x, y, shift),
            Some(MeasureKind::Angle) => self.angle_pick(x, y),
            None => {
                let hit = self.r3().pick_hit(x, y).map(|h| h.entity.node);
                self.view_select(hit, ctrl);
            }
        }
    }

    /// Selection by a click in the view: replace, or toggle with Ctrl; empty space clears.
    pub fn view_select(&mut self, node: Option<NodeId>, ctrl: bool) {
        let Some(m) = self.model.as_mut() else { return };
        match node {
            None => m.selected.clear(),
            Some(n) if ctrl => {
                // XOR on the displayed leaves of the current selection
                let current: Vec<NodeId> = m.unique_leaves(&m.selection()).into_iter().filter(|l| m.is_visible_leaf(*l)).collect();
                let mut set: std::collections::BTreeSet<NodeId> = current.into_iter().collect();
                if !set.remove(&n) {
                    set.insert(n);
                }
                m.select_only(set);
            }
            Some(n) => m.select_only([n]),
        }
        let first = m.selected.iter().next().copied();
        m.anchor = first;
        if let Some(f) = first {
            for n in m.selection() {
                m.expand_ancestors(n);
            }
            m.scroll_to = Some(f);
        }
        m.rows_dirty = true;
        self.on_view_selection();
    }

    /// `on_view_selection`: the tree follows the view selection, then the panel.
    pub fn on_view_selection(&mut self) {
        self.sync_selection_to_renderer();
        self.refresh_props();
    }

    /// Selected parts of the model go to the renderer (assemblies select everything below, hidden parts are skipped).
    pub fn sync_selection_to_renderer(&mut self) {
        let Some(m) = &self.model else { return };
        let nodes: Vec<NodeId> = m.selection();
        self.r3().set_selection(&nodes);
    }

    /// The panel for the current selection (`show_props` and friends).
    pub fn refresh_props(&mut self) {
        let Some(m) = self.model.as_mut() else { return };
        let sel = m.selection();
        let props = match sel.len() {
            0 => m.model_info(self.load_secs),
            1 => m.node_props(sel[0]),
            _ => m.multi_props(&sel),
        };
        if self.measure != Some(MeasureKind::Dist) || sel.is_empty() {
            if self.measure == Some(MeasureKind::Dist) && sel.is_empty() {
                // the measure tool owns the panel
                return;
            }
            self.props = props;
        }
    }

    // ------------------------------------------------------------------ 3D mouse

    /// `space_motion` of the view: the model follows the cap.
    pub fn space_motion_3d(&mut self, v: [f64; 6], dt: f64) {
        if self.mouse3.press.as_ref().is_some_and(|p| p.dragging) || self.model.is_none() {
            return;
        }
        let now = Instant::now();
        let stale = self.mouse3.sm_last.is_none_or(|t| now.duration_since(t) > Duration::from_millis(300));
        if stale || self.mouse3.sm_pivot.is_none() {
            let (w, h, _) = self.view_px;
            let pivot = {
                let mut r = self.r3();
                let c = r.camera.center.to_array();
                r.pivot_at(w as f32 / 2.0, h as f32 / 2.0).unwrap_or(c)
            };
            self.mouse3.sm_pivot = Some(pivot);
        }
        self.mouse3.sm_last = Some(now);
        let pivot = glam_vec(self.mouse3.sm_pivot.unwrap_or([0.0; 3]));
        let mut r = self.r3();
        let (dpr, vp) = (r.dpr(), r.viewport());
        r.camera.space_motion(v, dt, pivot, dpr, vp);
    }
}

fn glam_vec(p: [f64; 3]) -> glam::DVec3 {
    glam::DVec3::from_array(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_units_to_qt_angle_deltas() {
        assert_eq!(wheel_angle_delta(egui::MouseWheelUnit::Line, 1.0), 120.0);
        assert_eq!(wheel_angle_delta(egui::MouseWheelUnit::Line, -2.0), -240.0);
        assert_eq!(wheel_angle_delta(egui::MouseWheelUnit::Point, 50.0), 120.0);
        assert_eq!(wheel_angle_delta(egui::MouseWheelUnit::Page, 1.0), 1200.0);
        assert_eq!(wheel_angle_delta(egui::MouseWheelUnit::Line, 0.0), 0.0);
    }
}
