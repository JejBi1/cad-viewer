//! Curved geometry, IGES, placed instances and the crash guard.
use cad_types::{EntityKind, EntityRef};

use super::*;
use crate::stepgen::{assembly, cylinder, iges_line, part, usage};
use crate::tests::{
    BOX, TempDir, big_stack, close, close3, cross3, dot3, entity_of, face_ref, read, read_ok, row,
    sub3, write_step,
};

#[test]
fn cylinder_meshes_smoothly_and_measures_circles() {
    big_stack(|| {
        let dir = TempDir::new("cyl");
        let path = write_step(&dir, "cyl.step", &[cylinder("Válec", 5.0, 12.0)]);
        let doc = read_ok(&path);
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes[0].name, "Válec");

        let mass = doc.mass_props(0);
        let pi = std::f64::consts::PI;
        assert!(close(mass.volume_mm3, pi * 25.0 * 12.0, 1e-6), "{mass:?}");
        assert!(close3(mass.centroid, [0.0, 0.0, 6.0], 1e-6), "{mass:?}");
        assert_eq!((mass.solids, mass.faces), (1, 3));
        // the area comes from the display mesh: slightly below the exact 2 pi r (r + h)
        let exact = 2.0 * pi * 5.0 * (5.0 + 12.0);
        assert!(
            mass.area_mm2 < exact && mass.area_mm2 > exact * 0.99,
            "{} vs {exact}",
            mass.area_mm2
        );

        let m = doc.proto_mesh(0);
        assert_eq!((m.faces.len(), m.edges.len(), m.vertices.len()), (3, 3, 2));
        assert!(m.faces.iter().all(|f| f.index_count >= 6));
        // outward winding and smooth normals: radial on the side face, +/- z on the caps
        let mut radial = 0;
        for f in &m.faces {
            for tri in m.indices[f.first_index as usize..(f.first_index + f.index_count) as usize]
                .chunks(3)
            {
                let (a, b, c) = (
                    m.positions[tri[0] as usize],
                    m.positions[tri[1] as usize],
                    m.positions[tri[2] as usize],
                );
                let n = cross3(sub3(b, a), sub3(c, a));
                for &i in tri {
                    let (p, vn) = (m.positions[i as usize], m.normals[i as usize]);
                    assert!((dot3(vn, vn) - 1.0).abs() < 1e-4);
                    assert!(dot3(vn, n) > 0.0, "normal disagrees with the winding");
                    if vn[2].abs() < 0.5 {
                        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
                        assert!((r - 5.0).abs() < 1e-3);
                        assert!(
                            (vn[0] * p[0] + vn[1] * p[1]) / r > 0.999,
                            "smooth radial normal: {vn:?} at {p:?}"
                        );
                        radial += 1;
                    }
                }
            }
        }
        assert!(radial > 20, "side face has many vertices: {radial}");
        // circles are polylines on the circle
        let circles: Vec<_> = m.edges.iter().filter(|e| e.point_count > 4).collect();
        assert_eq!(circles.len(), 2);
        for e in circles {
            for p in
                &m.edge_points[e.first_point as usize..(e.first_point + e.point_count) as usize]
            {
                assert!(((p[0] * p[0] + p[1] * p[1]).sqrt() - 5.0).abs() < 1e-3);
            }
        }

        // entities: two circles and the seam line
        let mut circle_edges = Vec::new();
        let mut line_edge = None;
        for i in 0..3 {
            let r = EntityRef {
                node: 0,
                kind: EntityKind::Edge,
                index: i,
            };
            let e = entity_of(&doc, r);
            if e.type_name == "kružnice" {
                circle_edges.push((r, e));
            } else {
                assert_eq!(e.type_name, "úsečka");
                line_edge = Some(e);
            }
        }
        assert_eq!(circle_edges.len(), 2);
        assert!(close(line_edge.unwrap().length.unwrap(), 12.0, 1e-9));
        let (c0, e0) = &circle_edges[0];
        assert!(
            close(e0.radius.unwrap(), 5.0, 1e-12)
                && close(e0.length.unwrap(), 2.0 * pi * 5.0, 1e-9)
        );
        assert!(close(e0.arc.unwrap(), 360.0, 1e-9));
        let labels: Vec<String> = e0.rows().iter().map(|r| r.label.clone()).collect();
        assert_eq!(
            labels,
            [
                "Entita",
                "Délka",
                "Poloměr R",
                "Průměr Ø",
                "Střed",
                "Osa",
                "Začátek",
                "Konec"
            ]
        );
        assert_eq!(e0.short(), "hrana: kružnice  Ø 10,000  R 5,000  L 31,416");
        let m2 = doc.measure(&[*c0, circle_edges[1].0]);
        assert_eq!(
            row(&m2, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("12,000 mm")
        );
        assert_eq!(
            row(&m2, "Vzdálenost středů").map(|r| r.value.as_str()),
            Some("12,000 mm")
        );
        assert_eq!(row(&m2, "Úhel").map(|r| r.value.as_str()), Some("0,000°"));
        assert_eq!(
            m2.rows[0],
            MeasureRow::new("Entita 1", "hrana – kružnice, Ø 10,00, délka 31,42")
        );

        // faces: the cylinder (its axis is a line) and the top plane (its axis is a normal)
        let mut side = None;
        let mut top = None;
        for i in 0..3 {
            let r = face_ref(0, i);
            let e = entity_of(&doc, r);
            match e.type_name.as_str() {
                "válec" => side = Some((r, e)),
                "rovina" if close(e.centroid.unwrap()[2], 12.0, 1e-6) => top = Some(r),
                _ => {}
            }
        }
        let (side_ref, side_e) = side.expect("cylinder face");
        assert!(side_e.is_line && close(side_e.radius.unwrap(), 5.0, 1e-12));
        assert!(close3(side_e.axis.unwrap(), [0.0, 0.0, 1.0], 1e-12));
        assert!(close(side_e.area.unwrap(), 2.0 * pi * 5.0 * 12.0, 1e-6));
        assert!(
            side_e
                .rows()
                .iter()
                .any(|r| r.label == "Osa" && r.value == "(0,0000; 0,0000; 1,0000)")
        );
        let m3 = doc.measure(&[side_ref, top.expect("top plane")]);
        assert_eq!(
            row(&m3, "Úhel (přímka – rovina)").map(|r| r.value.as_str()),
            Some("90,000°")
        );
        // a single cylinder: status text and 3D label
        let m4 = doc.measure(&[side_ref]);
        assert_eq!(
            m4.status.as_deref(),
            Some("válec: plocha 376,99 mm², Ø 10,00 / R 5,00 mm")
        );
        assert_eq!(m4.label.map(|l| l.1), Some("Ø 10,00".to_string()));
        // the face normal on the curved face at a world point
        let n = doc
            .face_normal_at(side_ref, [0.0, 5.0, 3.0])
            .expect("normal");
        assert!(close3(n, [0.0, 1.0, 0.0], 1e-9), "{n:?}");
        // the edge polylines survive the XBF round trip, too
        let xbf = dir.file("cyl.xbf");
        doc.save_xbf(&xbf).unwrap();
        let loaded = Document::load_xbf(&xbf).unwrap();
        let m5 = loaded.proto_mesh(0);
        assert_eq!(
            m5.edges.iter().map(|e| e.point_count).collect::<Vec<_>>(),
            m.edges.iter().map(|e| e.point_count).collect::<Vec<_>>()
        );
    });
}

#[test]
fn iges_line_is_read_with_the_edge_fallback() {
    big_stack(|| {
        let dir = TempDir::new("iges");
        let path = dir.file("cára.IGS");
        std::fs::write(&path, iges_line()).unwrap();
        let (doc, log) = read(&path);
        let doc = doc.expect("read iges");
        assert_eq!(
            log,
            [
                "Čtu soubor…",
                "Převádím geometrii…",
                "Kontroluji strukturu sestavy…",
                "Síťuji 1 těles…"
            ]
        );
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 1);
        let m = doc.proto_mesh(0);
        assert!(m.faces.is_empty() && m.indices.is_empty());
        assert_eq!((m.edges.len(), m.vertices.len()), (1, 2));
        assert!(m.edges[0].point_count >= 2);
        let first = m.edge_points[m.edges[0].first_point as usize];
        let last = m.edge_points[(m.edges[0].first_point + m.edges[0].point_count - 1) as usize];
        let ends = [first, last];
        assert!(
            ends.iter().any(|p| p[0].abs() < 1e-5)
                && ends.iter().any(|p| (p[0] - 10.0).abs() < 1e-5),
            "{ends:?}"
        );
        let e = entity_of(
            &doc,
            EntityRef {
                node: 0,
                kind: EntityKind::Edge,
                index: 0,
            },
        );
        assert_eq!(e.type_name, "úsečka");
        assert!(close(e.length.unwrap(), 10.0, 1e-9));
        let b = doc.node_bounds(0).unwrap();
        // the box is enlarged by the mesh deflection (AddOptimal with triangulation), 0.011 for this size class
        assert!(
            close3(b.max, [10.0, 0.0, 0.0], 0.02) && close3(b.min, [0.0; 3], 0.02),
            "{b:?}"
        );
    });
}

#[test]
fn placed_instances_measure_in_world_coordinates() {
    big_stack(|| {
        let dir = TempDir::new("placed");
        let path = write_step(
            &dir,
            "placed.step",
            &[
                part("Kostka", BOX),
                assembly(
                    "Sestava",
                    vec![
                        usage(0, "L", [0.0, 0.0, 0.0]),
                        usage(0, "P", [20.0, 5.0, 0.0]),
                    ],
                ),
            ],
        );
        let doc = read_ok(&path);
        // node ids and entities work without having asked for the tree first
        let top_of = |node: u32| {
            (0..6)
                .map(|i| face_ref(node, i))
                .find(|r| close(entity_of(&doc, *r).centroid.unwrap()[2], 30.0, 1e-6))
                .expect("top face")
        };
        let (l, p) = (top_of(1), top_of(2));
        let pe = entity_of(&doc, p);
        assert!(
            close3(pe.centroid.unwrap(), [25.0, 15.0, 30.0], 1e-6),
            "{:?}",
            pe.centroid
        );
        let m = doc.measure(&[l, p]);
        // box 1 spans x 0..10, y 0..20; box 2 spans x 20..30, y 5..25: the closest points are 10 mm apart in x
        assert_eq!(
            row(&m, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("10,000 mm"),
            "{:?}",
            m.rows
        );
        assert_eq!(
            row(&m, "Vzdálenost rovin").map(|r| r.value.as_str()),
            Some("0,000 mm")
        );
        // the same vertex of two instances differs by the placement
        let v = |node| {
            doc.entity(&MeasureItem::Entity(EntityRef {
                node,
                kind: EntityKind::Vertex,
                index: 0,
            }))
            .unwrap()
            .point
            .unwrap()
        };
        let (a, b) = (v(1), v(2));
        assert!(close3(
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [20.0, 5.0, 0.0],
            1e-9
        ));
    });
}

#[test]
fn a_crash_inside_occt_becomes_an_error() {
    big_stack(|| {
        // Not an XBF file at all: OCCT dereferences a bad pointer while reading it. The public entry point refuses
        // such a file up front; calling the raw layer shows that the process survives even without that check.
        let dir = TempDir::new("guard");
        let junk = dir.file("junk.xbf");
        std::fs::write(&junk, "not an xbf").unwrap();
        let mut status = String::new();
        match raw::load_xbf(junk.to_str().unwrap(), &mut status) {
            Err(e) => assert!(e.what().contains("neplatný přístup do paměti"), "{e}"),
            Ok(_) => assert!(!status.contains("OK"), "{status}"),
        }
    });
}

/// Fault points of the C++ side (`bridge.h`). A fault is injected for the thread that makes the call.
const FAULT_FACE_NORMAL: u32 = 1;
const FAULT_READ_STL: u32 = 2;

/// Runs `f` on a thread of its own and fails when it does not end in `secs` (a lock that was left locked).
fn within<T: Send + 'static>(secs: u64, what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let _ = tx.send(f());
        })
        .unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("{what}: no answer in {secs} s"))
}

#[test]
fn a_hardware_fault_while_the_normal_of_a_face_is_found_is_no_normal_and_not_the_end_of_the_process() {
    big_stack(|| {
        let dir = TempDir::new("normal-fault");
        let path = dir.file("box.step");
        std::fs::write(&path, crate::stepgen::box_step("Box", BOX)).unwrap();
        let doc = read_ok(&path);
        let _ = doc.tree().unwrap();
        let top = (0..6)
            .find(|&i| close(entity_of(&doc, face_ref(0, i)).centroid.unwrap()[2], 30.0, 1e-6))
            .unwrap();
        let ask = |doc: &Document| doc.face_normal_at(face_ref(0, top), [5.0, 10.0, 30.0]);
        assert!(ask(&doc).is_some());
        // an access violation inside OCCT: the guard turns it into an error, which reads as "no normal"
        crate::inject_fault(FAULT_FACE_NORMAL);
        assert!(ask(&doc).is_none());
        // it was a fault of one call only: the next call works
        let n = ask(&doc).expect("the normal after the fault");
        assert!(close3(n, [0.0, 0.0, 1.0], 1e-9), "{n:?}");
    });
}

#[test]
fn a_hardware_fault_while_an_stl_is_read_leaves_no_lock_behind() {
    let dir = TempDir::new("stl-fault");
    let path = dir.file("box.stl");
    std::fs::write(&path, crate::stepgen::box_stl(BOX)).unwrap();
    let p = path.to_str().unwrap().to_string();
    // the fault comes while the document is made under the lock of the application: an error
    let p2 = p.clone();
    let e = within(30, "read_stl with a fault", move || {
        crate::inject_fault(FAULT_READ_STL);
        raw::read_stl(&p2, "Box").err().map(|e| e.what().to_string())
    });
    let e = e.expect("an error, not a document");
    assert!(e.contains("neplatný přístup do paměti"), "{e}");
    // the lock is free again: documents are made and files are read afterwards, on other threads as well
    within(30, "new_doc after a fault", || assert!(raw::new_doc().is_ok()));
    let read = within(30, "read_stl after a fault", move || raw::read_stl(&p, "Box").map(|d| !d.is_null()));
    assert!(read.expect("read"), "the STL is read once the fault is over");
}

#[test]
fn unnamed_instances_fall_back_to_the_prototype_name() {
    big_stack(|| {
        let dir = TempDir::new("names");
        let path = write_step(
            &dir,
            "names.step",
            &[
                part("Šroub", BOX),
                assembly(
                    "Sestava",
                    vec![
                        usage(0, "", [0.0, 0.0, 0.0]),
                        usage(0, "Pojmenovaný", [20.0, 0.0, 0.0]),
                    ],
                ),
            ],
        );
        let doc = read_ok(&path);
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 3);
        // core.py: the instance name is used unless it is empty or starts with "=>[" - then the prototype name
        assert_eq!(tree.nodes[1].name, "Šroub");
        assert_eq!(tree.nodes[2].name, "Pojmenovaný");
        assert!(
            tree.nodes
                .iter()
                .all(|n| !n.name.starts_with("=>[") && !n.name.is_empty())
        );
        assert!(
            tree.nodes
                .iter()
                .all(|n| n.proto_name == "Šroub" || n.is_assembly)
        );
    });
}

#[test]
fn a_part_without_any_name_is_called_bez_nazvu() {
    big_stack(|| {
        let dir = TempDir::new("noname");
        let path = write_step(&dir, "noname.step", &[part("", BOX)]);
        let doc = read_ok(&path);
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes[0].proto_name, "");
        assert_eq!(tree.nodes[0].name, MSG_NO_NAME);
    });
}

#[test]
fn many_instances_build_quickly() {
    big_stack(|| {
        let dir = TempDir::new("many");
        let n = 400;
        let usages = (0..n)
            .map(|i| {
                usage(
                    0,
                    &format!("K{i}"),
                    [(i % 20) as f64 * 15.0, (i / 20) as f64 * 25.0, 0.0],
                )
            })
            .collect();
        let path = write_step(
            &dir,
            "many.step",
            &[part("Kostka", BOX), assembly("Hodně", usages)],
        );
        let t = std::time::Instant::now();
        let doc = read_ok(&path);
        let tree = doc.tree().unwrap();
        assert_eq!(
            tree.nodes.len(),
            n + 1,
            "the assembly and one node per instance"
        );
        assert_eq!(tree.proto_instances, vec![n as u32]);
        assert_eq!(tree.nodes[0].children.len(), n);
        assert_eq!(tree.nodes[n].name, format!("K{}", n - 1));
        let b = doc.node_bounds(0).unwrap();
        assert!(
            close3(b.max, [19.0 * 15.0 + 10.0, 19.0 * 25.0 + 20.0, 30.0], 0.05),
            "{b:?}"
        );
        // boxes are 15 apart in x (10 wide) and 25 apart in y (20 deep): nothing collides
        let all: Vec<u32> = (1..=n as u32).collect();
        let report = doc.interferences_ex(&all, false, &mut |_| true).unwrap();
        assert!(
            report.results.is_empty() && report.part_pairs == 0,
            "{report:?}"
        );
        assert!(t.elapsed().as_secs() < 60, "took {:?}", t.elapsed());
    });
}
