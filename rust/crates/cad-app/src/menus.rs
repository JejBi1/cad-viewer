//! Context menus (`show_context_menu`): the 3D menu (view and tree), the measure menu and the 2D layer menu.
use eframe::egui::{self, Area, Frame, Id, Order, Pos2};

use crate::app::{Action, App, MeasureKind, Mode};

pub struct ContextMenu {
    pub pos: Pos2,
    /// Set after the first frame, so the click that opened the menu does not close it.
    pub opened: bool,
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Entry { text: &'static str, action: Action, enabled: bool },
    Separator,
}

fn entry(text: &'static str, action: Action, enabled: bool) -> Item {
    Item::Entry { text, action, enabled }
}

/// The entries of the menu for the current state.
pub fn items(mode: Mode, measure: Option<MeasureKind>, has_selection: bool, has_selected_layers: bool, has_model: bool) -> Vec<Item> {
    if mode == Mode::ThreeD && measure == Some(MeasureKind::Dist) {
        return vec![
            entry("Ponechat měření a měřit dál  (Enter)", Action::PinMeasure, true),
            entry("Kopírovat výsledek měření", Action::CopyMeasure, true),
            entry("Vyčistit výběr", Action::ClearPicked, true),
            entry("Smazat vše a ukončit měření  (Esc)", Action::EndMeasure, true),
        ];
    }
    if mode == Mode::TwoD {
        return vec![
            entry("Skrýt hladinu", Action::Hide, has_selected_layers),
            entry("Jen tato hladina", Action::Isolate, has_selected_layers),
            entry("Zobrazit všechny hladiny", Action::ShowAll, true),
            Item::Separator,
            entry("Na výběr", Action::ZoomSel, true),
            entry("Vše", Action::Fit, true),
        ];
    }
    let _ = has_model;
    let mut v = vec![
        entry("Na výběr", Action::ZoomSel, has_selection),
        entry("Skrýt", Action::Hide, has_selection),
        entry("Jen vybrané", Action::Isolate, has_selection),
        entry("Průhledné", Action::Transp, has_selection),
        entry("Zobrazit vše", Action::ShowAll, true),
        Item::Separator,
        entry("Materiál…", Action::Material, true),
        entry("Hmotnost", Action::Mass, true),
        entry("Kolize", Action::Collide, true),
        Item::Separator,
    ];
    if has_selection {
        v.push(entry("Kopírovat název", Action::CopyNames, true));
    }
    v.extend([
        entry("Rozbalit vše", Action::Expand, true),
        entry("Sbalit vše", Action::Collapse, true),
        Item::Separator,
        entry("Vše", Action::Fit, true),
    ]);
    v
}

impl App {
    /// A modal dialog or message box is open (Qt's `activeModalWidget`).
    pub fn modal_open(&self) -> bool {
        !self.msgs.is_empty() || self.dlg.material.is_some() || self.dlg.spacemouse.is_some() || self.dlg.thumbs.is_some() || self.dlg.about
    }

    pub fn close_context_menu(&mut self) {
        self.dlg.context = None;
    }

    pub fn open_context_menu(&mut self, pos: Pos2) {
        self.dlg.context = Some(ContextMenu { pos, opened: false });
    }

    fn menu_items(&self) -> Vec<Item> {
        let has_sel = self.model.as_ref().is_some_and(|m| !m.selected.is_empty());
        let has_layers = self.draw.as_ref().is_some_and(|d| !d.selected_layers.is_empty());
        items(self.mode, self.measure, has_sel, has_layers, self.model.is_some())
    }

    /// Shows the open context menu; an entry queues its action.
    pub fn show_context_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = &self.dlg.context else { return };
        let (pos, opened) = (menu.pos, menu.opened);
        let items = self.menu_items();
        let mut chosen: Option<Action> = None;
        let resp = Area::new(Id::unique("context-menu"))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_min_width(190.0);
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                        for it in &items {
                            match it {
                                Item::Separator => {
                                    ui.separator();
                                }
                                Item::Entry { text, action, enabled } => {
                                    if ui.add_enabled(*enabled, egui::Button::new(*text).frame(false)).clicked() {
                                        chosen = Some(action.clone());
                                    }
                                }
                            }
                        }
                    });
                });
            });
        if let Some(a) = chosen {
            self.dlg.context = None;
            self.queue.push(a);
            return;
        }
        let rect = resp.response.rect;
        let pressed_outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p)));
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if opened && (pressed_outside || escape) {
            self.dlg.context = None;
            if escape {
                // Esc closes the menu only
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            }
        } else if let Some(m) = self.dlg.context.as_mut() {
            m.opened = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[Item]) -> Vec<&'static str> {
        v.iter().filter_map(|i| if let Item::Entry { text, .. } = i { Some(*text) } else { None }).collect()
    }

    #[test]
    fn menu_3d_matches_the_python_menu() {
        let v = items(Mode::ThreeD, None, true, false, true);
        assert_eq!(
            texts(&v),
            vec![
                "Na výběr", "Skrýt", "Jen vybrané", "Průhledné", "Zobrazit vše", "Materiál…", "Hmotnost", "Kolize",
                "Kopírovat název", "Rozbalit vše", "Sbalit vše", "Vše"
            ]
        );
        // without a selection the selection commands are disabled and "Kopírovat název" is missing
        let v = items(Mode::ThreeD, None, false, false, true);
        assert!(!texts(&v).contains(&"Kopírovat název"));
        for i in &v {
            if let Item::Entry { text: "Skrýt" | "Na výběr" | "Jen vybrané" | "Průhledné", enabled, .. } = i {
                assert!(!enabled);
            }
        }
    }

    #[test]
    fn measure_menu() {
        let v = items(Mode::ThreeD, Some(MeasureKind::Dist), false, false, true);
        assert_eq!(
            texts(&v),
            vec![
                "Ponechat měření a měřit dál  (Enter)",
                "Kopírovat výsledek měření",
                "Vyčistit výběr",
                "Smazat vše a ukončit měření  (Esc)"
            ]
        );
        // the angle tool has the ordinary menu
        let v = items(Mode::ThreeD, Some(MeasureKind::Angle), false, false, true);
        assert!(texts(&v).contains(&"Materiál…"));
    }

    #[test]
    fn menu_2d() {
        let v = items(Mode::TwoD, None, false, true, false);
        assert_eq!(
            texts(&v),
            vec!["Skrýt hladinu", "Jen tato hladina", "Zobrazit všechny hladiny", "Na výběr", "Vše"]
        );
        let v = items(Mode::TwoD, None, false, false, false);
        assert!(matches!(&v[0], Item::Entry { enabled: false, .. }));
        assert!(matches!(&v[2], Item::Entry { enabled: true, .. }));
    }
}
