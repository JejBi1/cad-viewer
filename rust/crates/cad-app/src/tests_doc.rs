//! Tests with real documents: a generated STEP assembly goes through the loader's preparation, the model state, the
//! numbers of the panel, the collision rows and the thumbnail role.
use std::path::{Path, PathBuf};

use cad_core::materials::Assignments;
use occt_bridge::Document;

use crate::flow::prepare_model;
use crate::model3::{Check, Loaded3d, Model3};
use crate::props::Props;
use crate::testgen;
use crate::util::run_big;

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// Loads a file the way the loader thread does.
fn load(path: PathBuf) -> Box<Loaded3d> {
    run_big("test-load", move || {
        let doc = Document::read(&path, &mut |_| {}).expect("read");
        prepare_model(doc, false).expect("prepare")
    })
    .expect("thread")
}

fn assembly_model(dir: &Path) -> Model3 {
    let path = write(dir, "sestava.step", testgen::step_assembly().as_bytes());
    let loaded = load(path.clone());
    // a material file in the temp folder, never in the user's cache
    let mats = Assignments::load(dir, &path).ok();
    Model3::new(path, None, *loaded, mats)
}

fn value(p: &Props, label: &str) -> Option<String> {
    match p {
        Props::Rows { rows, .. } => rows.iter().find(|r| r.label == label).map(|r| r.value_text()),
        Props::Welcome => None,
    }
}

#[test]
fn assembly_gives_tree_meshes_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load(write(dir.path(), "a.step", testgen::step_assembly().as_bytes()));
    // the assembly, the plate, two bolts, the block
    assert_eq!(loaded.tree.nodes.len(), 5);
    assert_eq!(loaded.tree.roots.len(), 1);
    assert_eq!(loaded.protos.len(), 3);
    assert!(loaded.protos.iter().all(|p| !p.positions.is_empty() && !p.faces.is_empty()));
    assert_eq!(loaded.tree.proto_instances.iter().sum::<u32>(), 4);
    assert_eq!(loaded.tree.proto_instances.iter().filter(|c| **c == 2).count(), 1);
}

#[test]
fn model_state_follows_the_check_boxes_and_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = assembly_model(dir.path());
    m.rebuild_rows("");
    // the top level is open: the assembly and its four parts
    assert_eq!(m.rows.len(), 5);
    assert_eq!(m.info_text(m.tree.roots[0]), "4 ks");
    let bolt = m.tree.nodes.iter().find(|n| n.name == "Šroub 1").expect("bolt").id;
    assert_eq!(m.info_text(bolt), "×2");
    assert_eq!(m.tooltip(bolt), format!("Instance: Šroub 1\nDíl: {}\nPoužit 2× v modelu", m.node(bolt).proto_name));
    assert!(m.set_checked(&[bolt], false));
    assert_eq!(m.check(m.tree.roots[0]), Check::Partial);
    assert!(m.is_visible_leaf(m.tree.nodes.iter().find(|n| n.name == "Deska").unwrap().id));
    assert!(!m.is_visible_leaf(bolt));
    // the filter finds parts by name, case-insensitively, and keeps their assembly
    m.rebuild_rows("deska");
    assert_eq!(m.rows.len(), 2);
}

#[test]
fn panel_numbers_of_a_part_and_of_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut m = assembly_model(dir.path());
    let plate = m.tree.nodes.iter().find(|n| n.name == "Deska").expect("plate").id;
    let p = m.node_props(plate);
    assert_eq!(value(&p, "Název").unwrap(), "Deska");
    assert_eq!(value(&p, "Typ").unwrap(), "díl");
    assert_eq!(value(&p, "Rozměr X×Y×Z").unwrap(), "100,00 × 60,00 × 8,00 mm");
    // volume 100 x 60 x 8 = 48 000 mm3 = 48 cm3, area 2 (6000 + 800 + 480) = 14 560 mm2
    assert_eq!(value(&p, "Objem").unwrap(), "48,000 cm³");
    assert_eq!(value(&p, "Povrch").unwrap(), "≈ 145,60 cm²");
    assert_eq!(value(&p, "Materiál").unwrap(), "bez materiálu (přiřadit…)");
    assert_eq!(value(&p, "Hmotnost (ocel 7,85)").unwrap(), "0,377 kg");
    assert_eq!(value(&p, "Těles / ploch").unwrap(), "1 / 6");
    // the whole model: 48 000 + 2 x pi 4^2 30 + 18 000 mm3
    let all = m.all_leaves();
    let t = m.mass_totals(&all);
    assert!((t.volume - (48000.0 + 2.0 * std::f64::consts::PI * 16.0 * 30.0 + 18000.0)).abs() < 1.0, "{}", t.volume);
    assert_eq!(t.missing, 4);
    // an assigned material turns the estimate into a mass
    let plate_name = m.proto_name_of(plate);
    m.mats.as_mut().unwrap().set(&[plate_name.as_str()], Some(("Ocel (konstrukční)", 7.85))).unwrap();
    let p = m.node_props(plate);
    assert_eq!(value(&p, "Hmotnost").unwrap(), "0,3768 kg");
    assert_eq!(value(&p, "Materiál").unwrap(), "Ocel (konstrukční) (přiřadit…)");
    assert_eq!(m.info_text(plate), "Ocel");
    let center = value(&p, "Těžiště X; Y; Z").unwrap();
    assert_eq!(center, "50,00; 30,00; 4,00 mm");
    // info panel without a selection
    let info = m.model_info(1.0);
    assert!(value(&info, "Rozměr X×Y×Z").unwrap().starts_with("100,00 × 60,00 × "));
    assert_eq!(value(&info, "Dílů").unwrap(), "4 (3 unikátních)");
}

#[test]
fn assigned_materials_survive_in_their_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "m.step", testgen::step_assembly().as_bytes());
    {
        let mut a = Assignments::load(dir.path(), &path).unwrap();
        a.set(&["Deska"], Some(("Hliník (slitiny)", 2.7))).unwrap();
    }
    let loaded = load(path.clone());
    let m = Model3::new(path.clone(), None, *loaded, Assignments::load(dir.path(), &path).ok());
    let plate = m.tree.nodes.iter().find(|n| n.name == "Deska").unwrap().id;
    let a = m.material_of(plate).expect("material");
    assert_eq!((a.name.as_str(), a.density, a.explicit), ("Hliník (slitiny)", 2.7, true));
    assert!(m.material_of(m.tree.nodes.iter().find(|n| n.name == "Blok").unwrap().id).is_none());
}

#[test]
fn stl_loads_as_one_mesh_part() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "kostka.stl", &testgen::stepgen::box_stl([40.0, 30.0, 20.0]));
    let loaded = load(path);
    assert_eq!(loaded.tree.nodes.iter().filter(|n| !n.is_assembly).count(), 1);
    assert!(loaded.protos[0].mesh_only);
}

#[test]
fn thumbnail_role_writes_a_png_for_step_stl_and_dxf() {
    // needs a graphics adapter (hardware or the software one); without one the role exits with 1 and the test says so
    if cad_render::GpuContext::new_headless().is_err() {
        eprintln!("SKIP: no graphics adapter");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let step = write(dir.path(), "sestava.step", testgen::step_assembly().as_bytes());
    let stl = write(dir.path(), "kostka.stl", &testgen::stepgen::box_stl([40.0, 30.0, 20.0]));
    let dxf = write(dir.path(), "vykres.dxf", testgen::dxf_drawing().as_bytes());
    for (src, size) in [(&step, "128"), (&stl, "64"), (&dxf, "128")] {
        let out = dir.path().join(format!("{}.png", src.file_stem().unwrap().to_string_lossy()));
        assert_eq!(crate::thumbnail::run(src, &out, size), 0, "{}", src.display());
        let img = image::open(&out).expect("png").to_rgba8();
        let n: u32 = size.parse().unwrap();
        assert_eq!(img.dimensions(), (n, n));
        // something was drawn, and the corners of 3D thumbnails are transparent
        assert!(img.pixels().any(|p| p.0[3] > 0));
        if src != &dxf {
            assert_eq!(img.get_pixel(0, 0).0[3], 0);
        }
    }
}
