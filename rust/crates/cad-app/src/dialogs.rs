//! Dialogs: message boxes, material assignment, about / help, 3D mouse settings and the Explorer thumbnail switch.
use cad_core::materials::{custom_name, Library, CUSTOM, DEFAULT_DENSITY};
use cad_types::NodeId;
use eframe::egui::{self, Align, Id, Layout, Modal, RichText};

use crate::app::{App, MsgKind};
use crate::rich;
use crate::spacemouse;
use crate::strings;

/// Density range of the spin box (`setRange(0.001, 30.0)`).
pub const DENSITY_RANGE: std::ops::RangeInclusive<f64> = 0.001..=30.0;

// ---------------------------------------------------------------------- material dialog

pub struct MaterialDlg {
    pub title: String,
    /// The parts the material goes to; empty = the default material of the model.
    pub leaves: Vec<NodeId>,
    /// 0 = "(bez materiálu)", 1..=N the library, N + 1 = "Vlastní hustota…".
    pub combo: usize,
    pub density: f64,
    pub show_default: bool,
    /// 0 = "(žádný – nepočítat hmotnost)", 1..=N the library.
    pub default_combo: usize,
    library: Library,
}

impl MaterialDlg {
    pub fn new(
        title: String,
        leaves: Vec<NodeId>,
        current: Option<(String, f64)>,
        show_default: bool,
        default: Option<(String, f64)>,
    ) -> MaterialDlg {
        let library = Library::builtin();
        let n = library.entries().len();
        let mut combo = 0;
        let mut density = DEFAULT_DENSITY;
        if let Some((name, d)) = &current {
            combo = library.entries().iter().position(|(n, _)| n == name).map(|i| i + 1).unwrap_or(n + 1);
            density = *d;
        }
        let default_combo = default
            .and_then(|(name, _)| library.entries().iter().position(|(n, _)| *n == name))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut dlg = MaterialDlg { title, leaves, combo, density, show_default, default_combo, library };
        dlg.changed();
        dlg
    }

    pub fn is_custom(&self) -> bool {
        self.combo == self.library.entries().len() + 1
    }

    /// `_changed`: a library entry sets its density.
    pub fn changed(&mut self) {
        if (1..=self.library.entries().len()).contains(&self.combo) {
            self.density = self.library.entries()[self.combo - 1].1;
        }
    }

    pub fn combo_text(&self, i: usize) -> String {
        match i {
            0 => "(bez materiálu)".to_string(),
            i if i <= self.library.entries().len() => {
                let (n, d) = &self.library.entries()[i - 1];
                Library::combo_label(n, *d)
            }
            _ => CUSTOM.to_string(),
        }
    }

    /// `result_material`: `None` for "no material".
    pub fn result_material(&self) -> Option<(String, f64)> {
        match self.combo {
            0 => None,
            i if i <= self.library.entries().len() => Some(self.library.entries()[i - 1].clone()),
            _ => Some((custom_name(self.density), self.density)),
        }
    }

    /// `result_default`.
    pub fn result_default(&self) -> Option<(String, f64)> {
        (self.show_default && self.default_combo > 0).then(|| self.library.entries()[self.default_combo - 1].clone())
    }

    pub fn entry_count(&self) -> usize {
        self.library.entries().len()
    }

    pub fn default_text(&self, i: usize) -> String {
        if i == 0 { "(žádný – nepočítat hmotnost)".to_string() } else { self.library.entries()[i - 1].0.clone() }
    }
}

/// What the user did in a dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Ok,
    Cancel,
}

fn buttons(ui: &mut egui::Ui, ok: &str, cancel: Option<&str>) -> Outcome {
    let mut out = Outcome::Open;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.button(ok).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.any()) {
            out = Outcome::Ok;
        }
        if let Some(c) = cancel {
            if ui.button(c).clicked() {
                out = Outcome::Cancel;
            }
        }
    });
    out
}

impl App {
    /// Message boxes (`QMessageBox`): the first one waits for OK.
    pub fn show_messages(&mut self, ctx: &egui::Context) {
        let Some(msg) = self.msgs.front() else { return };
        let (kind, text) = (msg.kind, msg.text.clone());
        let mut close = false;
        let r = Modal::new(Id::unique("message-box")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.horizontal_top(|ui| {
                let (sym, col) = match kind {
                    MsgKind::Info => ("i", egui::Color32::from_rgb(60, 130, 220)),
                    MsgKind::Warning => ("!", egui::Color32::from_rgb(230, 160, 20)),
                    MsgKind::Critical => ("×", egui::Color32::from_rgb(210, 50, 50)),
                };
                ui.label(RichText::new(sym).size(26.0).strong().color(col));
                ui.vertical(|ui| {
                    ui.strong(strings::APP_NAME);
                    ui.add(egui::Label::new(text.clone()).wrap());
                });
            });
            ui.add_space(8.0);
            if buttons(ui, "OK", None) == Outcome::Ok {
                close = true;
            }
        });
        if close || r.should_close() {
            self.msgs.pop_front();
        }
    }

    /// The material dialog (`MaterialDialog`).
    pub fn show_material_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dlg.material.take() else { return };
        let mut outcome = Outcome::Open;
        let r = Modal::new(Id::unique("material-dialog")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.strong(dlg.title.clone());
            ui.add_space(6.0);
            egui::Grid::new("material-grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Materiál:");
                let before = dlg.combo;
                egui::ComboBox::from_id_salt("material-combo")
                    .width(300.0)
                    .selected_text(dlg.combo_text(dlg.combo))
                    .show_ui(ui, |ui| {
                        for i in 0..=dlg.entry_count() + 1 {
                            let t = dlg.combo_text(i);
                            ui.selectable_value(&mut dlg.combo, i, t);
                        }
                    });
                if dlg.combo != before {
                    dlg.changed();
                }
                ui.end_row();
                ui.label("Hustota:");
                let custom = dlg.is_custom();
                ui.add_enabled(
                    custom,
                    egui::DragValue::new(&mut dlg.density)
                        .range(DENSITY_RANGE)
                        .speed(0.01)
                        .suffix(" g/cm³")
                        // a decimal comma, and both separators are accepted
                        .custom_formatter(|v, _| format!("{v:.3}").replace('.', ","))
                        .custom_parser(|s| s.trim().replace(',', ".").parse().ok()),
                );
                ui.end_row();
                if dlg.show_default {
                    ui.label("Díly bez materiálu:");
                    egui::ComboBox::from_id_salt("material-default")
                        .width(300.0)
                        .selected_text(dlg.default_text(dlg.default_combo))
                        .show_ui(ui, |ui| {
                            for i in 0..=dlg.entry_count() {
                                let t = dlg.default_text(i);
                                ui.selectable_value(&mut dlg.default_combo, i, t);
                            }
                        });
                    ui.end_row();
                }
            });
            if dlg.show_default {
                ui.add_space(4.0);
                ui.add(egui::Label::new("Výchozí materiál se použije pro všechny díly, kterým nebyl přiřazen žádný.").wrap());
            }
            ui.add_space(8.0);
            outcome = buttons(ui, "OK", Some("Zrušit"));
        });
        if r.should_close() && outcome == Outcome::Open {
            outcome = Outcome::Cancel;
        }
        match outcome {
            Outcome::Open => self.dlg.material = Some(dlg),
            Outcome::Cancel => {}
            Outcome::Ok => self.apply_material(dlg),
        }
    }

    /// The about box / help (`about`).
    pub fn show_about(&mut self, ctx: &egui::Context) {
        if !self.dlg.about {
            return;
        }
        let mut close = false;
        let r = Modal::new(Id::unique("about")).show(ctx, |ui| {
            ui.set_max_width(640.0);
            rich::heading(ui, &format!("{} {}", strings::APP_NAME, strings::VERSION));
            ui.add_space(4.0);
            for line in strings::ABOUT_DESCRIPTION.split('\n') {
                ui.label(line);
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for p in strings::ABOUT {
                    rich::paragraph(ui, p);
                    ui.add_space(6.0);
                }
            });
            if buttons(ui, "OK", None) == Outcome::Ok {
                close = true;
            }
        });
        if close || r.should_close() {
            self.dlg.about = false;
        }
    }

    /// Settings of the 3D mouse (`SpaceMouseDialog`).
    pub fn show_spacemouse_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dlg.spacemouse.take() else { return };
        let levels = self.sm.as_ref().map(|s| s.levels()).unwrap_or([0.0; 6]);
        let last_button = self.sm.as_ref().and_then(|s| s.last_button());
        let mut outcome = Outcome::Open;
        let r = Modal::new(Id::unique("spacemouse-dialog")).show(ctx, |ui| {
            ui.set_min_width(460.0);
            ui.strong(spacemouse::TITLE);
            ui.add_space(4.0);
            ui.add(egui::Label::new(spacemouse::devices_text(&dlg.devices)).wrap());
            ui.checkbox(&mut dlg.cfg.enabled, "Ovládat pohled 3D myší");
            ui.horizontal(|ui| {
                ui.label("Rychlost:");
                ui.add(egui::Slider::new(&mut dlg.speed_slider, 0..=100).show_value(false));
                ui.label(spacemouse::speed_label(dlg.speed_slider));
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut dlg.cfg.rotate, "Otáčení");
                ui.checkbox(&mut dlg.cfg.pan, "Posun a zoom");
                ui.checkbox(&mut dlg.cfg.dominant, "Jen převažující pohyb (dominantní osa)");
            });
            ui.add_space(4.0);
            ui.group(|ui| {
                ui.label("Osy – vychýlení čepičky a obrácení směru");
                egui::Grid::new("spacemouse-axes").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
                    for (i, name) in spacemouse::AXES.iter().enumerate() {
                        ui.label(*name);
                        axis_bar(ui, levels[i]);
                        ui.checkbox(&mut dlg.cfg.invert[i], "obrátit");
                        ui.end_row();
                    }
                });
            });
            ui.add_space(4.0);
            ui.label(match last_button {
                Some(n) => format!("Tlačítko: {n}"),
                None => "Tlačítko: –".to_string(),
            });
            ui.add(egui::Label::new(spacemouse::INFO).wrap());
            ui.add_space(8.0);
            outcome = buttons(ui, "OK", Some("Zrušit"));
        });
        if r.should_close() && outcome == Outcome::Open {
            outcome = Outcome::Cancel;
        }
        match outcome {
            Outcome::Open => {
                self.dlg.spacemouse = Some(dlg);
                // the live bars
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Outcome::Cancel => {}
            Outcome::Ok => {
                let mut cfg = dlg.cfg.clone();
                cfg.speed = spacemouse::speed_from_slider(dlg.speed_slider);
                self.settings.spacemouse = cfg;
                self.save_settings();
                if !self.settings.spacemouse.enabled {
                    if let Some(s) = self.sm.as_mut() {
                        s.reset();
                    }
                }
            }
        }
    }
}

/// Centred bar of an axis: -1 left, +1 right.
fn axis_bar(ui: &mut egui::Ui, level: f64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(160.0, 12.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    let mid = rect.center().x;
    let x = mid + (level.clamp(-1.0, 1.0) as f32) * rect.width() / 2.0;
    let fill = egui::Rect::from_x_y_ranges(x.min(mid)..=x.max(mid), rect.y_range());
    p.rect_filled(fill, 2.0, ui.visuals().selection.bg_fill);
    p.line_segment([egui::pos2(mid, rect.top()), egui::pos2(mid, rect.bottom())], egui::Stroke::new(1.0, ui.visuals().weak_text_color()));
}

pub struct SpaceMouseDlg {
    pub cfg: cad_core::settings::SpaceMouse,
    pub speed_slider: i32,
    /// `(vendor id, product id)` of the connected devices.
    pub devices: Vec<(u16, u16)>,
}

impl SpaceMouseDlg {
    pub fn new(cfg: &cad_core::settings::SpaceMouse) -> SpaceMouseDlg {
        #[cfg(windows)]
        let devices = crate::spacemouse_win::devices();
        #[cfg(not(windows))]
        let devices = Vec::new();
        SpaceMouseDlg { cfg: cfg.clone(), speed_slider: spacemouse::slider_from_speed(cfg.speed), devices }
    }
}

// ---------------------------------------------------------------------- Explorer thumbnails

pub struct ThumbDlg {
    /// Check box state, enabled flag (false when installed for all users) per format.
    pub checks: Vec<(bool, bool)>,
}

impl ThumbDlg {
    #[cfg(windows)]
    pub fn new() -> ThumbDlg {
        use crate::thumbsettings::{is_on, machine_wide, FORMATS};
        let checks = FORMATS
            .iter()
            .map(|(_, exts)| (exts.iter().any(|e| is_on(e)), !exts.iter().any(|e| machine_wide(e))))
            .collect();
        ThumbDlg { checks }
    }

    #[cfg(not(windows))]
    pub fn new() -> ThumbDlg {
        ThumbDlg { checks: Vec::new() }
    }
}

/// The check box of one format of the thumbnail dialog, with its tips: the warning of DWG, and "installed for all
/// users" on a box that is disabled for that reason. A disabled widget gets no ordinary hover, so its tip has its own
/// call (`on_disabled_hover_text`).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn format_check(ui: &mut egui::Ui, name: &str, exts: &[&str], checked: &mut bool, enabled: bool) -> egui::Response {
    use crate::thumbsettings as ts;
    let mut resp = ui.add_enabled(enabled, egui::Checkbox::new(checked, ts::format_label(name, exts)));
    if name == "DWG" {
        resp = resp.on_hover_text(ts::DWG_TIP);
    }
    if !enabled {
        resp = resp.on_disabled_hover_text(ts::MACHINE_TIP);
    }
    resp
}

impl App {
    #[cfg(windows)]
    pub fn show_thumb_dialog(&mut self, ctx: &egui::Context) {
        use crate::thumbsettings as ts;
        let Some(mut dlg) = self.dlg.thumbs.take() else { return };
        let mut outcome = Outcome::Open;
        let r = Modal::new(Id::unique("thumb-dialog")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.strong(ts::TITLE);
            ui.add_space(4.0);
            ui.add(egui::Label::new(ts::INTRO).wrap());
            ui.group(|ui| {
                for (i, (name, exts)) in ts::FORMATS.iter().enumerate() {
                    let (checked, enabled) = &mut dlg.checks[i];
                    format_check(ui, name, exts, checked, *enabled);
                }
            });
            ui.add_space(4.0);
            ui.add(egui::Label::new(ts::INFO).wrap());
            if ui.button(ts::CLEAN_BUTTON).clicked() {
                ts::open_disk_cleanup();
            }
            ui.add_space(8.0);
            outcome = buttons(ui, "OK", Some("Zrušit"));
        });
        if r.should_close() && outcome == Outcome::Open {
            outcome = Outcome::Cancel;
        }
        match outcome {
            Outcome::Open => self.dlg.thumbs = Some(dlg),
            Outcome::Cancel => {}
            Outcome::Ok => {
                let want_any = dlg.checks.iter().any(|(c, e)| *c && *e);
                if want_any {
                    if let Err(problem) = ts::ensure_clsid() {
                        // nothing is registered
                        self.dlg.thumbs = Some(dlg);
                        self.warn(problem);
                        return;
                    }
                }
                for (i, (_, exts)) in ts::FORMATS.iter().enumerate() {
                    let (checked, enabled) = dlg.checks[i];
                    if enabled {
                        for e in exts.iter() {
                            let _ = ts::set_on(e, checked);
                        }
                    }
                }
                ts::notify_shell();
            }
        }
    }

    #[cfg(not(windows))]
    pub fn show_thumb_dialog(&mut self, _ctx: &egui::Context) {
        self.dlg.thumbs = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text of everything painted in a frame (tooltips are painted as text too).
    fn painted_text(shapes: &[egui::epaint::ClippedShape]) -> String {
        fn walk(s: &egui::epaint::Shape, out: &mut String) {
            match s {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        shapes.iter().for_each(|c| walk(&c.shape, &mut out));
        out
    }

    /// Runs frames of a small UI with the pointer resting on the widget; returns what was painted in the last one.
    fn hover_frames(add: &mut dyn FnMut(&mut egui::Ui) -> egui::Response) -> String {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| {
            s.interaction.tooltip_delay = 0.0;
            s.interaction.show_tooltips_only_when_still = false;
            s.animation_time = 0.0;
        });
        let mut rect = egui::Rect::NOTHING;
        let mut text = String::new();
        for k in 0..6 {
            let mut raw = egui::RawInput::default();
            raw.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 300.0)));
            raw.time = Some(0.1 * f64::from(k));
            if k > 0 {
                raw.events.push(egui::Event::PointerMoved(rect.center()));
            }
            let mut out = ctx.run_ui(raw, |ui| rect = add(ui).rect);
            // nothing is drawn on a screen here, so the texture changes are not needed
            out.textures_delta.clear();
            text = painted_text(&out.shapes);
        }
        text
    }

    #[test]
    fn the_installed_for_all_users_tip_shows_on_the_disabled_check_box() {
        use crate::thumbsettings as ts;
        let mut checked = true;
        // the box is disabled because the installer set the handler for all users: the tip says why
        let shown = hover_frames(&mut |ui| format_check(ui, "STEP", &[".step", ".stp"], &mut checked, false));
        assert!(shown.contains(ts::MACHINE_TIP), "no tooltip on the disabled box:\n{shown}");
        // an enabled box has no such tip
        let mut checked = true;
        let shown = hover_frames(&mut |ui| format_check(ui, "STEP", &[".step", ".stp"], &mut checked, true));
        assert!(!shown.contains(ts::MACHINE_TIP), "{shown}");
        // DWG keeps its own warning
        let mut checked = false;
        let shown = hover_frames(&mut |ui| format_check(ui, "DWG", &[".dwg"], &mut checked, true));
        assert!(shown.contains(ts::DWG_TIP), "{shown}");
        // why it was never shown: `on_hover_text` on a widget that is disabled shows nothing at all
        let mut checked = true;
        let old = hover_frames(&mut |ui| {
            ui.add_enabled(false, egui::Checkbox::new(&mut checked, "STEP")).on_hover_text(ts::MACHINE_TIP)
        });
        assert!(!old.contains(ts::MACHINE_TIP), "{old}");
    }

    #[test]
    fn dialog_starts_from_the_current_material() {
        let d = MaterialDlg::new("Materiál – 1 dílů".into(), vec![3], Some(("Hliník (slitiny)".into(), 2.70)), true, None);
        assert_eq!(d.combo_text(d.combo), "Hliník (slitiny)  –  2,7 g/cm³");
        assert_eq!(d.density, 2.70);
        assert!(!d.is_custom());
        assert_eq!(d.result_material(), Some(("Hliník (slitiny)".to_string(), 2.70)));
        assert_eq!(d.result_default(), None);
    }

    #[test]
    fn unknown_current_material_becomes_custom() {
        let d = MaterialDlg::new("t".into(), vec![1], Some(("Vlastní (7,5 g/cm³)".into(), 7.5)), false, None);
        assert!(d.is_custom());
        assert_eq!(d.density, 7.5);
        assert_eq!(d.result_material(), Some(("Vlastní (7,5 g/cm³)".to_string(), 7.5)));
        // no default combo in this dialog
        assert_eq!(d.result_default(), None);
    }

    #[test]
    fn no_material_and_default_choice() {
        let mut d = MaterialDlg::new("t".into(), vec![], None, true, Some(("Ocel (konstrukční)".into(), 7.85)));
        assert_eq!(d.combo, 0);
        assert_eq!(d.result_material(), None);
        assert_eq!(d.default_combo, 1);
        assert_eq!(d.result_default(), Some(("Ocel (konstrukční)".to_string(), 7.85)));
        d.default_combo = 0;
        assert_eq!(d.result_default(), None);
        assert_eq!(d.default_text(0), "(žádný – nepočítat hmotnost)");
        // picking a library entry fills the density
        d.combo = 4;
        d.changed();
        assert_eq!(d.density, 2.70);
    }

    #[test]
    fn custom_density_is_named_with_the_value() {
        let mut d = MaterialDlg::new("t".into(), vec![1], None, false, None);
        d.combo = d.entry_count() + 1;
        d.density = 1.25;
        assert_eq!(d.result_material(), Some(("Vlastní (1,25 g/cm³)".to_string(), 1.25)));
        assert_eq!(d.combo_text(0), "(bez materiálu)");
        assert_eq!(d.combo_text(d.combo), "Vlastní hustota…");
    }
}
