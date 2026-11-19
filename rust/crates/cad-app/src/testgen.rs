//! Test-only sample files: a small STEP assembly (the generator of the bridge tests is included by path, the other crate
//! is not touched), an STL box and a DXF drawing. `generate_sample_files` writes them into the folder named by
//! `STEPVIEW_SAMPLE_DIR`, for trying the real program.
#[allow(dead_code)]
#[path = "../../occt-bridge/src/stepgen.rs"]
pub mod stepgen;

use std::fmt::Write;
use std::path::Path;

/// Plate with two bolts and a block: parts of different size, one prototype used twice.
pub fn step_assembly() -> String {
    use stepgen::{assembly, cylinder, part, usage, write_step};
    let products = vec![
        part("Deska", [100.0, 60.0, 8.0]),
        cylinder("Šroub M8", 4.0, 30.0),
        part("Blok", [30.0, 30.0, 20.0]),
        assembly(
            "Sestava příkladu",
            vec![
                usage(0, "Deska", [0.0, 0.0, 0.0]),
                usage(1, "Šroub 1", [20.0, 15.0, 8.0]),
                usage(1, "Šroub 2", [80.0, 45.0, 8.0]),
                usage(2, "Blok", [35.0, 15.0, 8.0]),
            ],
        ),
    ];
    write_step(&products)
}

/// Group code / value pairs as DXF text.
fn pair(out: &mut String, code: i32, v: impl std::fmt::Display) {
    let _ = write!(out, "{code:>3}\n{v}\n");
}

/// A drawing with three layers (one switched off), lines, circles, an arc, a polyline and a text.
pub fn dxf_drawing() -> String {
    let mut s = String::new();
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "HEADER");
    pair(&mut s, 9, "$ACADVER");
    pair(&mut s, 1, "AC1027");
    pair(&mut s, 9, "$INSUNITS");
    pair(&mut s, 70, 4);
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "TABLES");
    pair(&mut s, 0, "TABLE");
    pair(&mut s, 2, "LAYER");
    for (name, color, flags) in [("Obrys", 7, 0), ("Kóty", 1, 0), ("Osy", 5, 0), ("Skrytá", 3, 1)] {
        pair(&mut s, 0, "LAYER");
        pair(&mut s, 2, name);
        pair(&mut s, 70, flags);
        pair(&mut s, 62, color);
        pair(&mut s, 6, "Continuous");
    }
    pair(&mut s, 0, "ENDTAB");
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "ENTITIES");
    let mut line = |layer: &str, a: [f64; 2], b: [f64; 2]| {
        pair(&mut s, 0, "LINE");
        pair(&mut s, 8, layer);
        pair(&mut s, 10, a[0]);
        pair(&mut s, 20, a[1]);
        pair(&mut s, 11, b[0]);
        pair(&mut s, 21, b[1]);
    };
    // a plate 200 x 120 with two holes and centre lines
    line("Obrys", [0.0, 0.0], [200.0, 0.0]);
    line("Obrys", [200.0, 0.0], [200.0, 120.0]);
    line("Obrys", [200.0, 120.0], [0.0, 120.0]);
    line("Obrys", [0.0, 120.0], [0.0, 0.0]);
    line("Osy", [40.0, -10.0], [40.0, 130.0]);
    line("Osy", [-10.0, 60.0], [210.0, 60.0]);
    line("Kóty", [0.0, -20.0], [200.0, -20.0]);
    line("Skrytá", [0.0, 0.0], [200.0, 120.0]);
    for (layer, c, r) in [("Obrys", [40.0, 60.0], 15.0), ("Obrys", [150.0, 60.0], 25.0)] {
        pair(&mut s, 0, "CIRCLE");
        pair(&mut s, 8, layer);
        pair(&mut s, 10, c[0]);
        pair(&mut s, 20, c[1]);
        pair(&mut s, 40, r);
    }
    pair(&mut s, 0, "ARC");
    pair(&mut s, 8, "Obrys");
    pair(&mut s, 10, 100.0);
    pair(&mut s, 20, 120.0);
    pair(&mut s, 40, 30.0);
    pair(&mut s, 50, 180.0);
    pair(&mut s, 51, 360.0);
    pair(&mut s, 0, "LWPOLYLINE");
    pair(&mut s, 8, "Kóty");
    pair(&mut s, 90, 4);
    pair(&mut s, 70, 1);
    for (x, y) in [(10.0, 10.0), (30.0, 10.0), (30.0, 30.0), (10.0, 30.0)] {
        pair(&mut s, 10, x);
        pair(&mut s, 20, y);
    }
    pair(&mut s, 0, "TEXT");
    pair(&mut s, 8, "Kóty");
    pair(&mut s, 10, 70.0);
    pair(&mut s, 20, -16.0);
    pair(&mut s, 40, 6.0);
    pair(&mut s, 1, "200 mm – délka desky");
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "EOF");
    s
}

/// A drawing with one IMAGE entity (10 x 10 units) whose picture file is `picture` (looked up next to the drawing).
pub fn dxf_with_picture(picture: &str) -> String {
    let mut s = String::new();
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "HEADER");
    pair(&mut s, 9, "$ACADVER");
    pair(&mut s, 1, "AC1027");
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "ENTITIES");
    pair(&mut s, 0, "IMAGE");
    pair(&mut s, 8, "0");
    for (code, v) in [(10, 0.0), (20, 0.0), (30, 0.0), (11, 5.0), (21, 0.0), (31, 0.0), (12, 0.0), (22, 5.0), (32, 0.0), (13, 2.0), (23, 2.0)] {
        pair(&mut s, code, v);
    }
    pair(&mut s, 340, "90");
    pair(&mut s, 70, 3);
    pair(&mut s, 71, 1);
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "OBJECTS");
    pair(&mut s, 0, "IMAGEDEF");
    pair(&mut s, 5, "90");
    pair(&mut s, 1, picture);
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "EOF");
    s
}

/// A drawing of two spaces: a plate of lines in model space and a sheet ("Sheet A") with a frame in paper space.
pub fn dxf_with_sheet() -> String {
    sheet_drawing(true)
}

/// The frame of the sheet in paper space only: the drawing opens on its sheet.
pub fn dxf_sheet_only() -> String {
    sheet_drawing(false)
}

fn sheet_drawing(model: bool) -> String {
    let mut s = String::new();
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "HEADER");
    pair(&mut s, 9, "$ACADVER");
    pair(&mut s, 1, "AC1027");
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "ENTITIES");
    let mut line = |paper: bool, a: [f64; 2], b: [f64; 2]| {
        pair(&mut s, 0, "LINE");
        pair(&mut s, 8, "0");
        if paper {
            pair(&mut s, 67, 1);
        }
        pair(&mut s, 10, a[0]);
        pair(&mut s, 20, a[1]);
        pair(&mut s, 11, b[0]);
        pair(&mut s, 21, b[1]);
    };
    if model {
        line(false, [0.0, 0.0], [200.0, 0.0]);
        line(false, [200.0, 0.0], [200.0, 120.0]);
        line(false, [200.0, 120.0], [0.0, 0.0]);
    }
    // the frame of the sheet
    line(true, [0.0, 0.0], [297.0, 0.0]);
    line(true, [297.0, 0.0], [297.0, 210.0]);
    line(true, [297.0, 210.0], [0.0, 210.0]);
    line(true, [0.0, 210.0], [0.0, 0.0]);
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "OBJECTS");
    for (handle, name, tab) in [("50", "Model", 0), ("51", "Sheet A", 1)] {
        pair(&mut s, 0, "LAYOUT");
        pair(&mut s, 5, handle);
        pair(&mut s, 100, "AcDbLayout");
        pair(&mut s, 1, name);
        pair(&mut s, 70, 1);
        pair(&mut s, 71, tab);
        pair(&mut s, 330, if tab == 0 { "1F" } else { "1E" });
    }
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "EOF");
    s
}

/// A big assembly of different boxes in a grid (about 1.2 MB), for the worker process and the cache.
pub fn step_big(parts: usize) -> String {
    use stepgen::{assembly, part, usage, write_step};
    let mut products: Vec<stepgen::Product> = (0..parts)
        .map(|i| part(&format!("Díl {i:04}"), [10.0 + (i % 7) as f64, 8.0 + (i % 5) as f64, 4.0 + (i % 3) as f64]))
        .collect();
    let usages = (0..parts)
        .map(|i| usage(i, &format!("Díl {i:04}"), [(i % 30) as f64 * 20.0, (i / 30) as f64 * 20.0, 0.0]))
        .collect();
    products.push(assembly("Velká sestava", usages));
    write_step(&products)
}

/// Writes `sestava.step`, `velka.step`, `kostka.stl` and `vykres.dxf` into a folder.
pub fn write_samples(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("sestava.step"), step_assembly())?;
    std::fs::write(dir.join("velka.step"), step_big(900))?;
    std::fs::write(dir.join("kostka.stl"), stepgen::box_stl([40.0, 30.0, 20.0]))?;
    std::fs::write(dir.join("vykres.dxf"), dxf_drawing())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_written_where_asked() {
        let dir = tempfile::tempdir().unwrap();
        write_samples(dir.path()).unwrap();
        let step = std::fs::read_to_string(dir.path().join("sestava.step")).unwrap();
        assert!(step.starts_with("ISO-10303-21;"));
        assert!(std::fs::read_to_string(dir.path().join("vykres.dxf")).unwrap().contains("ENTITIES"));
        assert_eq!(std::fs::metadata(dir.path().join("kostka.stl")).unwrap().len(), 84 + 12 * 50);
    }

    /// `STEPVIEW_SAMPLE_DIR=<folder> cargo test -p cad-app generate_sample_files` writes the files for a manual run.
    #[test]
    fn generate_sample_files() {
        if let Some(dir) = std::env::var_os("STEPVIEW_SAMPLE_DIR") {
            write_samples(Path::new(&dir)).unwrap();
        }
    }
}
