//! The 2D view: pan, zoom, picking, the measure tools and the rulers (`DrawingView`, `Ruler`, `DrawingPanel`).
//!
//! * left click picks the entity under the cursor (5 px), empty space clears; dragging with any button pans (after
//!   4 px); the wheel zooms by 1.2 per notch under the cursor; a double click zooms to the selection; a right click
//!   opens the context menu;
//! * measure tools: clicks place points that snap to path vertices within 10 px, Shift = horizontal / vertical.
use std::time::{Duration, Instant};

use cad_render::interaction::{drag_started, wheel_factor, PICK_RADIUS_2D_PX, SNAP_RADIUS_2D_PX};
use cad_render::view2d::{ruler_tick_length, ruler_ticks, RULER_THICKNESS};
use cad_render::{Renderer2d, Viewport};
use eframe::egui::{self, Align2, Color32, Event, FontId, PointerButton, Pos2, Rect, Sense, Stroke};

use crate::app::{App, MeasureKind};
use crate::drawing2::{angle_at, distance_status, measure_label};
use crate::menus::ContextMenu;
use crate::view3d::wheel_angle_delta;

struct Press2 {
    button: PointerButton,
    pos: Pos2,
    last: Pos2,
    dragging: bool,
}

#[derive(Default)]
pub struct Mouse2 {
    press: Option<Press2>,
    /// 3D mouse: last motion and the sub-pixel remainder of the pan.
    pub sm_last: Option<Instant>,
    pub sm_frac: [f64; 2],
    /// Cursor in drawing coordinates, for the ruler marks.
    pub cursor_world: Option<[f64; 2]>,
}

impl App {
    /// The central area in 2D mode: rulers, the drawing and the mouse.
    pub fn show_view2d(&mut self, ui: &mut egui::Ui) {
        let full = ui.available_rect_before_wrap();
        let _ = ui.allocate_rect(full, Sense::click_and_drag());
        let thick = if self.settings.rulers { RULER_THICKNESS as f32 } else { 0.0 };
        let view_rect = Rect::from_min_max(full.min + egui::vec2(thick, thick), full.max);
        self.view_rect = view_rect;
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((view_rect.width() * ppp).round().max(1.0) as u32, (view_rect.height() * ppp).round().max(1.0) as u32);
        self.view_px = (w, h, f64::from(ppp));
        self.r2().set_viewport(w, h, f64::from(ppp));
        ui.painter().add(Renderer2d::paint_callback(&self.r2, view_rect));
        if self.draw.is_none() {
            return;
        }
        if self.settings.rulers {
            self.paint_rulers(ui, full, view_rect, thick);
        }
        self.view2_input(ui, view_rect, ppp);
    }

    fn paint_rulers(&self, ui: &egui::Ui, full: Rect, view: Rect, thick: f32) {
        let painter = ui.painter_at(full);
        let bg = ui.visuals().panel_fill;
        let fg = ui.visuals().text_color();
        let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let font = FontId::proportional(10.0);
        painter.rect_filled(Rect::from_min_max(full.min, Pos2::new(full.max.x, full.min.y + thick)), 0.0, bg);
        painter.rect_filled(Rect::from_min_max(full.min, Pos2::new(full.min.x + thick, full.max.y)), 0.0, bg);
        let r = self.r2().view;
        let vp = Viewport::new(f64::from(view.width() * ui.ctx().pixels_per_point()), f64::from(view.height() * ui.ctx().pixels_per_point()));
        let cursor = self.mouse2.cursor_world;
        // horizontal ruler: x runs from the left edge of the view to the right edge
        let ax = r.to_world([0.0, 0.0], vp)[0];
        let bx = r.to_world([vp.width, 0.0], vp)[0];
        let n = f64::from(view.width());
        for t in ruler_ticks(ax, bx, n) {
            let x = view.min.x + t.pos as f32;
            let tl = thick * ruler_tick_length(t.major) as f32;
            painter.line_segment([Pos2::new(x, full.min.y + thick - tl), Pos2::new(x, full.min.y + thick)], Stroke::new(1.0, fg));
            if let Some(label) = t.label {
                painter.text(Pos2::new(x + 3.0, full.min.y + 1.0), Align2::LEFT_TOP, label, font.clone(), fg);
            }
        }
        painter.line_segment([Pos2::new(full.min.x, full.min.y + thick - 1.0), Pos2::new(full.max.x, full.min.y + thick - 1.0)], Stroke::new(1.0, line));
        // vertical ruler: y runs from the top of the view (the larger value) to the bottom
        let ay = r.to_world([0.0, 0.0], vp)[1];
        let by = r.to_world([0.0, vp.height], vp)[1];
        let n = f64::from(view.height());
        for t in ruler_ticks(ay, by, n) {
            let y = view.min.y + t.pos as f32;
            let tl = thick * ruler_tick_length(t.major) as f32;
            painter.line_segment([Pos2::new(full.min.x + thick - tl, y), Pos2::new(full.min.x + thick, y)], Stroke::new(1.0, fg));
            if let Some(label) = t.label {
                // text rotated by -90 degrees, reading upwards
                let galley = painter.layout_no_wrap(label, font.clone(), fg);
                let shape = egui::epaint::TextShape::new(Pos2::new(full.min.x + 1.0, y - 3.0), galley, fg).with_angle(-std::f32::consts::FRAC_PI_2);
                painter.add(shape);
            }
        }
        painter.line_segment([Pos2::new(full.min.x + thick - 1.0, full.min.y), Pos2::new(full.min.x + thick - 1.0, full.max.y)], Stroke::new(1.0, line));
        // marks of the cursor
        if let Some(c) = cursor {
            let red = Color32::from_rgb(230, 60, 60);
            let px = ((c[0] - ax) / (bx - ax) * f64::from(view.width())) as f32;
            if px.is_finite() && (0.0..=view.width()).contains(&px) {
                painter.line_segment([Pos2::new(view.min.x + px, full.min.y), Pos2::new(view.min.x + px, full.min.y + thick)], Stroke::new(1.0, red));
            }
            let py = ((c[1] - ay) / (by - ay) * f64::from(view.height())) as f32;
            if py.is_finite() && (0.0..=view.height()).contains(&py) {
                painter.line_segment([Pos2::new(full.min.x, view.min.y + py), Pos2::new(full.min.x + thick, view.min.y + py)], Stroke::new(1.0, red));
            }
        }
    }

    fn view_px_of(view: Rect, pos: Pos2, ppp: f32) -> [f64; 2] {
        [f64::from((pos.x - view.min.x) * ppp), f64::from((pos.y - view.min.y) * ppp)]
    }

    fn view2_input(&mut self, ui: &egui::Ui, view: Rect, ppp: f32) {
        let ctx = ui.ctx().clone();
        let layer = ui.layer_id();
        let over = |pos: Pos2| view.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == layer);
        let modal = self.modal_open();
        let events = ctx.input(|i| i.events.clone());
        let shift_now = ctx.input(|i| i.modifiers.shift);
        let mut moved: Option<Pos2> = None;
        for ev in events {
            match ev {
                Event::PointerButton { pos, button, pressed: true, .. } if over(pos) && !modal => {
                    self.focus.view = true;
                    self.focus.tree = false;
                    self.close_context_menu();
                    self.mouse2.press = Some(Press2 { button, pos, last: pos, dragging: false });
                }
                Event::PointerMoved(pos) => {
                    if self.mouse2.press.is_some() {
                        let Some(p) = self.mouse2.press.as_mut() else { continue };
                        if !p.dragging {
                            if !drag_started(f64::from((pos.x - p.pos.x).abs()), f64::from((pos.y - p.pos.y).abs())) {
                                continue;
                            }
                            p.dragging = true;
                        }
                        let (dx, dy) = ((pos.x - p.last.x) * ppp, (pos.y - p.last.y) * ppp);
                        p.last = pos;
                        self.r2().view.pan_pixels(f64::from(dx), f64::from(dy));
                    }
                    if over(pos) && !modal {
                        moved = Some(pos);
                    }
                }
                Event::PointerButton { pos, button, pressed: false, modifiers } => {
                    let Some(p) = self.mouse2.press.take() else { continue };
                    if p.button != button || p.dragging {
                        continue;
                    }
                    match button {
                        PointerButton::Primary => {
                            self.click_2d(pos, view, ppp, modifiers.shift);
                            let double = ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                            if double && self.measure.is_none() && self.draw.as_ref().is_some_and(|d| d.picked.is_some()) {
                                self.zoom_selected_2d();
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
                    let recent = self.mouse2.sm_last.is_some_and(|t| t.elapsed() < Duration::from_millis(400));
                    if d != 0.0 && !recent {
                        let px = Self::view_px_of(view, pos, ppp);
                        let mut r = self.r2();
                        let vp = r.viewport();
                        r.view.zoom_at(px, wheel_factor(d), vp);
                    }
                }
                _ => {}
            }
        }
        if let Some(pos) = moved {
            let px = Self::view_px_of(view, pos, ppp);
            let world = self.world_at_2d(px);
            self.mouse2.cursor_world = Some(world);
            self.status.coord = Self::coord_text(world);
            if self.measure.is_some() && self.mouse2.press.is_none() {
                self.hover_2d(pos, view, ppp, shift_now);
            }
        }
        if self.measure.is_some() && ctx.input(|i| i.pointer.hover_pos()).is_some_and(over) {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// Pixels of a view position in logical units and the matching drawing coordinates.
    fn snap_2d(&self, pos: Pos2, view: Rect, ppp: f32) -> Option<[f64; 2]> {
        let d = self.draw.as_ref()?;
        let r = self.r2();
        let vp = r.viewport();
        let cursor_logical = [f64::from(pos.x - view.min.x), f64::from(pos.y - view.min.y)];
        let px = Self::view_px_of(view, pos, ppp);
        let view2 = r.view;
        drop(r);
        let rad = SNAP_RADIUS_2D_PX;
        // the search rectangle: 10 logical pixels around the cursor, in drawing coordinates
        let corner = |dx: f64, dy: f64| {
            view2.to_world([px[0] + dx * f64::from(ppp), px[1] + dy * f64::from(ppp)], vp)
        };
        let (a, b) = (corner(-rad, rad), corner(rad, -rad));
        let rect = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
        let to_px = move |p: [f64; 2]| {
            let s = view2.to_screen(p, vp);
            [s[0] / f64::from(ppp), s[1] / f64::from(ppp)]
        };
        d.drawing.snap(rect, &to_px, cursor_logical)
    }

    /// The point of a measuring click or move: snapped to the geometry, constrained with Shift.
    fn measure_point_2d(&self, pos: Pos2, view: Rect, ppp: f32, shift: bool) -> [f64; 2] {
        let sp = self.snap_2d(pos, view, ppp).unwrap_or_else(|| self.world_at_2d(Self::view_px_of(view, pos, ppp)));
        match (shift, self.meas2.pts.last()) {
            (true, Some(a)) => {
                if (sp[0] - a[0]).abs() >= (sp[1] - a[1]).abs() {
                    [sp[0], a[1]]
                } else {
                    [a[0], sp[1]]
                }
            }
            _ => sp,
        }
    }

    /// Cursor moved in a measure tool: snap marker and the rubber band.
    fn hover_2d(&mut self, pos: Pos2, view: Rect, ppp: f32, shift: bool) {
        self.meas2.snap = self.snap_2d(pos, view, ppp);
        self.meas2.rubber = self.meas2.pts.last().map(|a| (*a, self.measure_point_2d(pos, view, ppp, shift)));
        self.refresh_overlays_2d();
    }

    /// Left click without a drag.
    fn click_2d(&mut self, pos: Pos2, view: Rect, ppp: f32, shift: bool) {
        match self.measure {
            Some(kind) => self.measure_click_2d(pos, view, ppp, shift, kind),
            None => {
                let px = Self::view_px_of(view, pos, ppp);
                let world = self.world_at_2d(px);
                let tol = {
                    let r = self.r2();
                    // 5 logical pixels
                    PICK_RADIUS_2D_PX * f64::from(ppp) * r.view.units_per_pixel()
                };
                let hit = self.draw.as_ref().and_then(|d| d.drawing.pick(world, tol));
                self.set_entity(hit);
            }
        }
    }

    /// `_measure_click`.
    fn measure_click_2d(&mut self, pos: Pos2, view: Rect, ppp: f32, shift: bool, kind: MeasureKind) {
        let sp = self.measure_point_2d(pos, view, ppp, shift);
        self.meas2.pts.push(sp);
        self.meas2.dots.push(sp);
        self.meas2.rubber = None;
        let pts = self.meas2.pts.clone();
        match (kind, pts.len()) {
            (MeasureKind::Dist, 2) => {
                let (a, b) = (pts[0], pts[1]);
                self.meas2.lines.push((a, b));
                let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                self.meas2.texts.push((mid(a, b), measure_label(len)));
                self.meas2.pts.clear();
                self.status.model = distance_status(a, b);
            }
            (MeasureKind::Angle, 3) => {
                let (a, v, c) = (pts[0], pts[1], pts[2]);
                self.meas2.lines.push((v, c));
                let ang = angle_at(a, v, c);
                self.meas2.texts.push((v, format!("{}°", measure_label(ang))));
                self.meas2.pts.clear();
                self.status.model = format!("Úhel: {}°   – další 3 kliky = nové měření, Esc = konec", cad_core::fmt::fmt_len(ang));
            }
            (MeasureKind::Angle, 2) => self.meas2.lines.push((pts[0], pts[1])),
            _ => {}
        }
        self.refresh_overlays_2d();
    }

    /// `space_motion` of the 2D view: push = pan, pull towards you = zoom in; rotations are unused.
    pub fn space_motion_2d(&mut self, v: [f64; 6], dt: f64) {
        self.mouse2.sm_last = Some(Instant::now());
        let frac = self.mouse2.sm_frac;
        let next = self.r2().view.space_motion(v, dt, frac);
        self.mouse2.sm_frac = next;
    }
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midpoint() {
        assert_eq!(mid([0.0, 2.0], [4.0, 6.0]), [2.0, 4.0]);
    }
}
