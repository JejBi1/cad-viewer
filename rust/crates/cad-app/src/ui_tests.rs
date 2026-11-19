//! The window driven with synthetic input: real documents, a headless GPU, egui frames run by hand. Mouse and keyboard
//! events go through the same code as the real window. Tests that need a graphics adapter say so and return without one.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cad_core::settings::Settings;
use cad_render::{wgpu, GpuContext, Renderer2d, Renderer3d};
use cad_types::NodeId;
use eframe::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, RawInput, Rect};
use eframe::egui_wgpu;

use crate::app::{App, MeasureKind, Mode};
use crate::props::Props;
use crate::testgen;
use crate::ui::Init;

struct H {
    app: App,
    ctx: egui::Context,
    frame: eframe::Frame,
    dir: tempfile::TempDir,
    time: f64,
    mods: Modifiers,
    /// Paints the frames when a picture is wanted (`STEPVIEW_UI_SHOTS`).
    painter: egui_wgpu::Renderer,
    last: Option<(Vec<egui::epaint::ClippedShape>, f32)>,
    gpu: GpuContext,
}

impl H {
    fn new() -> Option<H> {
        let gpu = GpuContext::new_headless().ok()?;
        let dir = tempfile::tempdir().ok()?;
        let ctx = egui::Context::default();
        let fmt = wgpu::TextureFormat::Rgba8Unorm;
        let r3 = Renderer3d::new(&gpu.device, fmt).with_queue(&gpu.queue).shared();
        let r2 = Renderer2d::new(&gpu.device, fmt).with_queue(&gpu.queue).shared();
        let init = Init { file: None, link: None, exe: PathBuf::from("StepViewer"), early: None };
        let mut app = App::build(ctx.clone(), r3, r2, None, init, Settings::default());
        // nothing of the user's is touched
        app.settings_path = dir.path().join("settings.json");
        app.data_dir = dir.path().join("cache");
        app.prune_due = None;
        let painter = egui_wgpu::Renderer::new(&gpu.device, fmt, egui_wgpu::RendererOptions::default());
        ctx.all_styles_mut(|s| s.animation_time = 0.0);
        let mut h = H { app, ctx, frame: eframe::Frame::_new_kittest(), dir, time: 0.0, mods: Modifiers::NONE, painter, last: None, gpu };
        h.frames(2);
        Some(h)
    }

    fn run(&mut self, events: Vec<Event>, mods: Modifiers) {
        let mut raw = RawInput::default();
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1500.0, 900.0)));
        let mut all = Vec::new();
        if mods != self.mods {
            all.push(Event::ModifiersChanged(mods));
            self.mods = mods;
        }
        all.extend(events);
        raw.events = all;
        self.time += 0.02;
        raw.time = Some(self.time);
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut out = self.ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, frame));
        for (id, deltas) in out.textures_delta.set.drain() {
            for d in deltas {
                self.painter.update_texture(&self.gpu.device, &self.gpu.queue, id, &d);
            }
        }
        for id in out.textures_delta.free.drain() {
            self.painter.free_texture(&id);
        }
        self.last = Some((out.shapes, out.pixels_per_point));
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.run(vec![], Modifiers::NONE);
        }
    }

    /// Frames until the loader has nothing left to do.
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            self.frames(1);
            if !self.app.flow.active() && self.app.flow.token > 0 {
                break;
            }
            assert!(Instant::now() < end, "loading did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.frames(3);
    }

    fn open(&mut self, path: &Path) {
        self.app.open_file(path.to_path_buf(), None);
        self.settle();
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn open_assembly(&mut self) {
        let p = self.write("sestava.step", testgen::step_assembly().as_bytes());
        self.open(&p);
        assert_eq!(self.app.mode, Mode::ThreeD);
    }

    fn node(&self, name: &str) -> NodeId {
        self.app.model.as_ref().unwrap().tree.nodes.iter().find(|n| n.name == name).unwrap_or_else(|| panic!("no node {name}")).id
    }

    /// Position in points of a world point of the 3D view.
    fn at(&self, world: [f64; 3]) -> Pos2 {
        let px = self.app.r3().world_to_pixel(world).expect("on screen");
        self.app.view_rect.min + egui::vec2(px[0] as f32, px[1] as f32)
    }

    fn mv(&mut self, pos: Pos2) {
        self.run(vec![Event::PointerMoved(pos)], Modifiers::NONE);
    }

    fn button(&mut self, pos: Pos2, b: PointerButton, pressed: bool, mods: Modifiers) {
        self.run(vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: b, pressed, modifiers: mods }], mods);
    }

    fn click_with(&mut self, pos: Pos2, b: PointerButton, mods: Modifiers) {
        self.button(pos, b, true, mods);
        self.button(pos, b, false, mods);
        self.frames(1);
    }

    fn click(&mut self, pos: Pos2) {
        self.click_with(pos, PointerButton::Primary, Modifiers::NONE);
    }

    fn key(&mut self, key: Key) {
        self.key_with(key, Modifiers::NONE);
    }

    fn key_with(&mut self, key: Key, mods: Modifiers) {
        self.run(vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers: mods }], mods);
        self.run(vec![Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers: mods }], mods);
        self.frames(1);
    }

    fn selected(&self) -> Vec<NodeId> {
        self.app.model.as_ref().map(|m| m.selection()).unwrap_or_default()
    }

    fn rows(&self) -> Vec<(String, String)> {
        match &self.app.props {
            Props::Rows { rows, .. } => rows.iter().map(|r| (r.label.clone(), r.value_text())).collect(),
            Props::Welcome => Vec::new(),
        }
    }

    fn row(&self, label: &str) -> Option<String> {
        self.rows().into_iter().find(|(l, _)| l == label).map(|(_, v)| v)
    }
}

macro_rules! harness {
    () => {
        match H::new() {
            Some(h) => h,
            None => {
                eprintln!("SKIP: no graphics adapter");
                return;
            }
        }
    };
}

/// A point of the plate that no other part covers in the isometric view.
const PLATE_POINT: [f64; 3] = [90.0, 10.0, 8.0];
/// The top of the block.
const BLOCK_POINT: [f64; 3] = [50.0, 30.0, 28.0];

#[test]
fn start_screen_and_loading_a_model() {
    let mut h = harness!();
    assert_eq!(h.app.mode, Mode::Empty);
    assert_eq!(h.app.props, Props::Welcome);
    assert_eq!(h.app.window_title(), "CAD prohlížeč");
    h.open_assembly();
    assert_eq!(h.app.window_title(), "sestava.step – CAD prohlížeč");
    let status = h.app.status.model.clone();
    assert!(status.starts_with("sestava.step  |  4 dílů (3 unikátních), 1 sestav  |  rozměr "), "{status}");
    assert!(status.contains("načteno za"), "{status}");
    // the panel shows the model, the tree has the assembly open
    assert_eq!(h.row("Dílů").unwrap(), "4 (3 unikátních)");
    assert_eq!(h.app.model.as_ref().unwrap().rows.len(), 5);
    // recent files and the last folder are remembered in the settings file
    assert!(h.app.settings.recent[0].ends_with("sestava.step"));
    let saved = Settings::load_from(&h.app.settings_path);
    assert_eq!(saved.recent, h.app.settings.recent);
}

#[test]
fn clicks_select_parts_and_the_tree_follows() {
    let mut h = harness!();
    h.open_assembly();
    let (plate, block) = (h.node("Deska"), h.node("Blok"));
    h.click(h.at(PLATE_POINT));
    assert_eq!(h.selected(), vec![plate]);
    assert_eq!(h.row("Název").unwrap(), "Deska");
    assert_eq!(h.row("Typ").unwrap(), "díl");
    // Ctrl+click adds, a second Ctrl+click removes
    h.click_with(h.at(BLOCK_POINT), PointerButton::Primary, Modifiers::CTRL);
    assert_eq!(h.selected(), vec![plate.min(block), plate.max(block)]);
    assert_eq!(h.row("Vybráno").unwrap(), "2 položek, 2 dílů");
    h.click_with(h.at(BLOCK_POINT), PointerButton::Primary, Modifiers::CTRL);
    assert_eq!(h.selected(), vec![plate]);
    // a click on the block replaces the selection
    h.click(h.at(BLOCK_POINT));
    assert_eq!(h.selected(), vec![block]);
    // empty space clears it and the panel shows the model again
    h.click(h.app.view_rect.min + egui::vec2(30.0, 200.0));
    assert!(h.selected().is_empty());
    assert!(h.row("Složka").is_some());
}

#[test]
fn camera_mouse_and_keyboard() {
    let mut h = harness!();
    h.open_assembly();
    let start = h.app.r3().camera;
    // wheel: 1.2 per notch, under the cursor
    let p = h.at(PLATE_POINT);
    h.run(vec![Event::PointerMoved(p)], Modifiers::NONE);
    h.run(
        vec![Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );
    let zoomed = h.app.r3().camera;
    assert!((zoomed.scale / start.scale - 1.0 / 1.2).abs() < 1e-9, "{} {}", zoomed.scale, start.scale);
    // right button drag orbits about the point under the cursor
    let (dir0, centre0) = (zoomed.direction(), zoomed.center);
    h.button(p, PointerButton::Secondary, true, Modifiers::NONE);
    h.mv(p + egui::vec2(30.0, 0.0));
    h.mv(p + egui::vec2(120.0, 20.0));
    h.button(p + egui::vec2(120.0, 20.0), PointerButton::Secondary, false, Modifiers::NONE);
    h.frames(1);
    let orbited = h.app.r3().camera;
    assert!(orbited.direction().dot(dir0) < 0.999, "the view turned");
    assert!(h.app.dlg.context.is_none(), "a drag does not open the menu");
    assert!((orbited.center - centre0).length() > 1e-6, "the centre moves when the pivot is not the centre");
    // a middle button drag pans: the direction stays
    let dir1 = orbited.direction();
    h.button(p, PointerButton::Middle, true, Modifiers::NONE);
    h.mv(p + egui::vec2(40.0, 10.0));
    h.mv(p + egui::vec2(80.0, 30.0));
    h.button(p + egui::vec2(80.0, 30.0), PointerButton::Middle, false, Modifiers::NONE);
    let panned = h.app.r3().camera;
    assert!(panned.direction().dot(dir1) > 0.999999);
    assert!((panned.center - orbited.center).length() > 1e-6);
    // a right click without a drag opens the context menu; a click elsewhere closes it
    h.click_with(p, PointerButton::Secondary, Modifiers::NONE);
    assert!(h.app.dlg.context.is_some());
    h.frames(2);
    h.click(h.app.view_rect.min + egui::vec2(30.0, 200.0));
    assert!(h.app.dlg.context.is_none());
    // arrow keys orbit by 15 degrees when the view has the focus
    let before = h.app.r3().camera.direction();
    h.key(Key::ArrowLeft);
    let after = h.app.r3().camera.direction();
    let angle = before.dot(after).clamp(-1.0, 1.0).acos().to_degrees();
    assert!(angle > 5.0 && angle < 16.0, "{angle}");
    // standard views by the number keys, fit by F
    h.key(Key::Num3);
    let top = h.app.r3().camera.direction();
    // the direction points from the eye into the scene: looking down from the top
    assert!((top.z + 1.0).abs() < 1e-9, "{top:?}");
    h.key(Key::Num0);
    let iso = h.app.r3().camera.direction();
    assert!(iso.x < -0.5 && iso.y > 0.5 && iso.z < -0.5, "{iso:?}");
    // perspective on P
    h.key(Key::P);
    assert!(h.app.r3().camera.perspective && h.app.opts.persp);
}

#[test]
fn hiding_isolating_and_transparency() {
    let mut h = harness!();
    h.open_assembly();
    let (plate, block) = (h.node("Deska"), h.node("Blok"));
    h.click(h.at(PLATE_POINT));
    h.key(Key::H);
    assert!(h.app.model.as_ref().unwrap().hidden.contains(&plate));
    assert!(h.selected().is_empty());
    assert_eq!(h.app.model.as_ref().unwrap().check(plate), crate::model3::Check::Off);
    h.key(Key::A);
    assert!(h.app.model.as_ref().unwrap().hidden.is_empty());
    // "Jen vybrané" hides everything else
    h.app.select_node(block);
    h.key(Key::I);
    let m = h.app.model.as_ref().unwrap();
    assert_eq!(m.hidden.len(), 3);
    assert!(m.is_visible_leaf(block));
    h.key(Key::A);
    // transparency on and off
    h.app.select_node(block);
    h.key(Key::T);
    assert!(h.app.r3().transparency(block) > 0.7);
    h.app.select_node(block);
    h.key(Key::T);
    assert_eq!(h.app.r3().transparency(block), 0.0);
}

#[test]
fn while_the_check_has_the_document_a_measurement_and_a_size_say_so_and_come_back() {
    use crate::collide::CollisionEvent;
    let computing = crate::strings::COMPUTING_AFTER_CHECK;
    let mut h = harness!();
    h.open_assembly();
    let plate = h.node("Deska");
    // a measurement of one face
    h.key(Key::M);
    h.click(h.at(PLATE_POINT));
    assert_eq!(h.row("Entita").unwrap(), "plocha – rovina");
    // the check takes the document: the measurement says that its value is being computed, not nothing
    let doc = h.app.model.as_mut().unwrap().doc.take().unwrap();
    h.app.picks_changed();
    assert_eq!(h.row("Hodnota").as_deref(), Some(computing));
    assert!(h.row("Entita").is_none());
    // the document is back: the measurement is measured again and shown
    let generation = h.app.flow.model_generation;
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 1, doc, result: Err("x".into()) })).unwrap();
    h.frames(2);
    assert_eq!(h.row("Entita").unwrap(), "plocha – rovina");
    assert!(h.row("Hodnota").is_none());
    // the size of a selected part, while the check has the document, and then when it is back
    h.app.set_measure(None);
    h.app.model.as_mut().unwrap().select_only([plate]);
    let doc = h.app.model.as_mut().unwrap().doc.take().unwrap();
    h.app.refresh_props();
    assert_eq!(h.row("Rozměr X×Y×Z").as_deref(), Some(computing));
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 1, doc, result: Err("x".into()) })).unwrap();
    h.frames(2);
    assert!(h.row("Rozměr X×Y×Z").unwrap().ends_with("mm"), "the size is there again");
}

#[test]
fn measuring_two_faces_and_the_angle_tool() {
    let mut h = harness!();
    h.open_assembly();
    h.key(Key::M);
    assert_eq!(h.app.measure, Some(MeasureKind::Dist));
    assert!(h.app.status.model.starts_with("Měřit: klikej na vrcholy, hrany a plochy"));
    assert_eq!(h.rows()[0], ("Měření".to_string(), String::new()));
    h.click(h.at(PLATE_POINT));
    assert_eq!(h.app.meas.picked.len(), 1);
    assert_eq!(h.row("Entita").unwrap(), "plocha – rovina");
    assert!(h.row("Plocha").is_some());
    // a second face gives the relation
    h.click(h.at(BLOCK_POINT));
    assert_eq!(h.app.meas.picked.len(), 2);
    assert!(h.row("Entita 1").is_some() && h.row("Entita 2").is_some());
    let d = h.row("Min. vzdálenost").expect("distance");
    assert!(d.contains("mm"), "{d}");
    assert!(h.row("Vzdálenost rovin").is_some() || h.row("Kolmá vzdálenost (tloušťka)").is_some());
    assert!(h.app.status.model.starts_with("Vzdálenost "), "{}", h.app.status.model);
    // clicking the first face again removes it; empty space clears
    h.click(h.at(PLATE_POINT));
    assert_eq!(h.app.meas.picked.len(), 1);
    h.click(h.app.view_rect.min + egui::vec2(30.0, 200.0));
    assert!(h.app.meas.picked.is_empty());
    // Shift+click gives an exact point on the face
    h.click_with(h.at(PLATE_POINT), PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!(h.row("Entita").unwrap(), "bod – bod na ploše");
    // Enter keeps the measurement and clears the picks; Ctrl+C copies; Esc ends the tool
    h.key(Key::Enter);
    assert!(h.app.meas.picked.is_empty());
    h.key(Key::Escape);
    assert_eq!(h.app.measure, None);
    // the angle tool takes vertices: three clicks on corners of the plate
    h.key(Key::U);
    assert_eq!(h.app.measure, Some(MeasureKind::Angle));
    for corner in [[100.0, 0.0, 8.0], [0.0, 0.0, 8.0], [0.0, 60.0, 8.0]] {
        h.click(h.at(corner));
    }
    assert!(h.app.status.model.starts_with("Úhel: 90,00°"), "{}", h.app.status.model);
    h.key(Key::U);
    assert_eq!(h.app.measure, None);
}

#[test]
fn mass_materials_and_the_section() {
    let mut h = harness!();
    h.open_assembly();
    let plate = h.node("Deska");
    // mass of the whole model without materials: only the volume centre
    h.key(Key::G);
    assert_eq!(h.rows()[0].0, "Hmotnostní vlastnosti");
    assert_eq!(h.row("Rozsah").unwrap(), "celý model, 4 dílů");
    assert!(h.row("Hmotnost (ocel 7,85)").is_some());
    assert_eq!(h.app.overlays.cog.labels[0].text, "  T (objem)");
    // an assigned material makes it a mass; the dialog is the real one
    h.app.select_node(plate);
    h.app.assign_material();
    let mut dlg = h.app.dlg.material.take().expect("dialog");
    assert!(dlg.title.starts_with("Materiál – 1 dílů"));
    dlg.combo = 1;
    dlg.changed();
    h.app.apply_material(dlg);
    assert_eq!(h.row("Materiál").unwrap(), "Ocel (konstrukční) (přiřadit…)");
    assert_eq!(h.row("Hmotnost").unwrap(), "0,3768 kg");
    assert_eq!(h.app.model.as_ref().unwrap().info_text(plate), "Ocel");
    // the assignment is in the file of the model, not in the user's folders
    assert!(h.app.model.as_ref().unwrap().mats.as_ref().unwrap().path().starts_with(h.dir.path()));
    // the section plane: Z at the middle of the model
    h.app.opts.section_axis = 3;
    h.app.opts.section_slider = 500.0;
    h.app.update_section(true);
    let s = h.app.r3().section().expect("section");
    assert_eq!(s.axis, 2);
    let b = h.app.model.as_ref().unwrap().model_box.unwrap();
    assert!((s.position - (b.min[2] + b.max[2]) / 2.0).abs() < 1e-9);
    assert!(h.app.status.temp.as_ref().unwrap().0.starts_with("Řez Z = "));
    h.app.opts.section_flip = true;
    h.app.update_section(false);
    assert!(h.app.r3().section().unwrap().flip);
    h.app.opts.section_axis = 0;
    h.app.update_section(false);
    assert!(h.app.r3().section().is_none());
}

#[test]
fn the_collision_check_runs_on_a_thread_and_returns_the_document() {
    let mut h = harness!();
    h.open_assembly();
    h.key(Key::K);
    assert!(h.app.dlg.collisions.is_some());
    // the plate, two bolts and the block only touch: no collisions
    let end = Instant::now() + Duration::from_secs(60);
    while h.app.dlg.collisions.as_ref().is_some_and(|d| d.running) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() < end);
    }
    h.frames(2);
    let d = h.app.dlg.collisions.as_ref().unwrap();
    assert!(d.results.is_empty(), "{:?}", d.results);
    assert!(d.status.starts_with("Hotovo za "), "{}", d.status);
    assert!(h.app.model.as_ref().unwrap().doc.is_some(), "the document is back");
    // with the touch option the contacts are listed
    h.app.dlg.collisions.as_mut().unwrap().touch = true;
    h.app.run_collisions_check();
    while h.app.dlg.collisions.as_ref().is_some_and(|d| d.running) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() < end);
    }
    h.frames(2);
    let d = h.app.dlg.collisions.as_ref().unwrap();
    assert_eq!(d.results.len(), 3, "{:?}", d.results);
    assert!(d.results.iter().all(|r| r.kind == crate::collide::Kind::Touch));
    // a row selects both parts
    h.app.collision_pick(0);
    assert_eq!(h.selected().len(), 2);
    h.app.close_collisions();
    assert!(h.app.dlg.collisions.is_none());
}

#[test]
fn drawings_pick_measure_and_layers() {
    let mut h = harness!();
    let p = h.write("vykres.dxf", testgen::dxf_drawing().as_bytes());
    h.open(&p);
    assert_eq!(h.app.mode, Mode::TwoD);
    assert_eq!(h.app.window_title(), "vykres.dxf – CAD prohlížeč");
    assert!(h.app.status.model.starts_with("vykres.dxf  |  R2013  |  13 entit, 5 hladin  |  rozměr 220,00 × 150,00 mm"), "{}", h.app.status.model);
    assert_eq!(h.row("Layout").unwrap(), "Model");
    assert_eq!(h.row("Jednotky").unwrap(), "mm");
    let layers: Vec<String> = h.app.draw.as_ref().unwrap().layers.iter().map(|l| l.name.clone()).collect();
    assert_eq!(layers, vec!["0", "Kóty", "Obrys", "Osy", "Skrytá"]);
    // the panel counts the layers of the list (the file's own layer table has four), the size is written like the
    // Python f"{size:.2f} MB", and the folder is the one of the opened file
    assert!(h.row("Hladin / bloků").unwrap().starts_with("5 / "), "{:?}", h.row("Hladin / bloků"));
    assert_eq!(h.row("Velikost").unwrap(), crate::util::size_mb(std::fs::metadata(&p).unwrap().len()));
    assert_eq!(h.row("Složka").unwrap(), p.parent().unwrap().to_string_lossy());
    // the screen position of a drawing point
    let at = |h: &H, w: [f64; 2]| {
        let r = h.app.r2();
        let s = r.view.to_screen(w, r.viewport());
        h.app.view_rect.min + egui::vec2(s[0] as f32, s[1] as f32)
    };
    // a click on the big circle picks it
    let on_circle = at(&h, [150.0 + 25.0, 60.0]);
    h.click(on_circle);
    let picked = h.app.draw.as_ref().unwrap().picked.clone().expect("picked");
    assert_eq!(picked.dxftype, "CIRCLE");
    assert_eq!(picked.layer, "Obrys");
    assert_eq!(h.row("Entita").unwrap(), "CIRCLE");
    assert_eq!(h.row("Poloměr").unwrap(), "25,00");
    // empty space clears it
    h.click(at(&h, [100.0, 100.0]));
    assert!(h.app.draw.as_ref().unwrap().picked.is_none());
    // the measure tool: two points on the plate corners, snapped to the end points of the lines
    h.key(Key::M);
    h.mv(at(&h, [0.2, 0.2]));
    assert!(h.app.meas2.snap.is_some(), "the cursor near a corner snaps");
    h.click(at(&h, [0.2, 0.2]));
    h.click(at(&h, [199.8, 0.2]));
    assert_eq!(h.app.meas2.texts.len(), 1);
    assert_eq!(h.app.meas2.texts[0].1, "200,00");
    assert!(h.app.status.model.starts_with("Vzdálenost: 200,00   (ΔX 200,00, ΔY 0,00, úhel 0,00°)"), "{}", h.app.status.model);
    h.key(Key::Escape);
    assert_eq!(h.app.measure, None);
    assert!(h.app.meas2.texts.is_empty());
    // switching a layer off redraws without it, after the pause of 150 ms
    let (kote, before) = {
        let d = h.app.draw.as_ref().unwrap();
        (d.layers.iter().find(|l| l.name == "Kóty").unwrap().id, h.app.r2().visible_bounds().unwrap())
    };
    h.app.toggle_layer(kote, false);
    std::thread::sleep(Duration::from_millis(200));
    h.settle();
    let after = h.app.r2().visible_bounds().unwrap();
    // the dimension line 20 below the plate and its text are gone: the drawing is lower by that much
    assert!(after.0[1] > before.0[1] + 5.0, "{before:?} {after:?}");
    assert!(!h.app.draw.as_ref().unwrap().visible(kote));
    // "Zobrazit všechny hladiny" shows the layer that is off in the file as well
    h.key(Key::A);
    h.settle();
    let d = h.app.draw.as_ref().unwrap();
    assert!(d.layers.iter().all(|l| d.visible(l.id)));
}

#[test]
fn an_stl_gets_its_sharp_edges_after_the_analysis() {
    let mut h = harness!();
    let p = h.write("kostka.stl", &testgen::stepgen::box_stl([40.0, 30.0, 20.0]));
    h.open(&p);
    assert_eq!(h.app.mode, Mode::ThreeD);
    let end = Instant::now() + Duration::from_secs(30);
    while h.app.model.as_ref().is_some_and(|m| m.stl.is_none()) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() < end, "mesh analysis");
    }
    let m = h.app.model.as_mut().unwrap();
    assert!(!m.stl.as_mut().unwrap().mesh.loops().is_empty());
    assert!(!m.stl.as_ref().unwrap().proto.edges.is_empty());
}

// ---------------------------------------------------------------------- pictures of the window

impl H {
    /// The last frame as a PNG of the whole window, when `STEPVIEW_UI_SHOTS` names a folder.
    fn shot(&mut self, name: &str) {
        let Some(dir) = std::env::var_os("STEPVIEW_UI_SHOTS") else { return };
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        // a few frames let panels and dialogs that are sized in their first frame settle
        self.frames(3);
        let (shapes, ppp) = self.last.clone().expect("a frame");
        let jobs = self.ctx.tessellate(shapes, ppp);
        let size = [1500u32, 900];
        let sd = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: ppp };
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("window picture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("window picture") });
        let extra = self.painter.update_buffers(device, queue, &mut enc, &jobs, &sd);
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("window picture"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.painter.render(&mut pass, &jobs, &sd);
        }
        let bpr = (size[0] * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("window picture readback"),
            size: u64::from(bpr) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(size[1]) },
            },
            extent,
        );
        queue.submit(extra.into_iter().chain(Some(enc.finish())));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let raw = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        let mut data = Vec::new();
        for y in 0..size[1] {
            let o = (y * bpr) as usize;
            data.extend_from_slice(&raw[o..o + (size[0] * 4) as usize]);
        }
        let png = cad_render::capture::png_from_rgba(size[0], size[1], data).unwrap();
        std::fs::write(dir.join(format!("{name}.png")), png).unwrap();
    }
}

/// Pictures of many states of the window, for looking at: `STEPVIEW_UI_SHOTS=<folder> cargo test ui_screenshots`.
#[test]
fn ui_screenshots() {
    if std::env::var_os("STEPVIEW_UI_SHOTS").is_none() {
        return;
    }
    let mut h = harness!();
    h.shot("00-start");
    h.open_assembly();
    h.shot("01-model");
    h.click(h.at(PLATE_POINT));
    h.shot("02-selected-part");
    h.app.dlg.about = true;
    h.shot("03-about");
    h.app.dlg.about = false;
    h.frames(2);
    h.key(Key::G);
    h.shot("04-mass");
    h.app.select_node(h.node("Deska"));
    h.app.assign_material();
    h.shot("05-material-dialog");
    h.app.dlg.material = None;
    h.frames(2);
    h.key(Key::M);
    h.click(h.at(PLATE_POINT));
    h.click(h.at(BLOCK_POINT));
    h.shot("06-measure-two-faces");
    h.click_with(h.at(PLATE_POINT), PointerButton::Secondary, Modifiers::NONE);
    h.frames(2);
    h.shot("07-context-menu-measure");
    h.key(Key::Escape);
    h.key(Key::Escape);
    h.click_with(h.at(PLATE_POINT), PointerButton::Secondary, Modifiers::NONE);
    h.frames(2);
    h.shot("08-context-menu");
    h.app.close_context_menu();
    h.app.dlg.collisions = Some(crate::collide::CollisionDlg::new("všechny viditelné díly", vec![1, 2, 3, 4]));
    h.app.dlg.collisions.as_mut().unwrap().touch = true;
    h.app.run_collisions_check();
    let end = Instant::now() + Duration::from_secs(60);
    while h.app.dlg.collisions.as_ref().is_some_and(|d| d.running) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() < end);
    }
    h.frames(2);
    h.shot("09-collisions");
    h.app.close_collisions();
    h.app.opts.section_axis = 1;
    h.app.opts.section_slider = 400.0;
    h.app.update_section(true);
    h.key(Key::P);
    h.shot("10-section-perspective");
    h.app.msgs.push_back(crate::app::MsgBox {
        kind: crate::app::MsgKind::Critical,
        text: "Nepodařilo se načíst model:\nSoubor se nepodařilo přečíst (neplatný nebo poškozený STEP/IGES).".into(),
    });
    h.shot("11-error-box");
    h.app.msgs.clear();
    h.app.dlg.spacemouse = Some(crate::dialogs::SpaceMouseDlg::new(&h.app.settings.spacemouse));
    h.shot("12-spacemouse");
    h.app.dlg.spacemouse = None;
    h.frames(2);
    h.app.dlg.thumbs = Some(crate::dialogs::ThumbDlg::new());
    h.shot("13-thumbs");
    h.app.dlg.thumbs = None;
    h.frames(2);
    let dxf = h.write("vykres.dxf", testgen::dxf_drawing().as_bytes());
    h.open(&dxf);
    h.shot("20-drawing");
    h.key(Key::M);
    let at = |h: &H, w: [f64; 2]| {
        let r = h.app.r2();
        let s = r.view.to_screen(w, r.viewport());
        h.app.view_rect.min + egui::vec2(s[0] as f32, s[1] as f32)
    };
    h.mv(at(&h, [0.0, 0.0]));
    h.click(at(&h, [0.0, 0.0]));
    h.mv(at(&h, [120.0, 60.0]));
    h.click(at(&h, [200.0, 120.0]));
    h.shot("21-drawing-measure");
    h.key(Key::Escape);
    h.click(at(&h, [150.0 + 25.0, 60.0]));
    h.shot("22-drawing-picked");
    h.app.on_light_bg(true);
    h.settle();
    h.shot("23-drawing-light");
}

#[test]
fn a_file_from_google_drive_is_opened_from_its_local_copy() {
    use crate::drive::DriveEvent;
    use crate::flow::Event;
    let mut h = harness!();
    let local = h.write("1AbC_0123.step", testgen::step_assembly().as_bytes());
    // the download is announced: busy, with its progress text
    h.app.flow.token += 1;
    let token = h.app.flow.token;
    h.app.set_busy(true, "Otevírám soubor z Google Drive…");
    h.app.flow.thread_busy = true;
    h.app.drive = Some(crate::drive::DriveRun { cancel: cad_drive::CancelToken::new() });
    h.app
        .tx
        .send(Event::Drive(DriveEvent::Progress { token, progress: cad_drive::Progress::Downloading { done: 1024 * 1024, total: 5 * 1024 * 1024 } }))
        .unwrap();
    h.frames(2);
    assert_eq!(h.app.status.model, "Stahuji soubor z Google Drive: 1,0 MB z 5,0 MB");
    // progress of an older download is ignored
    h.app.tx.send(Event::Drive(DriveEvent::Progress { token: token - 1, progress: cad_drive::Progress::Finished })).unwrap();
    h.frames(2);
    assert_eq!(h.app.status.model, "Stahuji soubor z Google Drive: 1,0 MB z 5,0 MB");
    // the file arrives: it is shown under the name it has on the Drive
    let file = cad_drive::DriveFile {
        local_path: local.clone(),
        display_name: "Sestava z Drive.step".into(),
        file_id: "1AbC_0123".into(),
        size: 16448,
        modified_time: "2026-01-15T10:30:00.123Z".into(),
        from_cache: false,
    };
    h.app.tx.send(Event::Drive(DriveEvent::Done { token, result: Ok(file) })).unwrap();
    h.settle();
    assert_eq!(h.app.mode, Mode::ThreeD);
    assert_eq!(h.app.window_title(), "Sestava z Drive.step – CAD prohlížeč");
    assert!(h.app.status.model.starts_with("Sestava z Drive.step  |  4 dílů"), "{}", h.app.status.model);
    assert!(h.app.drive.is_none());
    // a failure is a message, a cancel is quiet
    h.app.flow.token += 1;
    let token = h.app.flow.token;
    h.app.set_busy(true, "…");
    h.app.flow.thread_busy = true;
    h.app.tx.send(Event::Drive(DriveEvent::Done { token, result: Err(cad_drive::DriveError::NotFound) })).unwrap();
    h.frames(3);
    assert!(!h.app.flow.busy);
    let m = h.app.msgs.front().expect("message");
    assert!(m.text.starts_with("Soubor z Google Drive se nepodařilo otevřít:"), "{}", m.text);
    assert!(m.text.contains("nebyl na Google Drive nalezen"));
    h.app.msgs.clear();
    h.app.flow.token += 1;
    let token = h.app.flow.token;
    h.app.set_busy(true, "…");
    h.app.flow.thread_busy = true;
    h.app.tx.send(Event::Drive(DriveEvent::Done { token, result: Err(cad_drive::DriveError::DownloadCancelled) })).unwrap();
    h.frames(3);
    assert!(h.app.msgs.is_empty() && !h.app.flow.busy);
}

#[test]
fn cancel_stops_a_download_and_ignores_late_results() {
    use crate::flow::Event;
    let mut h = harness!();
    h.app.flow.token += 1;
    let token = h.app.flow.token;
    h.app.set_busy(true, "…");
    h.app.flow.thread_busy = true;
    let cancel = cad_drive::CancelToken::new();
    h.app.drive = Some(crate::drive::DriveRun { cancel: cancel.clone() });
    h.app.cancel_load();
    assert!(cancel.is_cancelled());
    assert!(!h.app.flow.busy && !h.app.flow.thread_busy && h.app.drive.is_none());
    // the result of the cancelled download arrives later and is dropped
    let local = h.write("late.step", testgen::step_assembly().as_bytes());
    let file = cad_drive::DriveFile {
        local_path: local,
        display_name: "x.step".into(),
        file_id: "1AbC_0123".into(),
        size: 1,
        modified_time: String::new(),
        from_cache: false,
    };
    h.app.tx.send(Event::Drive(crate::drive::DriveEvent::Done { token, result: Ok(file) })).unwrap();
    h.frames(5);
    assert_eq!(h.app.mode, Mode::Empty);
}

// ------------------------------------------------------------------ interference check, numbers and portable mode

/// Waits until no interference check runs.
fn wait_for_check(h: &mut H) {
    let end = Instant::now() + Duration::from_secs(60);
    while h.app.dlg.collisions.as_ref().is_some_and(|d| d.running) {
        h.frames(1);
        std::thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() < end, "the check did not end");
    }
    h.frames(2);
}

#[test]
fn a_new_check_while_one_runs_is_not_a_dead_dialog() {
    use crate::collide::CollisionEvent;
    use std::sync::atomic::Ordering;
    let mut h = harness!();
    h.open_assembly();
    // the situation of a check that is still running: it holds the document (the thread of the real check does)
    let doc = h.app.model.as_mut().unwrap().doc.take().expect("document");
    let generation = h.app.flow.model_generation;
    let old = crate::collide::CollisionDlg::new("vybrané díly", vec![]);
    let old_stop = old.stop.clone();
    h.app.dlg.collisions = Some(old);
    // K again: the old dialog is replaced and its check is told to stop ...
    h.key(Key::K);
    assert!(old_stop.load(Ordering::SeqCst), "the replaced dialog stops its check");
    let d = h.app.dlg.collisions.as_ref().expect("a new dialog");
    // ... and the new one waits for the document instead of doing nothing
    assert!(d.queued && d.running, "the dialog waits for the document");
    assert_eq!(d.status, "Čekám na ukončení předchozí kontroly…");
    // the old check ends and gives the document back: the new check starts and its result is the one that is shown
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 999, doc, result: Err("zastaveno".into()) })).unwrap();
    h.frames(2);
    assert!(!h.app.dlg.collisions.as_ref().unwrap().queued, "the new check started");
    wait_for_check(&mut h);
    let d = h.app.dlg.collisions.as_ref().unwrap();
    assert!(!d.queued && !d.running);
    assert!(d.status.starts_with("Hotovo za "), "{}", d.status);
    assert!(h.app.model.as_ref().unwrap().doc.is_some(), "the document is back");
    // the answer of an older run does not change the dialog
    let doc = h.app.model.as_mut().unwrap().doc.take().unwrap();
    let before = h.app.dlg.collisions.as_ref().unwrap().status.clone();
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 0, doc, result: Err("starý".into()) })).unwrap();
    h.frames(2);
    assert_eq!(h.app.dlg.collisions.as_ref().unwrap().status, before);
    assert!(h.app.model.as_ref().unwrap().doc.is_some());
}

#[test]
fn closing_a_waiting_dialog_and_replacing_the_model_stop_the_check() {
    use std::sync::atomic::Ordering;
    let mut h = harness!();
    h.open_assembly();
    let doc = h.app.model.as_mut().unwrap().doc.take().expect("document");
    h.key(Key::K);
    assert!(h.app.dlg.collisions.as_ref().unwrap().queued);
    // closing the dialog while it waits: nothing starts when the document comes back
    h.app.close_collisions();
    assert!(h.app.dlg.collisions.is_none());
    h.app.model.as_mut().unwrap().doc = Some(doc);
    // a dialog that runs gets the stop flag when the model is replaced (reload, another file)
    h.key(Key::K);
    let stop = h.app.dlg.collisions.as_ref().expect("dialog").stop.clone();
    assert!(!stop.load(Ordering::SeqCst));
    h.app.clear_model();
    assert!(stop.load(Ordering::SeqCst), "clear_model stops the check");
    assert!(h.app.dlg.collisions.is_none());
}

#[test]
fn a_reload_stops_a_running_check() {
    use std::sync::atomic::Ordering;
    let mut h = harness!();
    h.open_assembly();
    h.key(Key::K);
    let stop = h.app.dlg.collisions.as_ref().expect("dialog").stop.clone();
    // F5 loads the file again and shows it: the check of the old model must not go on
    let path = h.app.model.as_ref().unwrap().path.clone();
    h.app.open_file(path, None);
    h.settle();
    assert!(stop.load(Ordering::SeqCst), "the reload stopped the check");
    assert!(h.app.dlg.collisions.is_none());
    assert_eq!(h.app.mode, Mode::ThreeD);
    // the new model has its document and a new check can start
    assert!(h.app.model.as_ref().unwrap().doc.is_some());
    h.key(Key::K);
    wait_for_check(&mut h);
    assert!(h.app.dlg.collisions.as_ref().unwrap().status.starts_with("Hotovo za "));
}

#[test]
fn volumes_asked_while_the_check_has_the_document_are_not_remembered_as_zero() {
    use crate::collide::CollisionEvent;
    let mut h = harness!();
    h.open_assembly();
    let plate = h.node("Deska");
    h.app.model.as_mut().unwrap().select_only([plate]);
    h.app.on_tree_selection();
    assert_eq!(h.row("Objem").unwrap(), "48,000 cm³");
    // volumes not known yet, then the check takes the document and the panel is built
    h.app.model.as_mut().unwrap().vol_cache.clear();
    let doc = h.app.model.as_mut().unwrap().doc.take().unwrap();
    h.app.refresh_props();
    assert_eq!(h.row("Objem").unwrap(), "0,000 cm³", "nothing can be measured now");
    assert!(h.app.model.as_ref().unwrap().vol_cache.is_empty(), "and the zero is not remembered");
    // the check ends: the panel is built again and shows the real volume
    let generation = h.app.flow.model_generation;
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 1, doc, result: Err("x".into()) })).unwrap();
    h.frames(2);
    assert_eq!(h.row("Objem").unwrap(), "48,000 cm³");
    // the mass panel (G) is refreshed the same way
    h.app.model.as_mut().unwrap().select_only([]);
    h.app.model.as_mut().unwrap().vol_cache.clear();
    let doc = h.app.model.as_mut().unwrap().doc.take().unwrap();
    h.app.show_mass();
    assert_eq!(h.row("Objem").unwrap(), "0,000 cm³");
    h.app.tx.send(crate::flow::Event::Collision(CollisionEvent::Done { model_generation: generation, run: 1, doc, result: Err("x".into()) })).unwrap();
    h.frames(2);
    assert!(h.row("Rozsah").is_some(), "still the mass panel");
    // the volume of the whole model: 48 000 + 2 x pi 4^2 30 + 18 000 mm3
    let all = {
        let m = h.app.model.as_mut().unwrap();
        let roots = m.tree.roots.clone();
        let leaves = m.unique_leaves(&roots);
        m.mass_totals(&leaves).volume / 1000.0
    };
    assert!((all - 69.016).abs() < 0.01, "{all}");
    assert_eq!(h.row("Objem").unwrap(), format!("{} cm³", cad_core::fmt::fmt_num(all, 3)));
}

#[test]
fn the_open_menu_does_not_ask_the_file_system_on_every_frame() {
    let mut h = harness!();
    let (a, b) = (h.write("a.step", b"x"), h.write("b.step", b"x"));
    h.app.settings.recent = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
    // the menu opens: the files are looked at
    assert_eq!(h.app.menu_recent().len(), 2);
    std::fs::remove_file(&b).unwrap();
    // it is drawn on every frame while it is open: the answer comes from the first look, not from the disk
    for _ in 0..5 {
        assert_eq!(h.app.menu_recent().len(), 2);
    }
    // a frame with the menu closed forgets it, so the next opening looks again
    assert!(h.app.recent_cache.is_some());
    h.frames(1);
    assert!(h.app.recent_cache.is_none(), "the closed menu keeps no list");
    assert_eq!(h.app.menu_recent(), vec![a.to_string_lossy().into_owned()]);
}

#[test]
fn portable_mode_keeps_the_original_name_the_material_key_and_the_folder() {
    let mut h = harness!();
    h.app.portable = true;
    // an STL is named after its file by the reader: that name is the tree name and the key of the materials
    let dir = h.dir.path().join("Google Drive");
    std::fs::create_dir_all(&dir).unwrap();
    let stl = dir.join("Kostka dílu.stl");
    std::fs::write(&stl, testgen::stepgen::box_stl([40.0, 30.0, 20.0])).unwrap();
    h.open(&stl);
    assert_eq!(h.app.mode, Mode::ThreeD);
    let m = h.app.model.as_ref().unwrap();
    let leaf = m.all_leaves()[0];
    assert_eq!(m.node(leaf).name, "Kostka dílu", "the tree shows the name of the file, not the name of the local copy");
    assert_eq!(m.proto_name_of(leaf), "Kostka dílu", "and so does the key of the material assignments");
    assert_eq!(m.path, stl);
    // the copy exists below the cache folder, under the same name
    let copy = std::fs::read_dir(h.app.sources_dir())
        .unwrap()
        .flatten()
        .find_map(|e| std::fs::read_dir(e.path()).ok()?.flatten().next())
        .expect("a local copy");
    assert_eq!(copy.file_name(), "Kostka dílu.stl");
    // a material assigned under that key is found again after a reload
    let mut a = cad_core::materials::Assignments::load(&h.app.data_dir, &stl).unwrap();
    a.set(&["Kostka dílu"], Some(("PLA", 1.24))).unwrap();
    h.app.open_file(stl.clone(), None);
    h.settle();
    let m = h.app.model.as_ref().unwrap();
    let leaf = m.all_leaves()[0];
    assert_eq!(m.material_of(leaf).map(|a| a.name), Some("PLA".to_string()));
    // a drawing: the panel names the folder and the size of the file that was opened, not those of the copy
    let dxf = dir.join("Výkres.dxf");
    std::fs::write(&dxf, testgen::dxf_drawing().as_bytes()).unwrap();
    h.open(&dxf);
    assert_eq!(h.app.mode, Mode::TwoD);
    assert_eq!(h.row("Složka").unwrap(), dir.to_string_lossy());
    assert_eq!(h.row("Velikost").unwrap(), crate::util::size_mb(std::fs::metadata(&dxf).unwrap().len()));
    assert_eq!(h.app.window_title(), "Výkres.dxf – CAD prohlížeč");
}

#[test]
fn portable_mode_draws_the_pictures_that_lie_next_to_the_original_drawing() {
    let mut h = harness!();
    h.app.portable = true;
    let dir = h.dir.path().join("Google Drive");
    std::fs::create_dir_all(&dir).unwrap();
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255])).save(dir.join("pic.png")).unwrap();
    let dxf = dir.join("Plán.dxf");
    std::fs::write(&dxf, testgen::dxf_with_picture("pic.png")).unwrap();
    h.open(&dxf);
    assert_eq!(h.app.mode, Mode::TwoD);
    // the drawing was read from the local copy ...
    let d = h.app.draw.as_ref().unwrap().drawing.clone();
    assert_ne!(d.info().path.parent(), Some(dir.as_path()), "a copy in the cache was loaded");
    // ... and its picture is the one next to the original
    let layout = d.default_layout();
    let (scene, _) = crate::scene2d::build(&d, &layout, false, true).unwrap();
    assert_eq!(scene.images.len(), 1);
}

// ---------------------------------------------------------------------- third review: sheets, highlight, cancel, long paths

impl H {
    /// A picture of the 2D view as the renderer draws it (the corner pixel is the background).
    fn view2d_picture(&self) -> cad_render::Rgba8 {
        let mut r = self.app.r2();
        let vp = r.viewport();
        r.capture_rgba(vp.width as u32, vp.height as u32, false).expect("picture of the 2D view")
    }

    /// Where the last frame has this text on the screen.
    fn shows_text(&self, text: &str) -> Option<Pos2> {
        let (shapes, _) = self.last.as_ref()?;
        shapes.iter().find_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == text => Some(t.pos),
            _ => None,
        })
    }

    /// Frames until the scene that is being built has arrived.
    fn settle_scene(&mut self) {
        let end = Instant::now() + Duration::from_secs(30);
        while self.app.flow.scene_busy {
            self.frames(1);
            std::thread::sleep(Duration::from_millis(5));
            assert!(Instant::now() < end, "the scene did not arrive");
        }
        self.frames(2);
    }
}

/// A pixel that is a pen (the lines are thin and drawn smoothed): nearly black, or nearly white.
fn has_pixel(img: &cad_render::Rgba8, rgb: [u8; 3]) -> bool {
    img.data.chunks_exact(4).any(|p| (0..3).all(|k| p[k].abs_diff(rgb[k]) < 40))
}

#[test]
fn a_sheet_is_drawn_on_white_and_the_white_option_does_not_build_it_again() {
    let mut h = harness!();
    let p = h.write("list.dxf", testgen::dxf_with_sheet().as_bytes());
    h.open(&p);
    const DARK: [u8; 3] = [0x21, 0x28, 0x30];
    const WHITE: [u8; 3] = [255, 255, 255];
    // the model space is on the dark background, with white pens
    assert_eq!(h.app.draw.as_ref().unwrap().layout, "Model");
    assert_eq!(h.app.r2().background, DARK);
    let img = h.view2d_picture();
    assert_eq!(img.pixel(2, 2)[..3], DARK);
    assert!(has_pixel(&img, WHITE), "white pens on the dark background");
    // the sheet: white paper with black pens, as 1.9 shows it (it was black on the dark background)
    h.app.on_layout_changed("Sheet A".to_string());
    h.settle_scene();
    assert_eq!(h.app.r2().background, WHITE);
    let img = h.view2d_picture();
    assert_eq!(img.pixel(2, 2)[..3], WHITE, "{:?}", img.pixel(2, 2));
    assert!(has_pixel(&img, [0, 0, 0]), "black pens");
    // the option changes nothing on a sheet, and nothing is built for it
    for on in [true, false] {
        h.app.on_light_bg(on);
        assert!(!h.app.flow.scene_busy, "no new scene for a sheet (option {on})");
        h.frames(2);
        assert_eq!(h.app.r2().background, WHITE);
        assert_eq!(h.view2d_picture().pixel(2, 2)[..3], WHITE);
    }
    // back to the model space: dark again, and white with the option, which only changes colours (no scene is built)
    h.app.on_layout_changed("Model".to_string());
    h.settle_scene();
    assert_eq!(h.app.r2().background, DARK);
    h.app.on_light_bg(true);
    assert!(!h.app.flow.scene_busy, "the pens that follow the background change colour, the scene is not built again");
    h.frames(2);
    assert_eq!(h.app.r2().background, WHITE);
    assert!(has_pixel(&h.view2d_picture(), [0, 0, 0]), "black pens on white");
    // a sheet stays white with the option on, and the model space is white after the way back
    h.app.on_layout_changed("Sheet A".to_string());
    h.settle_scene();
    assert_eq!(h.app.r2().background, WHITE);
    h.app.on_layout_changed("Model".to_string());
    h.settle_scene();
    assert_eq!(h.app.r2().background, WHITE);
    h.app.on_light_bg(false);
    assert!(!h.app.flow.scene_busy);
    h.frames(2);
    assert_eq!(h.app.r2().background, DARK);
}

#[test]
fn the_white_background_option_gives_the_picture_of_a_scene_that_was_made_with_it() {
    // one window opens the drawing and ticks the option, another opens it with the option on: the same picture, pixel for
    // pixel (the pens that follow the background were made white, then drawn black; the others were never touched)
    let mut a = harness!();
    let mut b = harness!();
    let src = testgen::dxf_drawing();
    let pa = a.write("vykres.dxf", src.as_bytes());
    let pb = b.write("vykres.dxf", src.as_bytes());
    a.open(&pa);
    let before = a.view2d_picture();
    let (center, scale) = {
        let r = a.app.r2();
        (r.view.center, r.view.scale)
    };
    let picked_before = a.app.draw.as_ref().unwrap().picked.is_none();
    a.app.on_light_bg(true);
    assert!(!a.app.flow.scene_busy, "no scene is built");
    a.frames(2);
    {
        let r = a.app.r2();
        assert_eq!((r.view.center, r.view.scale), (center, scale), "pan and zoom stay");
    }
    assert!(picked_before);
    let ticked = a.view2d_picture();
    assert_ne!(before.data, ticked.data, "the colours changed");
    b.app.settings.light_bg = true;
    b.open(&pb);
    let opened = b.view2d_picture();
    assert_eq!(ticked.pixel(2, 2), opened.pixel(2, 2));
    assert_eq!(ticked.data, opened.data, "the same picture");
    // and back
    a.app.on_light_bg(false);
    a.frames(2);
    assert_eq!(a.view2d_picture().data, before.data, "and back again");
    // the layers that are off in the file and a hidden layer are as they were
    let kote = a.app.draw.as_ref().unwrap().layers.iter().find(|l| l.name == "Kóty").unwrap().id;
    a.app.toggle_layer(kote, false);
    std::thread::sleep(Duration::from_millis(200));
    a.settle();
    a.app.on_light_bg(true);
    a.frames(2);
    assert!(!a.app.draw.as_ref().unwrap().visible(kote));
    assert!(a.app.r2().visible_bounds().unwrap().0[1] > -15.0, "the dimension line is still off");
}

#[test]
fn a_drawing_that_opens_on_its_sheet_is_white_from_the_start() {
    let mut h = harness!();
    let p = h.write("jen_list.dxf", testgen::dxf_sheet_only().as_bytes());
    h.open(&p);
    assert_eq!(h.app.draw.as_ref().unwrap().layout, "Sheet A");
    assert_eq!(h.app.r2().background, [255, 255, 255], "the default layout is the sheet");
    let img = h.view2d_picture();
    assert_eq!(img.pixel(2, 2)[..3], [255, 255, 255]);
    assert!(has_pixel(&img, [0, 0, 0]));
}

#[test]
fn the_highlight_of_a_pick_is_made_once_in_the_renderer_and_not_among_the_overlays() {
    let mut h = harness!();
    let p = h.write("vykres.dxf", testgen::dxf_drawing().as_bytes());
    h.open(&p);
    let at = |h: &H, w: [f64; 2]| {
        let r = h.app.r2();
        let s = r.view.to_screen(w, r.viewport());
        h.app.view_rect.min + egui::vec2(s[0] as f32, s[1] as f32)
    };
    h.click(at(&h, [150.0 + 25.0, 60.0]));
    assert_eq!(h.app.draw.as_ref().unwrap().picked.as_ref().map(|e| e.dxftype.as_str()), Some("CIRCLE"));
    {
        let r = h.app.r2();
        let (segments, buffers) = r.highlight_counts();
        assert!(segments > 20 && buffers == 1, "the circle is outlined in one buffer: {segments} {buffers}");
        // what is made for every frame holds nothing of it
        assert!(r.overlays().is_empty());
    }
    assert!(h.app.msgs.is_empty());
    // more frames: nothing is converted again (the overlays stay empty and the highlight is the same)
    h.frames(3);
    assert!(h.app.r2().overlays().is_empty());
    // another entity replaces it; empty space and the layer switches clear it
    h.click(at(&h, [0.0, 60.0]));
    assert!(h.app.r2().highlight_counts().0 >= 1);
    h.click(at(&h, [100.0, 100.0]));
    assert_eq!(h.app.r2().highlight_counts(), (0, 0));
    h.click(at(&h, [150.0 + 25.0, 60.0]));
    assert!(h.app.r2().highlight_counts().0 > 0);
    let kote = h.app.draw.as_ref().unwrap().layers.iter().find(|l| l.name == "Kóty").unwrap().id;
    h.app.toggle_layer(kote, false);
    std::thread::sleep(Duration::from_millis(200));
    h.settle();
    assert_eq!(h.app.r2().highlight_counts(), (0, 0));
    assert!(!h.app.flow.scene_busy, "a layer is a mask, not a new scene");
}

#[test]
fn a_device_that_refuses_the_highlight_gives_a_warning_and_the_window_goes_on() {
    let mut h = harness!();
    let p = h.write("vykres.dxf", testgen::dxf_drawing().as_bytes());
    h.open(&p);
    h.app.r2().set_simulated_buffer_limit(Some(16));
    let s = {
        let r = h.app.r2();
        r.view.to_screen([150.0 + 25.0, 60.0], r.viewport())
    };
    let pos = h.app.view_rect.min + egui::vec2(s[0] as f32, s[1] as f32);
    h.click(pos);
    assert!(h.app.draw.as_ref().unwrap().picked.is_some(), "the entity is picked");
    assert_eq!(h.app.r2().highlight_counts().0, 0, "its highlight did not fit");
    let warned: Vec<&str> = h.app.msgs.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(warned, [cad_render::gpubuf::GPU_DYNAMIC_WARNING], "{warned:?}");
    h.frames(3);
    assert_eq!(h.app.msgs.len(), 1, "said once");
}

#[test]
fn a_scene_being_built_shows_the_cancel_button_and_it_stops_the_build() {
    let mut h = harness!();
    let p = h.write("list.dxf", testgen::dxf_with_sheet().as_bytes());
    h.open(&p);
    h.frames(2);
    assert!(h.shows_text("Zrušit").is_none(), "nothing is being built");
    // a layout is asked for and its build is under way (the thread is not started: the state is what the frame shows)
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    h.app.flow.scene_cancel = Some(flag.clone());
    h.app.flow.scene_busy = true;
    h.app.flow.scene_generation += 1;
    h.app.draw.as_mut().unwrap().layout = "Sheet A".to_string();
    h.frames(2);
    assert!(!h.app.flow.busy, "this is not a load");
    let at = h.shows_text("Zrušit").expect("the status bar has the cancel button");
    h.click(at + egui::vec2(6.0, 6.0));
    assert!(flag.load(std::sync::atomic::Ordering::Relaxed), "the build is told to stop");
    assert!(!h.app.flow.scene_busy && h.app.flow.scene_cancel.is_none());
    // the screen still has the model space, and the list of layouts says so again
    assert_eq!(h.app.draw.as_ref().unwrap().layout, "Model");
    h.frames(2);
    assert!(h.shows_text("Zrušit").is_none());
    assert_eq!(h.app.r2().background, [0x21, 0x28, 0x30]);
    // a build that is cancelled when its answer is already on the way: the answer is dropped by its generation
    let generation = h.app.flow.scene_generation;
    h.app.on_layout_changed("Sheet A".to_string());
    assert!(h.app.flow.scene_busy && h.app.flow.scene_generation > generation);
    h.app.cancel_scene_rebuild();
    assert_eq!(h.app.draw.as_ref().unwrap().layout, "Model");
    std::thread::sleep(Duration::from_millis(300));
    h.frames(3);
    assert_eq!(h.app.r2().background, [0x21, 0x28, 0x30], "the scene that is shown is still the model space");
}

#[cfg(windows)]
mod long_paths {
    use super::*;

    /// A folder below the temporary folder of the harness that makes `file` 270 characters long or more.
    fn long_dir(h: &H, file: &str) -> PathBuf {
        let mut dir = h.dir.path().to_path_buf();
        let mut n = 0;
        while crate::flow::path_chars(&dir.join(file)) < 270 {
            n += 1;
            dir.push(format!("{n:02}-slozka-s-dlouhym-nazvem-kolik-je-to-znaku"));
        }
        std::fs::create_dir_all(&dir).expect("long folder");
        assert!(crate::flow::path_chars(&dir.join(file)) >= 260);
        dir
    }

    fn messages(h: &H) -> Vec<String> {
        h.app.msgs.iter().map(|m| m.text.clone()).collect()
    }

    #[test]
    fn a_step_with_a_path_over_260_characters_is_loaded_through_a_local_copy() {
        let mut h = harness!();
        let dir = long_dir(&h, "sestava.step");
        let p = dir.join("sestava.step");
        std::fs::write(&p, testgen::step_assembly().as_bytes()).unwrap();
        assert!(crate::flow::needs_local_copy(&p));
        h.app.open_file(p.clone(), None);
        h.settle();
        assert_eq!(h.app.mode, Mode::ThreeD, "{:?}", messages(&h));
        assert!(h.app.msgs.is_empty(), "{:?}", messages(&h));
        let m = h.app.model.as_ref().unwrap();
        // the model is shown under the file that was opened
        assert_eq!(m.path, std::path::absolute(&p).unwrap());
        assert_eq!(h.app.window_title(), "sestava.step – CAD prohlížeč");
        assert_eq!(h.row("Dílů").unwrap(), "4 (3 unikátních)");
        // the copy is in the cache folder, below the short path of the program
        let copy = std::fs::read_dir(h.app.sources_dir()).unwrap().flatten().find_map(|e| std::fs::read_dir(e.path()).ok()?.flatten().next()).expect("a local copy");
        assert!(crate::flow::path_chars(&copy.path()) < 260, "{}", copy.path().display());
    }

    #[test]
    fn an_stl_with_a_long_path_and_a_long_name_keeps_its_name_in_the_tree() {
        let mut h = harness!();
        // the name is longer than the 100 characters a copy keeps: the copy is called `<key>.stl`
        let stem = format!("Kostka-{}", "x".repeat(110));
        let file = format!("{stem}.stl");
        let dir = long_dir(&h, &file);
        let p = dir.join(&file);
        std::fs::write(&p, testgen::stepgen::box_stl([40.0, 30.0, 20.0])).unwrap();
        h.app.open_file(p.clone(), None);
        h.settle();
        assert_eq!(h.app.mode, Mode::ThreeD, "{:?}", messages(&h));
        let m = h.app.model.as_ref().unwrap();
        let leaf = m.all_leaves()[0];
        assert_eq!(m.node(leaf).name, stem, "the tree shows the name of the file that was opened, not that of the copy");
        assert_eq!(m.path, std::path::absolute(&p).unwrap());
        assert_eq!(h.app.window_title(), format!("{file} – CAD prohlížeč"));
        // its sharp edges come from the analysis, which reads the copy as well
        let end = Instant::now() + Duration::from_secs(30);
        while h.app.model.as_ref().is_some_and(|m| m.stl.is_none()) {
            h.frames(1);
            std::thread::sleep(Duration::from_millis(10));
            assert!(Instant::now() < end, "mesh analysis of the copy");
        }
    }

    /// Folders of short copies in the temporary folder.
    fn leftover_copies() -> usize {
        std::fs::read_dir(std::env::temp_dir()).map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("thumb_in_")).count()).unwrap_or(0)
    }

    #[test]
    fn the_thumbnail_of_a_model_with_a_long_path_is_made_and_leaves_no_copy_behind() {
        let h = harness!();
        for (name, bytes) in [("sestava.step", testgen::step_assembly().into_bytes()), ("kostka.stl", testgen::stepgen::box_stl([40.0, 30.0, 20.0]))] {
            let dir = long_dir(&h, name);
            let src = dir.join(name);
            std::fs::write(&src, bytes).unwrap();
            let out = h.dir.path().join(format!("{name}.png"));
            let before = leftover_copies();
            assert_eq!(crate::thumbnail::run(&src, &out, "64"), 0, "{name}");
            assert!(std::fs::metadata(&out).map(|m| m.len() > 100).unwrap_or(false), "{name}: a PNG");
            assert_eq!(leftover_copies(), before, "the short copy was removed");
        }
    }
}

#[test]
fn the_status_bar_shows_the_note_of_a_drawing_that_is_drawn_with_less_detail() {
    let mut h = harness!();
    let p = h.write("vykres.dxf", testgen::dxf_drawing().as_bytes());
    h.open(&p);
    // a drawing at full detail has no note
    assert!(h.app.status.note.is_empty());
    assert!(h.shows_text(crate::strings::STATUS_REDUCED_DETAIL).is_none());
    // with one the note is painted in the status bar, at the bottom of the window, and goes when the note goes
    h.app.status.note = crate::strings::STATUS_REDUCED_DETAIL.to_string();
    h.frames(2);
    let at = h.shows_text(crate::strings::STATUS_REDUCED_DETAIL).expect("the note in the frame");
    assert!(at.y > 860.0, "the status bar is the lowest line of the window: {at:?}");
    h.app.status.note.clear();
    h.frames(2);
    assert!(h.shows_text(crate::strings::STATUS_REDUCED_DETAIL).is_none());
}
