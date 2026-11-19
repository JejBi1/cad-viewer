//! Integration tests: real STEP / STL files written by the tests, read through OCCT.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use cad_types::{EntityKind, EntityRef, IDENTITY, MassProps, ProtoMesh};

use super::*;
use crate::stepgen::{self, Product, assembly, box_step, box_stl, part, usage};

// ------------------------------------------------------------------ helpers

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "occt-bridge-test-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&p).expect("temp dir");
        TempDir(p)
    }
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// OCCT recurses deeply on nested assemblies; the default test thread stack is small.
pub(crate) fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("test thread panicked")
}

pub(crate) fn read(path: &Path) -> (Result<Document, BridgeError>, Vec<String>) {
    let mut log = Vec::new();
    let r = Document::read(path, &mut |m| log.push(m.to_string()));
    (r, log)
}

pub(crate) fn read_ok(path: &Path) -> Document {
    let (r, _) = read(path);
    r.unwrap_or_else(|e| panic!("read failed: {e}"))
}

pub(crate) fn write_step(dir: &TempDir, name: &str, products: &[Product]) -> PathBuf {
    let p = dir.file(name);
    std::fs::write(&p, stepgen::write_step(products)).expect("write step");
    p
}

pub(crate) fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
pub(crate) fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}
pub(crate) fn close3(a: [f64; 3], b: [f64; 3], tol: f64) -> bool {
    (0..3).all(|i| close(a[i], b[i], tol))
}

/// Outward-pointing normals and winding, 6 faces of 2 triangles, 8 vertices, 12 edges of a box 0..d.
fn check_box_mesh(m: &ProtoMesh, d: [f32; 3]) {
    assert_eq!(m.faces.len(), 6);
    assert_eq!(m.vertices.len(), 8);
    assert_eq!(m.edges.len(), 12);
    assert_eq!(m.indices.len(), 36);
    assert_eq!(m.positions.len(), 24);
    assert_eq!(m.normals.len(), 24);
    assert!(!m.mesh_only);
    for f in &m.faces {
        assert_eq!(f.index_count, 6);
    }
    for e in &m.edges {
        assert!(e.point_count >= 2);
        assert!((e.first_point + e.point_count) as usize <= m.edge_points.len());
    }
    let centre = [d[0] / 2.0, d[1] / 2.0, d[2] / 2.0];
    for tri in m.indices.chunks(3) {
        let (a, b, c) = (
            m.positions[tri[0] as usize],
            m.positions[tri[1] as usize],
            m.positions[tri[2] as usize],
        );
        let n = cross3(sub3(b, a), sub3(c, a));
        let mid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        assert!(
            dot3(n, sub3(mid, centre)) > 0.0,
            "triangle wound inwards: {a:?} {b:?} {c:?}"
        );
        for &i in tri {
            let vn = m.normals[i as usize];
            assert!((dot3(vn, vn) - 1.0).abs() < 1e-4, "normal not unit: {vn:?}");
            assert!(
                dot3(vn, n) > 0.0,
                "vertex normal disagrees with the winding"
            );
        }
    }
    let b = m.bounds.expect("bounds");
    assert!(close3(b.min, [0.0; 3], 1e-5));
    assert!(close3(
        b.max,
        [f64::from(d[0]), f64::from(d[1]), f64::from(d[2])],
        1e-4
    ));
}

pub(crate) fn face_ref(node: u32, index: u32) -> EntityRef {
    EntityRef {
        node,
        kind: EntityKind::Face,
        index,
    }
}

pub(crate) fn entity_of(doc: &Document, r: EntityRef) -> Entity {
    doc.entity(&MeasureItem::Entity(r)).expect("entity")
}

pub(crate) fn row<'a>(m: &'a Measurement, label: &str) -> Option<&'a MeasureRow> {
    m.rows.iter().find(|r| r.label == label)
}

pub(crate) const BOX: [f64; 3] = [10.0, 20.0, 30.0];

// ------------------------------------------------------------------ pure policy

#[test]
fn deflection_formula_and_half_octave_buckets() {
    assert_eq!(mesh_deflection(0.0), 0.002);
    assert_eq!(mesh_deflection(1.0), 0.002);
    assert_eq!(mesh_deflection(10.0), 0.01);
    assert!(close(mesh_deflection(300.0), 0.3, 1e-15));
    assert_eq!(mesh_deflection(1000.0), 0.5);
    assert_eq!(mesh_deflection(1e9), 0.5);
    // round(log2(defl) * 2)
    assert_eq!(mesh_bucket_key(0.5), -2);
    assert_eq!(mesh_bucket_key(0.002), -18);
    assert_eq!(mesh_bucket_key(0.01), -13);
    assert_eq!(mesh_bucket_key(1.0), 0);
    assert_eq!(mesh_bucket_key(2.0_f64.powf(0.3)), 1);
    assert_eq!(mesh_bucket_key(2.0_f64.powf(-0.3)), -1);
    // 2 ** (key / 2)
    assert_eq!(mesh_bucket_deflection(-2), 0.5);
    assert_eq!(mesh_bucket_deflection(0), 1.0);
    assert!(close(
        mesh_bucket_deflection(-13),
        2.0_f64.powf(-6.5),
        1e-18
    ));
    // a size maps to a bucket whose deflection is within a quarter octave of the wanted one
    for size in [0.5, 3.0, 25.0, 140.0, 800.0] {
        let want = mesh_deflection(size);
        let got = mesh_bucket_deflection(mesh_bucket_key(want));
        assert!(got / want < 2.0_f64.powf(0.25) + 1e-9 && want / got < 2.0_f64.powf(0.25) + 1e-9);
    }
}

#[test]
fn read_options_follow_the_environment_values() {
    let o = ReadOptions::from_values(None, None);
    assert!(!o.full_fix && !o.no_delabella);
    assert_eq!(o.delabella_min_faces, 2000);
    assert!(ReadOptions::from_values(Some("1"), None).full_fix);
    assert!(!ReadOptions::from_values(Some("0"), None).full_fix);
    assert!(!ReadOptions::from_values(Some("true"), None).full_fix);
    assert!(ReadOptions::from_values(None, Some("1")).no_delabella);
    assert!(!ReadOptions::from_values(None, Some("")).no_delabella);
}

#[test]
fn limits_and_messages_are_the_python_ones() {
    assert_eq!(
        (
            MAX_INSTANCES,
            AMPLIFY_MIN,
            AMPLIFY_RATIO,
            MAX_ASSEMBLY_DEPTH,
            DELABELLA_MIN_FACES
        ),
        (300_000, 50_000, 100, 200, 2000)
    );
    assert_eq!(msg_meshing(7), "Síťuji 7 těles…");
    assert_eq!(msg_save_failed("X"), "Uložení mezivýsledku selhalo: X");
    assert_eq!(msg_load_failed("X"), "Načtení mezivýsledku selhalo: X");
    let lim = msg_too_many_instances(300_000);
    assert!(lim.starts_with(
        "Sestava by se rozvinula na více než 300 000 instancí dílů a sestav – to je příliš"
    ));
    assert!(lim.ends_with("Soubor se neotevře."));
    let bomb = msg_amplification(120, 60_000);
    assert!(bomb.starts_with("Sestava má v souboru jen 120 záznamů, ale vnořením sestav by se rozvinula na 60000 instancí (500× víc)"));
    assert!(
        msg_amplification(0, 9).contains("(9× víc)"),
        "division by max(1, stored)"
    );
}

/// The texts must be byte-identical to the ones in stepview/core.py (the test is skipped when the Python sources
/// are not next to the Rust workspace).
#[test]
fn czech_messages_appear_in_core_py() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../stepview/core.py");
    let Ok(py) = std::fs::read_to_string(&core) else {
        eprintln!("stepview/core.py not found, skipping");
        return;
    };
    for s in [
        MSG_READING,
        MSG_TRANSFERRING,
        MSG_CHECKING,
        MSG_READING_STL,
        MSG_READ_FAILED,
        MSG_TRANSFER_FAILED,
        MSG_STL_FAILED,
        MSG_CYCLIC,
        MSG_TOO_DEEP,
        "Síťuji {n} těles…",
        "Uložení mezivýsledku selhalo: {st}",
        "Načtení mezivýsledku selhalo: {_retr.GetStatus()}",
        "Sestava by se rozvinula na více než {lim} instancí dílů a sestav – ",
        "to je příliš na zobrazení (pravděpodobně vnořené sestavy, které se opakovaně odkazují na sebe). ",
        "Soubor se neotevře.",
        "Sestava má v souboru jen {stored} záznamů, ale vnořením sestav by se rozvinula na {total} ",
        "instancí ({total // max(1, stored)}× víc) – to vypadá jako záměrně vytvořená „bomba“, ",
        "zobrazení by trvalo minuty. Soubor se neotevře.",
        "(bez názvu)",
    ] {
        assert!(py.contains(s), "not found in core.py: {s}");
    }
    // and the Rust messages are exactly those segments with the values filled in
    assert_eq!(
        msg_too_many_instances(300_000),
        "Sestava by se rozvinula na více než 300 000 instancí dílů a sestav – \
         to je příliš na zobrazení (pravděpodobně vnořené sestavy, které se opakovaně odkazují na sebe). Soubor se neotevře."
    );
    assert_eq!(
        msg_amplification(10, 5000),
        "Sestava má v souboru jen 10 záznamů, ale vnořením sestav by se rozvinula na 5000 \
         instancí (500× víc) – to vypadá jako záměrně vytvořená „bomba“, zobrazení by trvalo minuty. Soubor se neotevře."
    );
    assert_eq!(MSG_NO_NAME, "(bez názvu)");
}

#[test]
fn count_status_maps_to_the_czech_errors() {
    let rec = |status, total, stored| raw::CountRec {
        status,
        total,
        stored,
    };
    assert_eq!(count_result(&rec(0, 12, 3), 300_000, 200), Ok(12));
    assert_eq!(
        count_result(&rec(1, 0, 0), 300_000, 200),
        Err(BridgeError::TooComplex(MSG_CYCLIC.to_string()))
    );
    assert_eq!(
        count_result(&rec(2, 0, 0), 300_000, 200),
        Err(BridgeError::TooComplex(MSG_TOO_DEEP.to_string()))
    );
    assert_eq!(
        count_result(&rec(3, 400_000, 5), 300_000, 200),
        Err(BridgeError::TooComplex(msg_too_many_instances(300_000)))
    );
    assert_eq!(
        count_result(&rec(4, 60_000, 100), 300_000, 200),
        Err(BridgeError::TooComplex(msg_amplification(100, 60_000)))
    );
    assert_eq!(format!("{}", BridgeError::Failed("x".into())), "x");
}

#[test]
fn document_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Document>();
}

// ------------------------------------------------------------------ STEP: a single box

#[test]
fn box_step_reads_meshes_and_builds_the_tree() {
    big_stack(|| {
        let dir = TempDir::new("box");
        // a directory and file name with Czech letters exercise the UTF-8 path handling
        let sub = dir.file("díl šablona");
        std::fs::create_dir_all(&sub).unwrap();
        let path = sub.join("Krabička.step");
        std::fs::write(&path, box_step("Šroub", BOX)).unwrap();

        let (doc, log) = read(&path);
        let doc = doc.expect("read box");
        assert_eq!(
            log,
            [
                "Čtu soubor…",
                "Převádím geometrii…",
                "Kontroluji strukturu sestavy…",
                "Síťuji 1 těles…"
            ]
        );
        assert_eq!(doc.proto_count(), 1);
        assert_eq!(doc.count_instances(), Ok(1));

        let tree = doc.tree().expect("tree");
        assert_eq!(tree.roots, vec![0]);
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.proto_instances, vec![1]);
        let n = &tree.nodes[0];
        assert_eq!(n.name, "Šroub");
        assert_eq!(n.proto_name, "Šroub");
        assert!(!n.is_assembly && n.parent.is_none() && n.children.is_empty());
        assert_eq!(n.proto, Some(0));
        assert_eq!(n.location, IDENTITY);
        assert!(
            !n.key.is_empty() && n.key.contains(':'),
            "label entry: {}",
            n.key
        );
        assert!(!n.own_style);
        // the tree is cached and equal on the second call
        assert_eq!(doc.tree().unwrap().nodes[0].key, n.key);

        let mesh = doc.proto_mesh(0);
        check_box_mesh(&mesh, [10.0, 20.0, 30.0]);
        assert!(mesh.faces.iter().all(|f| f.color.is_none()));
        // ids beyond the table give an empty mesh, not a panic
        assert!(doc.proto_mesh(5).indices.is_empty());
        assert!(doc.try_proto_mesh(5).is_err());

        let b = doc.node_bounds(0).expect("bounds");
        assert!(
            close3(b.min, [0.0; 3], 1e-4) && close3(b.max, BOX, 1e-4),
            "{b:?}"
        );
        assert!(doc.node_bounds(99).is_none());
    });
}

#[test]
fn mass_props_of_the_box() {
    big_stack(|| {
        let dir = TempDir::new("mass");
        let path = dir.file("box.stp");
        std::fs::write(&path, box_step("Box", BOX)).unwrap();
        let doc = read_ok(&path);
        let m = doc.mass_props(0);
        assert!(close(m.volume_mm3, 6000.0, 1e-6), "{m:?}");
        assert!(close(m.area_mm2, 2200.0, 1e-6), "{m:?}");
        assert!(close3(m.centroid, [5.0, 10.0, 15.0], 1e-6), "{m:?}");
        assert_eq!((m.solids, m.faces), (1, 6));
        assert_eq!(doc.mass_props(9), MassProps::default());
    });
}

#[test]
fn delabella_and_full_fix_paths_mesh_the_same_box() {
    big_stack(|| {
        let dir = TempDir::new("delabella");
        let path = dir.file("box.step");
        std::fs::write(&path, box_step("Box", BOX)).unwrap();
        // threshold 0: 6 faces > 0, so the Delabella branch runs (and its check for unmeshed faces)
        let opts = ReadOptions {
            full_fix: true,
            no_delabella: false,
            delabella_min_faces: 0,
        };
        let doc = Document::read_with(&path, &mut |_| {}, &opts).expect("read");
        check_box_mesh(&doc.proto_mesh(0), [10.0, 20.0, 30.0]);
        let opts = ReadOptions {
            full_fix: false,
            no_delabella: true,
            delabella_min_faces: 0,
        };
        let doc = Document::read_with(&path, &mut |_| {}, &opts).expect("read");
        check_box_mesh(&doc.proto_mesh(0), [10.0, 20.0, 30.0]);
    });
}

#[test]
fn unreadable_input_gives_the_czech_message() {
    big_stack(|| {
        let dir = TempDir::new("bad");
        let missing = dir.file("neexistuje.step");
        assert_eq!(
            read(&missing).0.err(),
            Some(BridgeError::Failed(MSG_READ_FAILED.to_string()))
        );
        let junk = dir.file("junk.step");
        std::fs::write(&junk, "this is not a STEP file\n").unwrap();
        // OCCT reports "done" for a file it cannot parse (0 entities); the transfer then fails (same as Python)
        let e = read(&junk).0.err().expect("junk step");
        assert!(
            e == BridgeError::Failed(MSG_READ_FAILED.to_string())
                || e == BridgeError::Failed(MSG_TRANSFER_FAILED.to_string()),
            "{e}"
        );
        let igs = dir.file("junk.IGS");
        std::fs::write(&igs, "not an IGES file\n").unwrap();
        let (r, log) = read(&igs);
        let e = r.err().expect("junk iges");
        assert!(
            e == BridgeError::Failed(MSG_READ_FAILED.to_string())
                || e == BridgeError::Failed(MSG_TRANSFER_FAILED.to_string()),
            "{e}"
        );
        assert_eq!(log[0], "Čtu soubor…");
        assert!(log.len() <= 2);
        let empty_stl = dir.file("empty.stl");
        let mut bin = vec![0u8; 80];
        bin.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&empty_stl, bin).unwrap();
        let (r, log) = read(&empty_stl);
        assert_eq!(
            r.err(),
            Some(BridgeError::Failed(MSG_STL_FAILED.to_string()))
        );
        assert_eq!(log, ["Čtu STL…"]);
    });
}

// ------------------------------------------------------------------ XBF

#[test]
fn xbf_round_trip_keeps_tree_triangles_and_mass() {
    big_stack(|| {
        let dir = TempDir::new("xbf");
        let path = dir.file("box.step");
        std::fs::write(&path, box_step("Box", BOX)).unwrap();
        let doc = read_ok(&path);
        let xbf = dir.file("meziv\u{fd}sledek.xbf");
        doc.save_xbf(&xbf).expect("save");
        assert!(std::fs::metadata(&xbf).unwrap().len() > 0);

        let loaded = Document::load_xbf(&xbf).expect("load");
        assert_eq!(loaded.proto_count(), doc.proto_count());
        let (a, b) = (doc.tree().unwrap(), loaded.tree().unwrap());
        assert_eq!(a.nodes.len(), b.nodes.len());
        assert_eq!(a.roots, b.roots);
        assert_eq!(a.proto_instances, b.proto_instances);
        assert_eq!(
            (&a.nodes[0].name, &a.nodes[0].key, a.nodes[0].location),
            (&b.nodes[0].name, &b.nodes[0].key, b.nodes[0].location)
        );
        // the triangulation travels in the file: same mesh without meshing again
        let (m1, m2) = (doc.proto_mesh(0), loaded.proto_mesh(0));
        check_box_mesh(&m2, [10.0, 20.0, 30.0]);
        assert_eq!(m1.indices.len(), m2.indices.len());
        assert_eq!(m1.positions.len(), m2.positions.len());
        assert_eq!(m1.edges.len(), m2.edges.len());
        for (p, q) in m1.positions.iter().zip(&m2.positions) {
            assert!((0..3).all(|k| (p[k] - q[k]).abs() < 1e-5));
        }
        assert_eq!(doc.mass_props(0), loaded.mass_props(0));

        // errors keep the Python message shape: "<text>: <status>"
        let e = Document::load_xbf(&dir.file("neexistuje.xbf"))
            .err()
            .expect("load error");

        assert!(
            e.message()
                .starts_with("Načtení mezivýsledku selhalo: PCDM_ReaderStatus.PCDM_RS_"),
            "{e}"
        );
        let junk = dir.file("junk.xbf");
        std::fs::write(&junk, "not an xbf").unwrap();
        let e = Document::load_xbf(&junk).err().expect("junk error");
        assert!(
            e.message()
                .starts_with("Načtení mezivýsledku selhalo: PCDM_ReaderStatus.PCDM_RS_"),
            "{e}"
        );
        // truncated and damaged files must give an error, never a crash
        let good = std::fs::read(&xbf).unwrap();
        for (name, bytes) in [
            ("empty.xbf", Vec::new()),
            ("magic-only.xbf", good[..7].to_vec()),
            ("head.xbf", good[..60].to_vec()),
            ("half.xbf", good[..good.len() / 2].to_vec()),
            ("almost.xbf", good[..good.len() - 40].to_vec()),
            (
                "magic-junk.xbf",
                [b"BINFILE".as_slice(), &[0xFF; 300]].concat(),
            ),
        ] {
            let p = dir.file(name);
            std::fs::write(&p, bytes).unwrap();
            let e = Document::load_xbf(&p)
                .err()
                .unwrap_or_else(|| panic!("{name} should not load"));
            assert!(
                e.message().starts_with("Načtení mezivýsledku selhalo: "),
                "{name}: {e}"
            );
        }
        let e = doc
            .save_xbf(&dir.file("no-such-dir").join("x.xbf"))
            .expect_err("save error");
        assert!(
            e.message()
                .starts_with("Uložení mezivýsledku selhalo: PCDM_StoreStatus.PCDM_SS_"),
            "{e}"
        );
    });
}

#[test]
fn xbf_keeps_assemblies_and_colours() {
    big_stack(|| {
        let dir = TempDir::new("xbf-asm");
        let p = stepgen::Part {
            name: "Barevný".into(),
            dims: BOX,
            color: Some([0.8, 0.2, 0.1]),
            top_color: Some([0.1, 0.7, 0.2]),
            cylinder: false,
        };
        let path = write_step(
            &dir,
            "asm.step",
            &[
                Product::Part(p),
                assembly(
                    "Sestava",
                    vec![
                        usage(0, "Jedna", [0.0, 0.0, 0.0]),
                        usage(0, "Dva", [0.0, 50.0, 0.0]),
                    ],
                ),
            ],
        );
        let doc = read_ok(&path);
        let xbf = dir.file("asm.xbf");
        doc.save_xbf(&xbf).unwrap();
        let loaded = Document::load_xbf(&xbf).unwrap();
        let (a, b) = (doc.tree().unwrap(), loaded.tree().unwrap());
        assert_eq!(a.nodes.len(), 3);
        for (x, y) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!(
                (
                    &x.name,
                    &x.proto_name,
                    &x.key,
                    x.is_assembly,
                    x.proto,
                    x.location
                ),
                (
                    &y.name,
                    &y.proto_name,
                    &y.key,
                    y.is_assembly,
                    y.proto,
                    y.location
                )
            );
            assert_eq!(x.color, y.color);
            assert_eq!(x.own_style, y.own_style);
        }
        let (c1, c2) = (doc.proto_mesh(0), loaded.proto_mesh(0));
        assert_eq!(c1.faces, c2.faces, "face colours survive the XBF file");
    });
}

// ------------------------------------------------------------------ assemblies

fn four_box_assembly(dir: &TempDir) -> PathBuf {
    // boxes 10 x 20 x 30 at x = 0 (A), 5 (B overlaps A), 30 (C alone), 15 (D touches B at x = 15)
    write_step(
        dir,
        "four.step",
        &[
            part("Kostka", BOX),
            assembly(
                "Čtyři",
                vec![
                    usage(0, "A", [0.0, 0.0, 0.0]),
                    usage(0, "B", [5.0, 0.0, 0.0]),
                    usage(0, "C", [30.0, 0.0, 0.0]),
                    usage(0, "D", [15.0, 0.0, 0.0]),
                ],
            ),
        ],
    )
}

#[test]
fn assembly_tree_names_locations_and_instance_counts() {
    big_stack(|| {
        let dir = TempDir::new("asm");
        let path = write_step(
            &dir,
            "asm.step",
            &[
                part("Šroub", BOX),
                assembly(
                    "Sestava",
                    vec![
                        usage(0, "Levý", [0.0, 0.0, 0.0]),
                        usage(0, "Pravý", [20.0, 5.0, 0.0]),
                    ],
                ),
            ],
        );
        let (doc, log) = read(&path);
        let doc = doc.expect("read assembly");
        assert_eq!(log.last().map(String::as_str), Some("Síťuji 1 těles…"));
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.roots, vec![0]);
        let root = &tree.nodes[0];
        assert!(root.is_assembly && root.proto.is_none());
        assert_eq!(root.name, "Sestava");
        assert_eq!(root.children, vec![1, 2]);
        assert_eq!(tree.proto_instances, vec![2]);
        for (i, name) in [(1usize, "Levý"), (2, "Pravý")] {
            let n = &tree.nodes[i];
            assert_eq!(n.parent, Some(0));
            assert!(!n.is_assembly);
            assert_eq!(n.proto, Some(0));
            assert_eq!(n.name, name, "instance name wins over the prototype name");
            assert_eq!(n.proto_name, "Šroub");
            assert_eq!(
                n.key, tree.nodes[1].key,
                "instances share the prototype key"
            );
        }
        assert_eq!(&tree.nodes[1].location, &IDENTITY);
        let loc = tree.nodes[2].location;
        assert!(
            close(loc[12], 20.0, 1e-9) && close(loc[13], 5.0, 1e-9) && close(loc[14], 0.0, 1e-9),
            "{loc:?}"
        );
        assert!(
            close(loc[0], 1.0, 1e-12)
                && close(loc[5], 1.0, 1e-12)
                && close(loc[10], 1.0, 1e-12)
                && loc[15] == 1.0
        );
        // bounds of the assembly are the union of the instances; of a leaf the placed part
        let all = doc.node_bounds(0).unwrap();
        assert!(
            close3(all.min, [0.0, 0.0, 0.0], 1e-4) && close3(all.max, [30.0, 25.0, 30.0], 1e-4),
            "{all:?}"
        );
        let right = doc.node_bounds(2).unwrap();
        assert!(
            close3(right.min, [20.0, 5.0, 0.0], 1e-4)
                && close3(right.max, [30.0, 25.0, 30.0], 1e-4),
            "{right:?}"
        );

        // count_instances limits (core.py): total = assembly + 2 parts
        assert_eq!(doc.count_instances(), Ok(3));
        assert_eq!(doc.count_instances_with(3, 50_000, 100, 200), Ok(3));
        assert_eq!(
            doc.count_instances_with(2, 50_000, 100, 200),
            Err(BridgeError::TooComplex(msg_too_many_instances(2)))
        );
        // stored = 1 assembly + 2 component references + 1 free shape = 4; 3 > 0 and 3 > 0 * 4
        assert_eq!(
            doc.count_instances_with(300_000, 0, 0, 200),
            Err(BridgeError::TooComplex(msg_amplification(4, 3)))
        );
        assert_eq!(doc.count_instances_with(300_000, 0, 100, 200), Ok(3));
        assert_eq!(
            doc.count_instances_with(300_000, 2, 0, 200),
            Err(BridgeError::TooComplex(msg_amplification(4, 3)))
        );
        assert_eq!(
            doc.count_instances_with(300_000, 3, 0, 200),
            Ok(3),
            "total must exceed AMPLIFY_MIN"
        );
    });
}

#[test]
fn assembly_nested_deeper_than_200_levels_is_refused() {
    big_stack(|| {
        let dir = TempDir::new("deep");
        let mut products = vec![part("List", [1.0, 1.0, 1.0])];
        let levels = 203;
        for i in 0..levels {
            products.push(assembly(
                &format!("Úroveň {i}"),
                vec![usage(i, "dítě", [0.0, 0.0, 0.0])],
            ));
        }
        let path = write_step(&dir, "deep.step", &products);
        let (r, log) = read(&path);
        assert_eq!(
            r.err(),
            Some(BridgeError::TooComplex(MSG_TOO_DEEP.to_string()))
        );
        // the structure check runs before meshing
        assert_eq!(
            log,
            [
                "Čtu soubor…",
                "Převádím geometrii…",
                "Kontroluji strukturu sestavy…"
            ]
        );

        // a chain that stays within the limit opens
        let mut ok = vec![part("List", [1.0, 1.0, 1.0])];
        for i in 0..150 {
            ok.push(assembly(
                &format!("Úroveň {i}"),
                vec![usage(i, "dítě", [0.0, 0.0, 0.0])],
            ));
        }
        let path = write_step(&dir, "ok.step", &ok);
        let doc = read_ok(&path);
        assert_eq!(doc.count_instances(), Ok(151));
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 151);
        assert_eq!(tree.nodes[150].proto, Some(0));
    });
}

#[test]
fn step_colours_reach_the_tree_and_the_faces() {
    big_stack(|| {
        let dir = TempDir::new("colour");
        let p = stepgen::Part {
            name: "Barevný".into(),
            dims: BOX,
            color: Some([0.8, 0.2, 0.1]),
            top_color: Some([0.1, 0.7, 0.2]),
            cylinder: false,
        };
        let path = write_step(&dir, "c.step", &[Product::Part(p)]);
        let doc = read_ok(&path);
        let tree = doc.tree().unwrap();
        let mesh = doc.proto_mesh(0);
        // STEP colours are sRGB, the document keeps linear RGB: linear(0.8, 0.2, 0.1) and linear(0.1, 0.7, 0.2)
        let lin = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let want = |r: f32, g: f32, b: f32| [lin(r), lin(g), lin(b)];
        let same = |c: Rgb, w: [f32; 3]| {
            (c.0 - w[0]).abs() < 1e-4 && (c.1 - w[1]).abs() < 1e-4 && (c.2 - w[2]).abs() < 1e-4
        };
        let node = tree.nodes[0].color.expect("solid colour on the part");
        assert!(same(node, want(0.8, 0.2, 0.1)), "{node:?}");
        assert!(
            !tree.nodes[0].own_style,
            "the colour belongs to the prototype, not to an instance"
        );
        // only the top face carries its own colour; the other five take the solid's (node) colour
        let coloured: Vec<usize> = (0..mesh.faces.len())
            .filter(|&i| mesh.faces[i].color.is_some())
            .collect();
        assert_eq!(coloured.len(), 1, "{:?}", mesh.faces);
        let f = mesh.faces[coloured[0]];
        assert!(same(f.color.unwrap(), want(0.1, 0.7, 0.2)), "{f:?}");
        let first = f.first_index as usize;
        for &i in &mesh.indices[first..first + f.index_count as usize] {
            assert!(
                (mesh.positions[i as usize][2] - 30.0).abs() < 1e-5,
                "the coloured face is the top face"
            );
        }
    });
}

// ------------------------------------------------------------------ measurement

#[test]
fn measure_faces_edges_vertices_of_the_box() {
    big_stack(|| {
        let dir = TempDir::new("measure");
        let path = dir.file("box.step");
        std::fs::write(&path, box_step("Box", BOX)).unwrap();
        let doc = read_ok(&path);
        let _ = doc.tree().unwrap();

        // find the top (z = 30) and bottom (z = 0) planes
        let mut top = None;
        let mut bottom = None;
        for i in 0..6 {
            let e = entity_of(&doc, face_ref(0, i));
            assert_eq!(e.type_name, "rovina");
            assert!(!e.is_line && e.plane_point.is_some());
            let c = e.centroid.unwrap();
            if close(c[2], 30.0, 1e-6) {
                top = Some((i, e.clone()));
            }
            if close(c[2], 0.0, 1e-6) {
                bottom = Some((i, e.clone()));
            }
        }
        let (ti, top_e) = top.expect("top face");
        let (bi, bottom_e) = bottom.expect("bottom face");
        assert!(close(top_e.area.unwrap(), 200.0, 1e-9));
        assert!(
            close3(top_e.axis.unwrap(), [0.0, 0.0, 1.0], 1e-12),
            "outward normal of the top face: {:?}",
            top_e.axis
        );
        assert!(close3(bottom_e.axis.unwrap(), [0.0, 0.0, -1.0], 1e-12));
        let rows = top_e.rows();
        assert_eq!(rows[0], MeasureRow::new("Entita", "plocha – rovina"));
        assert!(rows.contains(&MeasureRow::new("Plocha", "200,00 mm²  (2,000 cm²)")));
        assert!(rows.contains(&MeasureRow::new("Normála", "(0,0000; 0,0000; 1,0000)")));

        // one entity: panel rows, label, status text
        let m1 = doc.measure(&[face_ref(0, ti)]);
        assert_eq!(m1.rows, top_e.rows());
        assert_eq!(m1.status.as_deref(), Some("rovina: plocha 200,00 mm²"));
        assert_eq!(m1.label.as_ref().map(|l| l.1.as_str()), Some("200,0 mm²"));
        assert!(m1.error.is_none());

        // two parallel planes: minimum distance, angle, plane spacing
        let m2 = doc.measure(&[face_ref(0, ti), face_ref(0, bi)]);
        assert_eq!(
            m2.rows[0],
            MeasureRow::new("Entita 1", "plocha – rovina, plocha 200,00")
        );
        assert_eq!(
            row(&m2, "Min. vzdálenost"),
            Some(&MeasureRow::bold("Min. vzdálenost", "30,000 mm"))
        );
        assert_eq!(row(&m2, "Úhel"), Some(&MeasureRow::new("Úhel", "180,000°")));
        assert_eq!(
            row(&m2, "Vzdálenost rovin"),
            Some(&MeasureRow::bold("Vzdálenost rovin", "30,000 mm"))
        );
        assert_eq!(m2.distance_label.as_deref(), Some("30,000 mm"));
        assert!(
            m2.status
                .as_deref()
                .unwrap()
                .starts_with("Vzdálenost 30,00 mm  (ΔX "),
            "{:?}",
            m2.status
        );
        let (p1, p2) = m2.min_points.expect("closest points");
        assert!(close(
            ((p1[0] - p2[0]).powi(2) + (p1[1] - p2[1]).powi(2) + (p1[2] - p2[2]).powi(2)).sqrt(),
            30.0,
            1e-6
        ));

        // perpendicular planes (top and a side) have no plane spacing row
        let mut side = None;
        for i in 0..6 {
            let e = entity_of(&doc, face_ref(0, i));
            if e.axis.map(|a| a[2].abs() < 0.5).unwrap_or(false) {
                side = Some(i);
                break;
            }
        }
        let m3 = doc.measure(&[face_ref(0, ti), face_ref(0, side.unwrap())]);
        assert_eq!(row(&m3, "Úhel"), Some(&MeasureRow::new("Úhel", "90,000°")));
        assert!(row(&m3, "Vzdálenost rovin").is_none());

        // three entities: totals
        let m4 = doc.measure(&[face_ref(0, 0), face_ref(0, 1), face_ref(0, 2)]);
        assert_eq!(m4.rows[0], MeasureRow::new("Vybráno", "3 entit"));
        assert_eq!(m4.rows[1].label, "Součet ploch");
        assert!(
            m4.rows[1].value.ends_with("cm², 3 ploch)"),
            "{:?}",
            m4.rows[1]
        );
        assert_eq!(m4.rows.len(), 2 + 3);

        // edges: 12 straight edges, 4 of each length
        let mut lens = Vec::new();
        for i in 0..12 {
            let e = entity_of(
                &doc,
                EntityRef {
                    node: 0,
                    kind: EntityKind::Edge,
                    index: i,
                },
            );
            assert_eq!(e.type_name, "úsečka");
            assert!(e.is_line && e.axis.is_some());
            lens.push(e.length.unwrap());
            if i == 0 {
                let labels: Vec<_> = e.rows().iter().map(|r| r.label.clone()).collect();
                assert_eq!(
                    labels,
                    ["Entita", "Délka", "Směr", "ΔX; ΔY; ΔZ", "Začátek", "Konec"]
                );
            }
        }
        for want in BOX {
            assert_eq!(
                lens.iter().filter(|l| close(**l, want, 1e-9)).count(),
                4,
                "edges of length {want}: {lens:?}"
            );
        }
        let m5 = doc.measure(&[EntityRef {
            node: 0,
            kind: EntityKind::Edge,
            index: 0,
        }]);
        assert!(
            m5.status.as_deref().unwrap().starts_with("úsečka: délka "),
            "{:?}",
            m5.status
        );
        assert!(m5.label.is_some());

        // vertices: 8, the same points as the mesh, and the distance between opposite corners
        let mesh = doc.proto_mesh(0);
        let mut corner0 = None;
        let mut corner7 = None;
        for i in 0..8u32 {
            let e = entity_of(
                &doc,
                EntityRef {
                    node: 0,
                    kind: EntityKind::Vertex,
                    index: i,
                },
            );
            assert_eq!(e.type_name, "bod");
            let p = e.point.unwrap();
            let q = mesh.vertices[i as usize];
            assert!((0..3).all(|k| (p[k] - f64::from(q[k])).abs() < 1e-4));
            if close3(p, [0.0; 3], 1e-9) {
                corner0 = Some(i);
            }
            if close3(p, BOX, 1e-9) {
                corner7 = Some(i);
            }
        }
        let v = |index| EntityRef {
            node: 0,
            kind: EntityKind::Vertex,
            index,
        };
        let m6 = doc.measure(&[v(corner0.unwrap()), v(corner7.unwrap())]);
        assert_eq!(
            row(&m6, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("37,417 mm")
        );
        assert_eq!(
            row(&m6, "Vzdálenost středů").map(|r| r.value.as_str()),
            Some("37,417 mm")
        );
        assert_eq!(
            row(&m6, "Bod 2").map(|r| r.value.as_str()),
            Some("10,000; 20,000; 30,000")
        );
        assert_eq!(m6.rows[0], MeasureRow::new("Entita 1", "vrchol – bod"));
        assert_eq!(m6.center_points, Some(([0.0; 3], BOX)));

        // an invalid pick does not panic: the panel says why
        let bad = doc.measure(&[face_ref(0, 99)]);
        assert!(bad.error.is_some());
        assert_eq!(bad.rows[0].label, "Měření");
        assert!(bad.rows[0].value.starts_with("nelze vyhodnotit: "));
        let bad = doc.measure(&[face_ref(77, 0)]);
        assert!(bad.error.is_some());
    });
}

#[test]
fn measure_points_and_face_normals() {
    big_stack(|| {
        let dir = TempDir::new("points");
        let path = dir.file("box.step");
        std::fs::write(&path, box_step("Box", BOX)).unwrap();
        let doc = read_ok(&path);
        let _ = doc.tree().unwrap();
        let find = |z: f64| {
            (0..6)
                .find(|&i| {
                    close(
                        entity_of(&doc, face_ref(0, i)).centroid.unwrap()[2],
                        z,
                        1e-6,
                    )
                })
                .unwrap()
        };
        let (top, bottom) = (find(30.0), find(0.0));
        let n = doc
            .face_normal_at(face_ref(0, top), [5.0, 10.0, 30.0])
            .expect("normal");
        assert!(close3(n, [0.0, 0.0, 1.0], 1e-9), "{n:?}");
        let n = doc
            .face_normal_at(face_ref(0, bottom), [5.0, 10.0, 0.0])
            .expect("normal");
        assert!(close3(n, [0.0, 0.0, -1.0], 1e-9), "{n:?}");
        assert!(doc.face_normal_at(face_ref(0, 99), [0.0; 3]).is_none());

        // thickness from two picked surface points (they carry the surface normal): 2 degrees of tolerance
        let pt = |p: P3, n: P3| MeasureItem::Point {
            point: p,
            normal: Some(n),
            mesh: false,
            snapped: false,
            patch: None,
        };
        let m = doc.measure_items(&[
            pt([5.0, 10.0, 30.0], [0.0, 0.0, 1.0]),
            pt([5.0, 10.0, 0.0], [0.0, 0.0, -1.0]),
        ]);
        assert_eq!(
            row(&m, "Kolmá vzdálenost (tloušťka)"),
            Some(&MeasureRow::bold(
                "Kolmá vzdálenost (tloušťka)",
                "30,000 mm"
            ))
        );
        assert_eq!(
            row(&m, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("30,000 mm")
        );
        assert_eq!(m.rows[0], MeasureRow::new("Entita 1", "bod – bod na ploše"));
        // a picked point and a B-rep face
        let m = doc.measure_items(&[
            pt([5.0, 10.0, 12.0], [0.0, 0.0, 1.0]),
            MeasureItem::Entity(face_ref(0, top)),
        ]);
        assert_eq!(
            row(&m, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("18,000 mm")
        );
        assert_eq!(
            row(&m, "Kolmá vzdálenost (tloušťka)").map(|r| r.value.as_str()),
            Some("18,000 mm")
        );
        // a mesh outline against a face
        let lp = MeshLoop {
            points: vec![[0.0, 0.0, 40.0], [10.0, 0.0, 40.0], [10.0, 10.0, 40.0]],
            closed: true,
            length: 34.14,
            kind: "obrys".to_string(),
            circle: None,
            coverage: 0.0,
        };
        let m = doc.measure_items(&[
            MeasureItem::Loop(lp.clone()),
            MeasureItem::Entity(face_ref(0, top)),
        ]);
        assert_eq!(
            row(&m, "Min. vzdálenost").map(|r| r.value.as_str()),
            Some("10,000 mm"),
            "{:?}",
            m.rows
        );
        assert_eq!(m.rows[0].value, "hrana – obrys (síť), délka 34,14");
        // a polyline of one point cannot be a wire
        let one = MeshLoop {
            points: vec![[0.0; 3]],
            ..lp
        };
        let m = doc.measure_items(&[
            MeasureItem::Loop(one),
            MeasureItem::Entity(face_ref(0, top)),
        ]);
        assert!(m.error.is_some());
        // the hover text of the measure tool
        let short = doc
            .entity(&MeasureItem::Entity(face_ref(0, top)))
            .unwrap()
            .short();
        assert_eq!(short, "plocha: rovina  A 200,00 mm²");
        let short = doc
            .entity(&MeasureItem::Entity(EntityRef {
                node: 0,
                kind: EntityKind::Vertex,
                index: 0,
            }))
            .unwrap()
            .short();
        assert!(short.starts_with("vrchol: bod  ("), "{short}");
    });
}

// ------------------------------------------------------------------ interference

#[test]
fn interference_finds_collisions_and_touches() {
    big_stack(|| {
        let dir = TempDir::new("clash");
        let doc = read_ok(&four_box_assembly(&dir));
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 5);
        let (a, b, c, d) = (1, 2, 3, 4);
        assert_eq!(tree.nodes[b as usize].name, "B");

        let mut calls = 0;
        let report = doc
            .interferences_ex(&[0], false, &mut |p| {
                calls += 1;
                assert_eq!(p.total, 2);
                true
            })
            .expect("interference");
        assert!(calls >= 1);
        assert!(!report.stopped);
        assert_eq!(
            (report.part_pairs, report.body_pairs),
            (2, 2),
            "A-B overlap and B-D touch have overlapping boxes"
        );
        assert_eq!(report.results.len(), 1, "{:?}", report.results);
        let r = report.results[0];
        assert_eq!((r.a, r.b), (a, b));
        assert!(close(r.volume_mm3, 5.0 * 20.0 * 30.0, 1e-3), "{r:?}");
        assert!(!is_touch(&r));
        let bb = r.bounds.expect("bounds");
        assert!(
            close3(bb.min, [5.0, 0.0, 0.0], 1e-6) && close3(bb.max, [10.0, 20.0, 30.0], 1e-6),
            "{bb:?}"
        );

        // touches on request: B-D share the face x = 15, reported with zero volume after the collision
        let report = doc.interferences_ex(&[0], true, &mut |_| true).unwrap();
        assert_eq!(report.results.len(), 2, "{:?}", report.results);
        assert_eq!((report.results[0].a, report.results[0].b), (a, b));
        let touch = report.results[1];
        assert_eq!((touch.a, touch.b), (b, d));
        assert_eq!(touch.volume_mm3, 0.0);
        assert!(is_touch(&touch));
        assert!(touch.bounds.is_none());

        // the simple entry point and a subset of nodes
        let simple = doc.interferences(&[1, 2, 3, 4], &mut |_| true);
        assert_eq!(simple.len(), 1);
        assert!(doc.interferences(&[c, d], &mut |_| true).is_empty());
        assert!(
            doc.interferences(&[a], &mut |_| true).is_empty(),
            "one part cannot collide with itself"
        );

        // stopping from the progress callback
        let report = doc.interferences_ex(&[0], false, &mut |_| false).unwrap();
        assert!(report.stopped);

        // the red overlay of the common volume
        let mesh = doc.interference_mesh(a, b).unwrap().expect("overlay mesh");
        let mb = mesh.bounds.unwrap();
        assert!(
            close3(mb.min, [5.0, 0.0, 0.0], 1e-4) && close3(mb.max, [10.0, 20.0, 30.0], 1e-4),
            "{mb:?}"
        );
        assert!(!mesh.indices.is_empty());
        assert!(doc.interference_mesh(a, c).unwrap().is_none());
        assert!(doc.interference_mesh(a, 999).is_err());
    });
}

// ------------------------------------------------------------------ STL

#[test]
fn stl_box_is_a_mesh_only_part() {
    big_stack(|| {
        let dir = TempDir::new("stl");
        let path = dir.file("Šablona díl.v2.STL");
        std::fs::write(&path, box_stl(BOX)).unwrap();
        let (doc, log) = read(&path);
        let doc = doc.expect("read stl");
        assert_eq!(log, ["Čtu STL…"]);
        let tree = doc.tree().unwrap();
        assert_eq!(tree.nodes.len(), 1);
        assert_eq!(tree.nodes[0].name, "Šablona díl.v2");
        assert_eq!(tree.proto_instances, vec![1]);
        let m = doc.proto_mesh(0);
        assert!(m.mesh_only);
        assert_eq!(m.faces.len(), 1);
        assert_eq!(m.indices.len(), 36);
        assert!(m.edges.is_empty() && m.vertices.is_empty());
        assert_eq!(m.normals.len(), m.positions.len());
        // sharp 90 degree edges are not merged: 24 nodes
        assert_eq!(
            m.positions.len(),
            24,
            "nodes are merged below 30 degrees only"
        );
        for n in &m.normals {
            assert!((dot3(*n, *n) - 1.0).abs() < 1e-4);
        }
        let mp = doc.mass_props(0);
        assert!(close(mp.volume_mm3, 6000.0, 1e-3), "{mp:?}");
        assert!(close(mp.area_mm2, 2200.0, 1e-3), "{mp:?}");
        assert!(close3(mp.centroid, [5.0, 10.0, 15.0], 1e-4));
        assert_eq!((mp.solids, mp.faces), (0, 1));
        // no solids: collisions skip it
        assert!(doc.interferences(&[0], &mut |_| true).is_empty());
        // xbf round trip of a mesh-only document
        let xbf = dir.file("stl.xbf");
        doc.save_xbf(&xbf).unwrap();
        let loaded = Document::load_xbf(&xbf).unwrap();
        assert!(loaded.proto_mesh(0).mesh_only);
        assert_eq!(loaded.proto_mesh(0).indices.len(), 36);
    });
}

// ------------------------------------------------------------------ threads

#[test]
fn documents_load_concurrently() {
    let dir = TempDir::new("threads");
    let path = dir.file("box.step");
    std::fs::write(&path, box_step("Box", BOX)).unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let p = path.clone();
            std::thread::Builder::new()
                .stack_size(32 << 20)
                .spawn(move || {
                    let doc = Document::read(&p, &mut |_| {}).expect("read");
                    let m = doc.mass_props(0);
                    (m.volume_mm3, doc.proto_mesh(0).indices.len())
                })
                .unwrap()
        })
        .collect();
    for h in handles {
        let (v, n) = h.join().unwrap();
        assert!(close(v, 6000.0, 1e-6));
        assert_eq!(n, 36);
    }
}
