//! The toolbar "Nástroje" (`_build_actions`). The view, display and section controls exist only for 3D, the layout and
//! background controls only for drawings.
use cad_render::StdView;
use eframe::egui::{self, Ui};

use crate::app::{Action, App, MeasureKind, Mode};
use crate::strings;
use crate::util::base_name;

/// Tooltip of a command: its text and the shortcut in brackets.
pub fn tip(text: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(s) => format!("{text}  [{s}]"),
        None => text.to_string(),
    }
}

/// `(button text, view, key, Onshape key)` of the view buttons.
pub const VIEWS: [(&str, StdView, &str, &str); 7] = [
    ("Iso", StdView::Iso, "0", "Shift+7"),
    ("Přední", StdView::Front, "1", "Shift+1"),
    ("Zadní", StdView::Back, "2", "Shift+2"),
    ("Horní", StdView::Top, "3", "Shift+5"),
    ("Spodní", StdView::Bottom, "4", "Shift+6"),
    ("Levý", StdView::Left, "5", "Shift+3"),
    ("Pravý", StdView::Right, "6", "Shift+4"),
];

/// Tooltip of a view button: `Pohled přední  [1 nebo Shift+1]`.
pub fn view_tip(text: &str, key: &str, onshape: &str) -> String {
    format!("Pohled {}  [{key} nebo {onshape}]", text.to_lowercase())
}

pub const MEASURE_TIP: &str = "3D: klikej na vrcholy, hrany a plochy – délky, Ø / R, plochy, vzdálenosti s ΔX/ΔY/ΔZ, úhly. 2D: vzdálenost dvou bodů";

impl App {
    fn button(&mut self, ui: &mut Ui, text: &str, tip_text: &str, action: Action) {
        if ui.button(text).on_hover_text(tip_text).clicked() {
            self.queue.push(action);
        }
    }

    fn toggle(&mut self, ui: &mut Ui, text: &str, on: bool, tip_text: &str, action: Action) {
        if ui.selectable_label(on, text).on_hover_text(tip_text).clicked() {
            self.queue.push(action);
        }
    }

    pub fn show_toolbar(&mut self, ui: &mut Ui) {
        let three = self.mode != Mode::TwoD;
        ui.spacing_mut().button_padding = egui::vec2(5.0, 3.0);
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.horizontal_wrapped(|ui| {
            // Otevřít with the list of recent files
            self.button(ui, "Otevřít", &tip("Otevřít STEP / IGES / STL / DXF / DWG", Some("Ctrl+O")), Action::Open);
            let arrow = ui.menu_button("    ", |ui| self.recent_menu(ui));
            if arrow.inner.is_none() {
                // closed: the next opening looks at the files again
                self.recent_cache = None;
            }
            // the arrow is drawn: the default fonts have no glyph for it
            let c = arrow.response.rect.center();
            ui.painter().add(egui::Shape::convex_polygon(
                vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(4.0, -2.0), c + egui::vec2(0.0, 3.0)],
                ui.visuals().text_color(),
                egui::Stroke::NONE,
            ));
            self.button(ui, "Znovu načíst", &tip("Znovu načíst", Some("F5")), Action::Reload);
            ui.separator();
            self.button(ui, "Vše", &tip("Zobrazit celý model / výkres", Some("F")), Action::Fit);
            self.button(ui, "Na výběr", &tip("Přiblížit na vybrané", Some("Z")), Action::ZoomSel);
            if three {
                for (text, view, key, onshape) in VIEWS {
                    self.button(ui, text, &view_tip(text, key, onshape), Action::View(view));
                }
                ui.separator();
                let (edges, wire, persp) = (self.opts.edges, self.opts.wire, self.opts.persp);
                self.toggle(ui, "Hrany", edges, &tip("Zobrazit hrany", Some("E")), Action::Edges);
                self.toggle(ui, "Drátově", wire, &tip("Drátový model", Some("W")), Action::Wire);
                self.toggle(ui, "Perspektiva", persp, &tip("Perspektivní promítání", Some("P")), Action::Persp);
                ui.separator();
                self.section_controls(ui);
                let mut scale = self.settings.scale_bar;
                if ui
                    .checkbox(&mut scale, "měřítko")
                    .on_hover_text("Měřítko v levém dolním rohu (jen v pravoúhlém promítání)")
                    .changed()
                {
                    self.on_scale_bar(scale);
                }
            } else {
                self.layout_controls(ui);
            }
            ui.separator();
            let (dist, angle) = (self.measure == Some(MeasureKind::Dist), self.measure == Some(MeasureKind::Angle));
            self.toggle(ui, "Měřit", dist, &tip(MEASURE_TIP, Some("M")), Action::Measure);
            self.toggle(
                ui,
                "Úhel",
                angle,
                &tip("Měření úhlu: klikni na konec ramene, vrchol a konec druhého ramene", Some("U")),
                Action::Angle,
            );
            if three {
                self.button(
                    ui,
                    "Hmotnost",
                    &tip("Hmotnost, objem a těžiště vybraných dílů (bez výběru celého modelu)", Some("G")),
                    Action::Mass,
                );
                self.button(
                    ui,
                    "Kolize",
                    &tip("Najít kolize (průniky) mezi díly – vybranými, jinak všemi viditelnými", Some("K")),
                    Action::Collide,
                );
            }
            self.button(ui, "Snímek", &tip("Uložit obrázek pohledu (PNG)", Some("Ctrl+S")), Action::Shot);
            self.button(ui, "Nápověda", &tip("Ovládání a klávesové zkratky", Some("F1")), Action::About);
        });
    }

    /// `Řez: [vyp. | X | Y | Z]  ---o---  [otočit]`.
    fn section_controls(&mut self, ui: &mut Ui) {
        ui.label(" Řez: ");
        let names = ["vyp.", "X", "Y", "Z"];
        let before = self.opts.section_axis;
        egui::ComboBox::from_id_salt("section-axis")
            .width(54.0)
            .selected_text(names[self.opts.section_axis.min(3)])
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.opts.section_axis, i, *n);
                }
            })
            .response
            .on_hover_text("Řezová rovina kolmá na osu");
        let on = self.opts.section_axis > 0 && self.model.as_ref().is_some_and(|m| m.model_box.is_some());
        let mut changed = before != self.opts.section_axis;
        ui.scope(|ui| {
            ui.spacing_mut().slider_width = 140.0;
            changed |= ui
                .add_enabled(on, egui::Slider::new(&mut self.opts.section_slider, 0.0..=1000.0).show_value(false))
                .changed();
        });
        changed |= ui.add_enabled(on, egui::Checkbox::new(&mut self.opts.section_flip, "otočit")).changed();
        if changed {
            self.update_section(true);
        }
    }

    /// `Layout: [..]  [bílé pozadí]  [pravítka]`.
    fn layout_controls(&mut self, ui: &mut Ui) {
        ui.label(" Layout: ");
        let mut chosen: Option<String> = None;
        if let Some(d) = &self.draw {
            let current = d
                .drawing
                .layouts()
                .iter()
                .find(|l| l.name == d.layout)
                .map(|l| format!("{}  ({})", l.name, l.drawable))
                .unwrap_or_else(|| d.layout.clone());
            egui::ComboBox::from_id_salt("layout-combo")
                .selected_text(current)
                .width(160.0)
                .show_ui(ui, |ui| {
                    for l in d.drawing.layouts() {
                        let text = format!("{}  ({})", l.name, l.drawable);
                        if ui.selectable_label(l.name == d.layout, text).clicked() {
                            chosen = Some(l.name.clone());
                        }
                    }
                })
                .response
                .on_hover_text("Prostor modelu / výkresové listy (layouty)");
        }
        if let Some(name) = chosen {
            self.on_layout_changed(name);
        }
        let mut light = self.settings.light_bg;
        if ui.checkbox(&mut light, "bílé pozadí").on_hover_text("Bílé pozadí i v prostoru modelu").changed() {
            self.on_light_bg(light);
        }
        let mut rulers = self.settings.rulers;
        if ui.checkbox(&mut rulers, "pravítka").on_hover_text("Pravítka se souřadnicemi výkresu nahoře a vlevo").changed() {
            self.on_rulers(rulers);
        }
    }

    /// The recent files for the open menu. The menu is drawn on every frame while it is open, so the worker is started
    /// and the files are looked at once, when it opens (`recent_cache` is emptied while it is closed).
    pub(crate) fn menu_recent(&mut self) -> Vec<String> {
        if self.recent_cache.is_none() {
            self.prestart_worker();
            self.recent_cache = Some(self.settings.existing_recent());
        }
        self.recent_cache.clone().unwrap_or_default()
    }

    /// The drop-down of "Otevřít": recent files, cache, thumbnails, 3D mouse, the Google Drive link.
    fn recent_menu(&mut self, ui: &mut Ui) {
        let recent = self.menu_recent();
        if recent.is_empty() {
            ui.add_enabled(false, egui::Button::new("(žádné nedávné soubory)").frame(false));
        }
        for p in recent {
            let path = std::path::PathBuf::from(&p);
            if ui.add(egui::Button::new(base_name(&path)).frame(false)).on_hover_text(&p).clicked() {
                self.queue.push(Action::OpenPath(path));
                ui.close();
            }
        }
        ui.separator();
        if ui.add_enabled(!self.flow.active(), egui::Button::new("Vymazat mezipaměť načtených modelů").frame(false)).clicked() {
            self.queue.push(Action::ClearCache);
            ui.close();
        }
        if cfg!(windows) && !self.portable && ui.add(egui::Button::new("Náhledy v Průzkumníku…").frame(false)).clicked() {
            self.queue.push(Action::ThumbSettings);
            ui.close();
        }
        if self.sm.is_some() && ui.add(egui::Button::new("3D myš (3Dconnexion)…").frame(false)).clicked() {
            self.queue.push(Action::SpaceMouseSettings);
            ui.close();
        }
        // Google Drive: the entries of the help page of the launcher; each one does something only when it is clicked
        ui.separator();
        for (text, tip_text, action) in [
            (strings::MENU_REGISTER_LINK, strings::TIP_REGISTER_LINK, Action::RegisterLink),
            (strings::MENU_UNREGISTER_LINK, strings::TIP_UNREGISTER_LINK, Action::UnregisterLink),
            (strings::MENU_DRIVE_SIGN_OUT, strings::TIP_DRIVE_SIGN_OUT, Action::DriveSignOut),
        ] {
            if ui.add(egui::Button::new(text).frame(false)).on_hover_text(tip_text).clicked() {
                self.queue.push(action);
                ui.close();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltips_match_the_python_texts() {
        assert_eq!(tip("Zobrazit hrany", Some("E")), "Zobrazit hrany  [E]");
        assert_eq!(tip("Znovu načíst", None), "Znovu načíst");
        assert_eq!(view_tip("Přední", "1", "Shift+1"), "Pohled přední  [1 nebo Shift+1]");
        assert_eq!(view_tip("Iso", "0", "Shift+7"), "Pohled iso  [0 nebo Shift+7]");
    }

    #[test]
    fn view_buttons_cover_every_standard_view_once() {
        let mut seen: Vec<StdView> = VIEWS.iter().map(|v| v.1).collect();
        seen.sort_by_key(|v| v.key());
        seen.dedup();
        assert_eq!(seen.len(), 7);
        // the keys agree with the keyboard table
        for (_, view, key, onshape) in VIEWS {
            let digit: u8 = key.parse().unwrap();
            assert_eq!(crate::keys::view_of_digit(digit, false), Some(view));
            let shifted: u8 = onshape.trim_start_matches("Shift+").parse().unwrap();
            assert_eq!(crate::keys::view_of_digit(shifted, true), Some(view));
        }
    }
}
