//! The interference (collision) check and its dialog (`collisions.py`, `run_collisions`).
//!
//! The check runs on a thread that owns the document while it works (a `Document` can be moved but not shared); the
//! model's functions that need the document wait until it is handed back with the result.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cad_core::fmt::fmt_num;
use cad_render::colors::INTERFERENCE_COLOR;
use cad_render::{OverlayMesh, Overlays};
use cad_types::{Interference, NodeId};
use eframe::egui::{self, Align, Layout};
use occt_bridge::{is_touch, Document, InterferenceReport};

use crate::app::App;
use crate::flow::Event;
use crate::props::Props;
use crate::util::spawn_big;

/// Statistics shown while the check runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub found: usize,
}

pub enum CollisionEvent {
    /// The check ended (or was stopped); the document comes back.
    Done { model_generation: u64, run: u64, doc: Document, result: Result<InterferenceReport, String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Collision,
    Touch,
}

impl Kind {
    pub fn text(self) -> &'static str {
        match self {
            Kind::Collision => "kolize",
            Kind::Touch => "dotyk",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResultRow {
    pub a: NodeId,
    pub b: NodeId,
    pub kind: Kind,
    pub volume: f64,
}

pub struct CollisionDlg {
    /// "vybrané díly" or "všechny viditelné díly".
    pub scope: String,
    pub parts: Vec<NodeId>,
    pub touch: bool,
    pub running: bool,
    pub stop: Arc<AtomicBool>,
    pub progress: Arc<Mutex<Option<Progress>>>,
    pub status: String,
    /// Part of the final status that is bold: the number of collisions.
    pub status_bold: Option<String>,
    pub status_tail: String,
    pub results: Vec<ResultRow>,
    pub selected: Option<usize>,
    pub t0: Instant,
    /// Number of the run (counted over all dialogs, `Flow::collision_runs`), so the answer of an older run, also one
    /// of a dialog that was replaced, is never taken for this one.
    pub run: u64,
    /// "Spustit" was pressed while an earlier check (of the dialog this one replaced) still holds the document: the
    /// check starts when the document is back.
    pub queued: bool,
}

impl CollisionDlg {
    pub fn new(scope: &str, parts: Vec<NodeId>) -> CollisionDlg {
        CollisionDlg {
            scope: scope.to_string(),
            parts,
            touch: false,
            running: false,
            stop: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Mutex::new(None)),
            status: String::new(),
            status_bold: None,
            status_tail: String::new(),
            results: Vec::new(),
            selected: None,
            t0: Instant::now(),
            run: 0,
            queued: false,
        }
    }

    pub fn header(&self) -> String {
        format!("Kontrola průniků: {} ({} dílů)", self.scope, self.parts.len())
    }

    pub fn can_show_all(&self) -> bool {
        self.results.iter().any(|r| r.kind == Kind::Collision)
    }
}

/// Rows of the dialog from the interferences, largest volume first.
pub fn rows_of(results: &[Interference]) -> Vec<ResultRow> {
    let mut rows: Vec<ResultRow> = results
        .iter()
        .map(|i| ResultRow {
            a: i.a,
            b: i.b,
            kind: if is_touch(i) { Kind::Touch } else { Kind::Collision },
            volume: i.volume_mm3,
        })
        .collect();
    rows.sort_by(|x, y| y.volume.partial_cmp(&x.volume).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

/// `"Tělesa {k} / {n}   ·   nalezeno kolizí {m}"`.
pub fn progress_text(p: &Progress) -> String {
    format!("Tělesa {} / {}   ·   nalezeno kolizí {}", p.done, p.total, p.found)
}

/// The final status line, in the parts before the bold collision count, the count, and what follows.
pub fn final_status(stopped: bool, secs: f64, report: &InterferenceReport, rows: &[ResultRow], touch: bool) -> (String, String, String) {
    let n_col = rows.iter().filter(|r| r.kind == Kind::Collision).count();
    let n_touch = rows.len() - n_col;
    let state = if stopped { "Zastaveno" } else { "Hotovo" };
    let head = format!(
        "{state} za {secs:.1} s: {} dvojic dílů / {} dvojic těles s překrytím obálek, ",
        report.part_pairs, report.body_pairs
    );
    let bold = format!("{n_col} kolizí");
    let mut tail = String::new();
    if touch {
        tail.push_str(&format!(", {n_touch} dotyků"));
    }
    tail.push_str(if rows.is_empty() { "." } else { ". Klikni na řádek = ukázat v modelu." });
    (head, bold, tail)
}

/// Overlay of an interference volume: the mesh in the interference colour, always on top.
fn volume_overlay(mesh: &cad_types::ProtoMesh) -> OverlayMesh {
    OverlayMesh {
        positions: mesh.positions.iter().map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]).collect(),
        indices: mesh.indices.clone(),
        color: INTERFERENCE_COLOR,
        alpha: 1.0,
    }
}

impl App {
    /// `run_collisions`: the parts to check, then the dialog.
    pub fn start_collisions(&mut self) {
        let Some(m) = &self.model else { return };
        if self.mode != crate::app::Mode::ThreeD || !m.has_nodes() {
            return;
        }
        let sel_leaves: Vec<NodeId> = m.unique_leaves(&m.selection());
        let (leaves, scope) = if sel_leaves.len() >= 2 {
            (sel_leaves, "vybrané díly")
        } else {
            (m.all_leaves(), "všechny viditelné díly")
        };
        // STL meshes have no solids for a boolean check
        let parts: Vec<NodeId> = leaves
            .into_iter()
            .filter(|l| m.is_visible_leaf(*l))
            .filter(|l| m.node(*l).proto.is_some_and(|p| !m.mesh_only.get(p as usize).copied().unwrap_or(false)))
            .collect();
        if parts.len() < 2 {
            self.info("Pro kontrolu kolizí jsou potřeba aspoň 2 viditelné díly.");
            return;
        }
        // a dialog that is still open is replaced (`collide_dlg.close()`): its check is stopped, it does not run on
        if let Some(old) = self.dlg.collisions.take() {
            old.stop.store(true, Ordering::SeqCst);
        }
        self.dlg.collisions = Some(CollisionDlg::new(scope, parts));
        self.run_collisions_check();
    }

    /// "Spustit" / "Spustit znovu".
    pub fn run_collisions_check(&mut self) {
        let Some(dlg) = self.dlg.collisions.as_mut() else { return };
        let Some(model) = self.model.as_mut() else { return };
        let Some(doc) = model.doc.take() else {
            // the check of the dialog this one replaced is winding down and still has the document: wait for it
            // instead of leaving a dialog that does nothing
            dlg.queued = true;
            dlg.running = true;
            dlg.results.clear();
            dlg.selected = None;
            dlg.status = "Čekám na ukončení předchozí kontroly…".to_string();
            dlg.status_bold = None;
            dlg.status_tail.clear();
            *dlg.progress.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return;
        };
        self.flow.collision_runs += 1;
        dlg.run = self.flow.collision_runs;
        dlg.queued = false;
        dlg.running = true;
        dlg.results.clear();
        dlg.selected = None;
        dlg.status = "Hledám dvojice těles s překrývajícími se obálkami…".to_string();
        dlg.status_bold = None;
        dlg.status_tail.clear();
        dlg.t0 = Instant::now();
        dlg.stop.store(false, Ordering::SeqCst);
        *dlg.progress.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let (nodes, touch) = (dlg.parts.clone(), dlg.touch);
        let (stop, progress, run) = (dlg.stop.clone(), dlg.progress.clone(), dlg.run);
        let model_generation = self.flow.model_generation;
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.overlays.collision = Overlays::default();
        self.refresh_overlays();
        let spawned = spawn_big("collisions", move || {
            let ctx2 = ctx.clone();
            let result = doc
                .interferences_ex(&nodes, touch, &mut |p| {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(Progress { done: p.done, total: p.total, found: p.found });
                    ctx2.request_repaint();
                    !stop.load(Ordering::SeqCst)
                })
                .map_err(|e| e.message().to_string());
            App::send(&tx, &ctx, Event::Collision(CollisionEvent::Done { model_generation, run, doc, result }));
        });
        if spawned.is_err() {
            if let Some(d) = self.dlg.collisions.as_mut() {
                d.running = false;
            }
            self.critical("Kontrolu kolizí se nepodařilo spustit.");
        }
    }

    pub fn on_collision_event(&mut self, ev: CollisionEvent) {
        let CollisionEvent::Done { model_generation, run, doc, result } = ev;
        if model_generation != self.flow.model_generation {
            return; // another model is open now; this document is no longer needed
        }
        let recalc = match self.model.as_mut() {
            Some(m) => {
                m.doc = Some(doc);
                m.needs_recalc()
            }
            None => false,
        };
        if recalc {
            // the panel was built while the check had the document, so it shows zero volumes
            self.refresh_numbers();
        }
        let Some(dlg) = self.dlg.collisions.as_mut() else { return };
        if dlg.queued {
            // the dialog of a newer check was waiting for the document; the answer in hand is the old one's
            self.run_collisions_check();
            return;
        }
        if dlg.run != run {
            return;
        }
        dlg.running = false;
        match result {
            Ok(report) => {
                let rows = rows_of(&report.results);
                let stopped = report.stopped || dlg.stop.load(Ordering::SeqCst);
                let (head, bold, tail) = final_status(stopped, dlg.t0.elapsed().as_secs_f64(), &report, &rows, dlg.touch);
                dlg.status = head;
                dlg.status_bold = Some(bold);
                dlg.status_tail = tail;
                dlg.results = rows;
            }
            Err(msg) => {
                dlg.status = msg;
                dlg.status_bold = None;
                dlg.status_tail.clear();
            }
        }
    }

    /// The properties panel again, with numbers: it was built while the interference check held the document. A
    /// measurement in progress is measured again; the other panels are rebuilt.
    pub(crate) fn refresh_numbers(&mut self) {
        if !self.meas.picked.is_empty() {
            self.picks_changed();
            return;
        }
        let mass_panel = matches!(
            &self.props,
            Props::Rows { rows, .. } if rows.first().is_some_and(|r| r.bold_label && r.label == crate::mass::HEADING)
        );
        if mass_panel {
            self.show_mass();
        } else {
            self.refresh_props();
        }
    }

    /// A row was clicked: select both parts, show the common volume, zoom (`on_pick`).
    pub fn collision_pick(&mut self, index: usize) {
        let Some(row) = self.dlg.collisions.as_ref().and_then(|d| d.results.get(index).copied()) else { return };
        self.overlays.cog = Overlays::default();
        self.overlays.collision = Overlays::default();
        if let Some(m) = self.model.as_mut() {
            m.select_only([row.a, row.b]);
            for n in [row.a, row.b] {
                m.expand_ancestors(n);
            }
            m.scroll_to = Some(row.a);
            m.rows_dirty = true;
        }
        self.sync_selection_to_renderer();
        if row.kind == Kind::Collision {
            if let Some(Ok(Some(mesh))) = self.model.as_ref().and_then(|m| m.doc.as_ref()).map(|d| d.interference_mesh(row.a, row.b)) {
                self.overlays.collision.meshes.push(volume_overlay(&mesh));
            }
        }
        self.refresh_overlays();
        self.r3().fit_nodes(&[row.a, row.b]);
        self.refresh_props();
    }

    /// "Zobrazit všechny průniky".
    pub fn collision_show_all(&mut self) {
        let rows: Vec<ResultRow> = self.dlg.collisions.as_ref().map(|d| d.results.clone()).unwrap_or_default();
        let mut o = Overlays::default();
        if let Some(doc) = self.model.as_ref().and_then(|m| m.doc.as_ref()) {
            for r in rows.iter().filter(|r| r.kind == Kind::Collision) {
                if let Ok(Some(mesh)) = doc.interference_mesh(r.a, r.b) {
                    o.meshes.push(volume_overlay(&mesh));
                }
            }
        }
        self.overlays.collision = o;
        self.refresh_overlays();
    }

    /// Closing the dialog stops the check and removes the overlay (`closeEvent`).
    pub fn close_collisions(&mut self) {
        if let Some(d) = self.dlg.collisions.take() {
            d.stop.store(true, Ordering::SeqCst);
        }
        self.overlays.collision = Overlays::default();
        self.refresh_overlays();
    }

    /// The non-modal dialog "Kolize dílů".
    pub fn show_collisions(&mut self, ctx: &egui::Context) {
        let Some(mut dlg) = self.dlg.collisions.take() else { return };
        let (names, mut open) = {
            let names: Vec<(String, String)> = dlg
                .results
                .iter()
                .map(|r| {
                    let n = |id: NodeId| self.model.as_ref().map(|m| m.node(id).name.clone()).unwrap_or_default();
                    (n(r.a), n(r.b))
                })
                .collect();
            (names, true)
        };
        if dlg.running {
            let p = *dlg.progress.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(p) = p {
                dlg.status = progress_text(&p);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut run_clicked = false;
        let mut stop_clicked = false;
        let mut show_all = false;
        let mut close_clicked = false;
        let mut picked: Option<usize> = None;
        egui::Window::new("Kolize dílů")
            .id(egui::Id::unique("collisions-window"))
            .default_size([640.0, 460.0])
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(dlg.header());
                ui.horizontal(|ui| {
                    ui.add_enabled(!dlg.running, egui::Checkbox::new(&mut dlg.touch, "hlásit i dotyky (plochy na sobě, bez průniku)"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let text = if dlg.running {
                            "Zastavit"
                        } else if dlg.results.is_empty() && dlg.status_bold.is_none() {
                            "Spustit"
                        } else {
                            "Spustit znovu"
                        };
                        if ui.button(text).clicked() {
                            if dlg.running {
                                stop_clicked = true;
                            } else {
                                run_clicked = true;
                            }
                        }
                    });
                });
                if dlg.running {
                    let p = *dlg.progress.lock().unwrap_or_else(|e| e.into_inner());
                    let frac = p.map(|p| p.done as f32 / p.total.max(1) as f32).unwrap_or(0.0);
                    ui.add(egui::ProgressBar::new(frac));
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(dlg.status.clone());
                    if let Some(b) = &dlg.status_bold {
                        ui.label(egui::RichText::new(b.clone()).strong());
                        ui.label(dlg.status_tail.clone());
                    }
                });
                let avail = ui.available_height() - 36.0;
                egui::ScrollArea::vertical().max_height(avail.max(80.0)).auto_shrink([false, false]).show(ui, |ui| {
                    egui::Grid::new("collision-rows").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                        for h in ["Díl A", "Díl B", "Typ", "Průnik [mm³]"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for (i, r) in dlg.results.iter().enumerate() {
                            let (a, b) = &names[i];
                            let sel = dlg.selected == Some(i);
                            let mut clicked = false;
                            clicked |= ui.selectable_label(sel, a).clicked();
                            clicked |= ui.selectable_label(sel, b).clicked();
                            clicked |= ui.selectable_label(sel, r.kind.text()).clicked();
                            let vol = if r.kind == Kind::Collision { fmt_num(r.volume, 2) } else { "–".to_string() };
                            clicked |= ui.selectable_label(sel, vol).clicked();
                            ui.end_row();
                            if clicked {
                                picked = Some(i);
                            }
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(dlg.can_show_all(), egui::Button::new("Zobrazit všechny průniky")).clicked() {
                        show_all = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Zavřít").clicked() {
                            close_clicked = true;
                        }
                    });
                });
            });
        if close_clicked {
            open = false;
        }
        if stop_clicked {
            dlg.stop.store(true, Ordering::SeqCst);
            if dlg.queued {
                // still waiting for the document: nothing runs yet, so there is nothing to wind down
                dlg.queued = false;
                dlg.running = false;
                dlg.status = "Zastaveno.".to_string();
            }
        }
        if let Some(i) = picked {
            dlg.selected = Some(i);
        }
        if !open {
            dlg.stop.store(true, Ordering::SeqCst);
            self.dlg.collisions = Some(dlg);
            self.close_collisions();
            return;
        }
        self.dlg.collisions = Some(dlg);
        if run_clicked {
            self.run_collisions_check();
        }
        if show_all {
            self.collision_show_all();
        }
        if let Some(i) = picked {
            self.collision_pick(i);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn inter(a: u32, b: u32, v: f64) -> Interference {
        Interference { a, b, volume_mm3: v, bounds: None }
    }

    #[test]
    fn rows_are_sorted_by_volume_and_touches_are_marked() {
        let rows = rows_of(&[inter(1, 2, 5.0), inter(3, 4, 0.0), inter(5, 6, 120.5), inter(7, 8, 0.0005)]);
        // a touch with a tiny volume sorts before one with none
        assert_eq!(rows.iter().map(|r| (r.a, r.kind)).collect::<Vec<_>>(), vec![
            (5, Kind::Collision),
            (1, Kind::Collision),
            (7, Kind::Touch),
            (3, Kind::Touch),
        ]);
        assert_eq!(Kind::Collision.text(), "kolize");
        assert_eq!(Kind::Touch.text(), "dotyk");
    }

    #[test]
    fn status_texts() {
        let p = Progress { done: 3, total: 20, found: 1 };
        assert_eq!(progress_text(&p), "Tělesa 3 / 20   ·   nalezeno kolizí 1");
        let report = InterferenceReport { part_pairs: 5, body_pairs: 7, ..Default::default() };
        let rows = rows_of(&[inter(1, 2, 3.0), inter(3, 4, 0.0)]);
        let (h, b, t) = final_status(false, 1.26, &report, &rows, true);
        assert_eq!(h, "Hotovo za 1.3 s: 5 dvojic dílů / 7 dvojic těles s překrytím obálek, ");
        assert_eq!(b, "1 kolizí");
        assert_eq!(t, ", 1 dotyků. Klikni na řádek = ukázat v modelu.");
        let (h, _, t) = final_status(true, 0.04, &report, &[], false);
        assert!(h.starts_with("Zastaveno za 0.0 s"));
        assert_eq!(t, ".");
    }

    #[test]
    fn dialog_header_names_scope_and_count() {
        let d = CollisionDlg::new("vybrané díly", vec![1, 2, 3]);
        assert_eq!(d.header(), "Kontrola průniků: vybrané díly (3 dílů)");
        assert!(!d.can_show_all());
    }
}
