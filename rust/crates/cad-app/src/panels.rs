//! The left column (filter, tree or layer list, properties panel), the status bar and the start screen.
use cad_render::colors::{rgb_to_srgb8, DEFAULT_PART_COLOR};
use cad_types::NodeId;
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Ui, Vec2};

use crate::app::{App, Mode};
use crate::model3::Check;
use crate::props::{LinkTarget, PropRow, Props, Span};
use crate::rich::{self, MUTED, WARN};
use crate::strings;

/// What the user did in the tree during a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TreeAct {
    Click { node: NodeId, ctrl: bool, shift: bool },
    Expand(NodeId),
    Check { node: NodeId, on: bool },
    Double(NodeId),
    Context { node: NodeId, pos: Pos2 },
}

/// The same for the layer list of a drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerAct {
    Click { id: u32, ctrl: bool, shift: bool },
    Check { id: u32, on: bool },
    Context { id: u32, pos: Pos2 },
}

const INDENT: f32 = 16.0;
const ICON: f32 = 14.0;
const BOX: f32 = 13.0;

fn row_height(ui: &Ui) -> f32 {
    ui.text_style_height(&TextStyle::Body) + 6.0
}

fn paint_check(ui: &Ui, rect: Rect, state: Check) {
    let p = ui.painter();
    let vis = ui.visuals();
    p.rect(rect, 2.0, vis.extreme_bg_color, Stroke::new(1.0, vis.widgets.inactive.fg_stroke.color), egui::StrokeKind::Inside);
    let c = vis.strong_text_color();
    match state {
        Check::On => {
            let a = rect.min + Vec2::new(3.0, 6.5);
            let b = rect.min + Vec2::new(5.5, 9.5);
            let d = rect.min + Vec2::new(10.0, 3.5);
            p.line_segment([a, b], Stroke::new(1.8, c));
            p.line_segment([b, d], Stroke::new(1.8, c));
        }
        Check::Partial => {
            p.rect_filled(rect.shrink(4.0), 1.0, c);
        }
        Check::Off => {}
    }
}

fn paint_node_icon(ui: &Ui, rect: Rect, assembly: bool, color: Option<[u8; 3]>) {
    let p = ui.painter();
    if assembly {
        let fill = color.map(|c| Color32::from_rgb(c[0], c[1], c[2])).unwrap_or(Color32::from_rgb(255, 210, 120));
        let stroke = Stroke::new(1.0, Color32::from_rgb(90, 100, 120));
        let body = Rect::from_min_size(rect.min + Vec2::new(1.0, 3.0), Vec2::new(12.0, 9.0));
        p.rect(body, 0.0, fill, stroke, egui::StrokeKind::Inside);
        let tab = Rect::from_min_size(rect.min + Vec2::new(1.0, 1.0), Vec2::new(6.0, 3.0));
        p.rect(tab, 0.0, fill, stroke, egui::StrokeKind::Inside);
    } else {
        let c = color.unwrap_or(DEFAULT_PART_COLOR);
        p.rect(
            rect.shrink(1.0),
            3.0,
            Color32::from_rgb(c[0], c[1], c[2]),
            Stroke::new(1.0, Color32::from_rgb(70, 70, 80)),
            egui::StrokeKind::Inside,
        );
    }
}

/// Header of the two columns.
fn header(ui: &mut Ui, left: &str, right: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(left).color(MUTED));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(right).color(MUTED));
        });
    });
    ui.separator();
}

impl App {
    /// The left column: filter, tree (3 parts) and the properties panel (1 part), with a movable divider.
    pub fn show_left(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let hint = if self.mode == Mode::TwoD { "Hledat hladinu…" } else { "Hledat ve stromu…" };
        ui.horizontal(|ui| {
            let w = ui.available_width() - 24.0;
            let r = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(hint).desired_width(w.max(40.0)));
            if r.changed() {
                if let Some(m) = self.model.as_mut() {
                    m.rows_dirty = true;
                }
            }
            if r.has_focus() {
                self.focus.tree = false;
                self.focus.view = false;
            }
            if !self.filter.is_empty() && ui.small_button("×").on_hover_text("Smazat").clicked() {
                self.filter.clear();
                if let Some(m) = self.model.as_mut() {
                    m.rows_dirty = true;
                }
            }
        });
        ui.add_space(2.0);
        let total = ui.available_height();
        let handle = 7.0;
        let tree_h = (total * self.layout.tree_frac).clamp(60.0, (total - 60.0 - handle).max(60.0));
        let tree_rect = ui.allocate_ui(Vec2::new(ui.available_width(), tree_h), |ui| {
            ui.set_min_height(tree_h);
            ui.set_max_height(tree_h);
            self.show_tree(ui, tree_h);
        });
        let _ = tree_rect;
        // the divider
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), handle), Sense::drag());
        if resp.dragged() {
            let delta = resp.drag_delta().y;
            self.layout.tree_frac = ((tree_h + delta) / total.max(1.0)).clamp(0.15, 0.9);
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        ui.painter().line_segment(
            [Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)],
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
        let link = egui::ScrollArea::vertical().id_salt("props-scroll").auto_shrink([false, false]).show(ui, |ui| self.show_props(ui)).inner;
        if let Some(l) = link {
            self.on_props_link(l);
        }
    }

    fn show_tree(&mut self, ui: &mut Ui, height: f32) {
        if self.mode == Mode::TwoD {
            self.show_layers(ui, height);
            return;
        }
        header(ui, "Název", "Info");
        let filter = self.filter.clone();
        let Some(m) = self.model.as_mut() else {
            ui.allocate_space(Vec2::new(ui.available_width(), (height - 30.0).max(0.0)));
            return;
        };
        if m.rows_dirty {
            m.rebuild_rows(&filter);
        }
        let row_h = row_height(ui);
        let mut acts: Vec<TreeAct> = Vec::new();
        let scroll_to = m.scroll_to.take().and_then(|n| m.rows.iter().position(|(r, _)| *r == n));
        let m_ref = &*m;
        let avail_h = (height - 30.0).max(40.0);
        let mut sa = egui::ScrollArea::vertical().id_salt("tree-scroll").auto_shrink([false, false]).max_height(avail_h);
        if let Some(i) = scroll_to {
            sa = sa.vertical_scroll_offset((i as f32 * row_h - avail_h / 2.0 + row_h).max(0.0));
        }
        let body_font = TextStyle::Body.resolve(ui.style());
        sa.show_rows(ui, row_h, m_ref.rows.len(), |ui, range| {
            // the width of the info column: the widest text of the rows in view
            let info_texts: Vec<String> = range.clone().map(|i| m_ref.info_text(m_ref.rows[i].0)).collect();
            let info_w = info_texts
                .iter()
                .map(|t| ui.painter().layout_no_wrap(t.clone(), body_font.clone(), Color32::WHITE).size().x)
                .fold(0.0f32, f32::max)
                + 8.0;
            for (k, i) in range.clone().enumerate() {
                let (node, depth) = m_ref.rows[i];
                let n = m_ref.node(node);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                let selected = m_ref.selected.contains(&node);
                let painter = ui.painter();
                if selected {
                    painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let x0 = rect.left() + 4.0 + f32::from(depth) * INDENT;
                let cy = rect.center().y;
                let arrow = Rect::from_center_size(Pos2::new(x0 + 6.0, cy), Vec2::splat(12.0));
                let check = Rect::from_center_size(Pos2::new(x0 + 14.0 + BOX / 2.0 + 2.0, cy), Vec2::splat(BOX));
                let icon = Rect::from_center_size(Pos2::new(check.right() + 4.0 + ICON / 2.0, cy), Vec2::splat(ICON));
                let has_children = !n.children.is_empty();
                if has_children {
                    let open = m_ref.expanded[node as usize];
                    let col = ui.visuals().text_color();
                    let c = arrow.center();
                    let pts = if open {
                        vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)]
                    } else {
                        vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(-2.0, 4.0), c + Vec2::new(3.0, 0.0)]
                    };
                    painter.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
                }
                paint_check(ui, check, m_ref.check(node));
                paint_node_icon(ui, icon, n.is_assembly, n.color.map(rgb_to_srgb8));
                let text_col = ui.visuals().text_color();
                let info_rect_left = rect.right() - info_w - 4.0;
                let name_clip = Rect::from_min_max(Pos2::new(icon.right() + 5.0, rect.top()), Pos2::new(info_rect_left - 4.0, rect.bottom()));
                ui.painter().with_clip_rect(name_clip).text(
                    Pos2::new(icon.right() + 5.0, cy),
                    Align2::LEFT_CENTER,
                    &n.name,
                    body_font.clone(),
                    text_col,
                );
                ui.painter().text(Pos2::new(rect.right() - 4.0, cy), Align2::RIGHT_CENTER, &info_texts[k], body_font.clone(), MUTED);
                if resp.hovered() {
                    resp.clone().on_hover_text(m_ref.tooltip(node));
                }
                let pos = resp.interact_pointer_pos();
                let on_arrow = pos.is_some_and(|p| arrow.expand(2.0).contains(p)) && has_children;
                let on_check = pos.is_some_and(|p| check.expand(2.0).contains(p));
                let mods = ui.input(|i| i.modifiers);
                if resp.clicked() {
                    if on_arrow {
                        acts.push(TreeAct::Expand(node));
                    } else if on_check {
                        acts.push(TreeAct::Check { node, on: m_ref.check(node).after_click() });
                    } else {
                        acts.push(TreeAct::Click { node, ctrl: mods.ctrl || mods.command, shift: mods.shift });
                    }
                }
                if resp.double_clicked() && !on_arrow && !on_check {
                    acts.push(TreeAct::Double(node));
                }
                if resp.secondary_clicked() {
                    acts.push(TreeAct::Context { node, pos: pos.unwrap_or(rect.center()) });
                }
            }
        });
        for a in acts {
            self.tree_act(a);
        }
    }

    /// The layer list of a drawing (`build_layer_items`).
    fn show_layers(&mut self, ui: &mut Ui, height: f32) {
        header(ui, "Hladina", "Entit");
        let filter = self.filter.clone();
        let row_h = row_height(ui);
        let mut acts: Vec<LayerAct> = Vec::new();
        let Some(d) = self.draw.as_ref() else {
            ui.allocate_space(Vec2::new(ui.available_width(), (height - 30.0).max(0.0)));
            return;
        };
        let rows = d.filtered(&filter);
        let body_font = TextStyle::Body.resolve(ui.style());
        let avail_h = (height - 30.0).max(40.0);
        egui::ScrollArea::vertical().id_salt("layers-scroll").auto_shrink([false, false]).max_height(avail_h).show_rows(
            ui,
            row_h,
            rows.len(),
            |ui, range| {
                for i in range {
                    let l = rows[i];
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
                    let selected = d.selected_layers.contains(&l.id);
                    let painter = ui.painter();
                    if selected {
                        painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                    } else if resp.hovered() {
                        painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
                    }
                    let cy = rect.center().y;
                    let check = Rect::from_center_size(Pos2::new(rect.left() + 4.0 + BOX / 2.0, cy), Vec2::splat(BOX));
                    let icon = Rect::from_center_size(Pos2::new(check.right() + 4.0 + ICON / 2.0, cy), Vec2::splat(ICON));
                    let on = d.visible(l.id);
                    paint_check(ui, check, if on { Check::On } else { Check::Off });
                    ui.painter().rect_filled(icon, 0.0, Color32::from_rgb(l.color[0], l.color[1], l.color[2]));
                    ui.painter().text(Pos2::new(icon.right() + 5.0, cy), Align2::LEFT_CENTER, &l.name, body_font.clone(), ui.visuals().text_color());
                    ui.painter().text(Pos2::new(rect.right() - 4.0, cy), Align2::RIGHT_CENTER, crate::drawing2::Draw2::count_text(l), body_font.clone(), MUTED);
                    if l.off_in_file && resp.hovered() {
                        resp.clone().on_hover_text(cad_2d::LAYER_OFF_TOOLTIP);
                    }
                    let pos = resp.interact_pointer_pos();
                    let on_check = pos.is_some_and(|p| check.expand(2.0).contains(p));
                    let mods = ui.input(|i| i.modifiers);
                    if resp.clicked() {
                        if on_check {
                            acts.push(LayerAct::Check { id: l.id, on: !on });
                        } else {
                            acts.push(LayerAct::Click { id: l.id, ctrl: mods.ctrl || mods.command, shift: mods.shift });
                        }
                    }
                    if resp.secondary_clicked() {
                        acts.push(LayerAct::Context { id: l.id, pos: pos.unwrap_or(rect.center()) });
                    }
                }
            },
        );
        for a in acts {
            self.layer_act(a);
        }
    }

    /// Properties panel; returns a clicked link.
    pub fn show_props(&mut self, ui: &mut Ui) -> Option<LinkTarget> {
        let mut link = None;
        match &self.props {
            Props::Welcome => {
                rich::heading(ui, strings::APP_NAME);
                for p in strings::WELCOME {
                    rich::paragraph(ui, p);
                    ui.add_space(4.0);
                }
            }
            Props::Rows { title, rows } => {
                if let Some(t) = title {
                    rich::heading(ui, t);
                    ui.add_space(2.0);
                }
                let font = TextStyle::Body.resolve(ui.style());
                let label_w = rows
                    .iter()
                    .filter(|r| !r.label.is_empty() && !r.value.is_empty())
                    .map(|r| ui.painter().layout_no_wrap(format!("{}:", r.label), font.clone(), Color32::WHITE).size().x)
                    .fold(0.0f32, f32::max)
                    .min(ui.available_width() * 0.5)
                    + 8.0;
                for row in rows {
                    if let Some(l) = prop_row(ui, row, label_w) {
                        link = Some(l);
                    }
                }
            }
        }
        link
    }

    /// The status bar: text, cursor position, progress and cancel.
    pub fn show_status(&mut self, ui: &mut Ui) {
        let temp_live = self.status.temp.as_ref().is_some_and(|(_, until)| std::time::Instant::now() < *until);
        if !temp_live {
            self.status.temp = None;
        }
        let text = match &self.status.temp {
            Some((t, _)) => t.clone(),
            None => self.status.model.clone(),
        };
        // a load, or the scene of another layout or background that is being built (the old one stays on the screen)
        let load = self.flow.busy;
        let busy = load || self.flow.scene_busy;
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if busy && ui.button("Zrušit").clicked() {
                    cancel = true;
                }
                if busy {
                    busy_bar(ui);
                }
                let w = 200.0;
                ui.allocate_ui_with_layout(Vec2::new(w, ui.available_height()), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_width(w);
                    ui.add(egui::Label::new(self.status.coord.clone()).truncate());
                });
                if !self.status.note.is_empty() {
                    ui.label(RichText::new(self.status.note.clone()).color(WARN)).on_hover_text(crate::strings::STATUS_REDUCED_DETAIL_TIP);
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(text).truncate());
                });
            });
        });
        if cancel {
            if load {
                self.cancel_load();
            } else {
                self.cancel_scene_rebuild();
            }
        }
        if temp_live {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        if busy {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    /// The start screen.
    pub fn show_empty(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        // the first model is fitted into the area this screen occupies
        let ppp = ui.ctx().pixels_per_point();
        self.view_px = ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32, f64::from(ppp));
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, strings::EMPTY_STATE, FontId::proportional(17.0), MUTED);
    }
}

/// The indeterminate progress bar (`QProgressBar` with range 0..0): a chunk that moves back and forth.
fn busy_bar(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(160.0, 14.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
    let t = ui.input(|i| i.time) as f32;
    let phase = (t * 0.8).fract();
    let tri = 1.0 - (phase * 2.0 - 1.0).abs();
    let w = rect.width() * 0.3;
    let x = rect.left() + (rect.width() - w) * tri;
    p.rect_filled(Rect::from_min_size(Pos2::new(x, rect.top() + 1.0), Vec2::new(w, rect.height() - 2.0)), 2.0, ui.visuals().selection.bg_fill);
}

/// One line of the properties panel; returns a clicked link.
fn prop_row(ui: &mut Ui, row: &PropRow, label_w: f32) -> Option<LinkTarget> {
    let mut link = None;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if !row.label.is_empty() {
            let label = if row.value.is_empty() { row.label.clone() } else { format!("{}:", row.label) };
            let mut t = RichText::new(label);
            if row.bold_label {
                t = t.strong();
            } else {
                t = t.color(MUTED);
            }
            if row.value.is_empty() {
                ui.label(t);
            } else {
                ui.allocate_ui_with_layout(Vec2::new(label_w, 0.0), Layout::top_down(Align::LEFT), |ui| {
                    ui.set_width(label_w);
                    ui.label(t);
                });
            }
        } else if !row.value.is_empty() {
            // continuation line: the value stays in the value column
            ui.add_space(label_w);
        }
        if !row.value.is_empty() {
            ui.vertical(|ui| {
                ui.set_max_width((ui.available_width()).max(40.0));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for s in &row.value {
                        match s {
                            Span::Text(t) => {
                                ui.label(t);
                            }
                            Span::Bold(t) => {
                                ui.label(RichText::new(t).strong());
                            }
                            Span::Muted(t) => {
                                ui.label(RichText::new(t).color(MUTED));
                            }
                            Span::Warn(t) => {
                                ui.label(RichText::new(t).color(WARN));
                            }
                            Span::Link(t, target) => {
                                if ui.link(t).clicked() {
                                    link = Some(*target);
                                }
                            }
                            Span::Swatch(c, name) => {
                                let (r, _) = ui.allocate_exact_size(Vec2::new(26.0, 12.0), Sense::hover());
                                ui.painter().rect_filled(r, 1.0, Color32::from_rgb(c[0], c[1], c[2]));
                                ui.label(format!(" {name}"));
                            }
                        }
                    }
                });
            });
        }
    });
    link
}
