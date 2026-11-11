//! rdxf - hot loops of the CAD viewer's 2D (DXF) renderer (stepview/fastscene.py).
//!
//!  * `assign_levels` : z-order preserving batching of drawing primitives. Used by fastscene.py.
//!  * `read_bulk`     : proof of concept ASCII DXF reader (LINE / CIRCLE / ARC / LWPOLYLINE of the ENTITIES section)
//!                      returning numpy arrays without one Python object per entity. NOT used by the viewer yet.
//!
//! Every function validates its input and returns a Python exception instead of panicking; the viewer falls back to
//! its Python implementation / legacy renderer on any exception.
use numpy::{IntoPyArray, PyArray1, PyArrayMethods, PyReadonlyArray1, PyReadonlyArray2};
use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::{BTreeMap, HashMap};

/// Version of the Python-visible API (fastscene.py refuses a module with another number).
const API: u32 = 2;

/// Largest grid (cells) assign_levels accepts: three i32 arrays of this size are allocated.
const MAX_CELLS: usize = 4_000_000;

// ------------------------------------------------------------------------------------------------ levels
/// bb (P,4) x0,y0,x1,y1 ; gid (P,) group ids >= 0 ; pad ; cell (grid cell size, chosen by the caller)
/// -> level (P,) int32.
///
/// Primitives are processed in drawing order. Primitive i is put into the lowest level of its own group that lies
/// above every level already used by a *different* group in the grid cells touched by its bounding box inflated by
/// `pad`. Primitives of one group look the same, so their relative order does not matter; primitives of different
/// groups that may overlap keep their drawing order (a later one always gets a higher level).
#[pyfunction]
fn assign_levels<'py>(
    py: Python<'py>,
    bb: PyReadonlyArray2<'py, f64>,
    gid: PyReadonlyArray1<'py, i32>,
    pad: f64,
    cell: f64,
) -> PyResult<Bound<'py, PyArray1<i32>>> {
    let bb = bb.as_array();
    let gid = gid
        .as_slice()
        .map_err(|_| PyValueError::new_err("gid must be a contiguous int32 array"))?;
    let p = gid.len();
    if bb.shape() != [p, 4] {
        return Err(PyValueError::new_err("bb must have shape (len(gid), 4)"));
    }
    if p == 0 {
        return Ok(Vec::<i32>::new().into_pyarray(py));
    }
    if !(pad.is_finite() && pad >= 0.0 && cell.is_finite() && cell > 0.0) {
        return Err(PyValueError::new_err("pad / cell must be finite and positive"));
    }
    if gid.iter().any(|&g| g < 0) {
        return Err(PyValueError::new_err("group ids must be >= 0"));
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..p {
        let (a, b, c, d) = (bb[[i, 0]], bb[[i, 1]], bb[[i, 2]], bb[[i, 3]]);
        if !(a.is_finite() && b.is_finite() && c.is_finite() && d.is_finite()) {
            return Err(PyValueError::new_err("bounding boxes must be finite"));
        }
        x0 = x0.min(a);
        y0 = y0.min(b);
        x1 = x1.max(c);
        y1 = y1.max(d);
    }
    x0 -= pad;
    y0 -= pad;
    x1 += pad;
    y1 += pad;
    let (w, h) = ((x1 - x0).max(1e-9), (y1 - y0).max(1e-9));
    let (nxf, nyf) = (w / cell + 1.0, h / cell + 1.0);
    if !(nxf * nyf <= MAX_CELLS as f64) {
        return Err(PyValueError::new_err("grid too large: cell size too small for the drawing extent"));
    }
    let (nx, ny) = (nxf as usize, nyf as usize);
    let clampi = |v: f64, n: usize| -> usize {
        if v < 0.0 {
            0
        } else {
            (v as usize).min(n - 1)
        }
    };
    let mut best_l = vec![-1i32; nx * ny]; // highest level in the cell
    let mut best_g = vec![-1i32; nx * ny]; // ... and its group
    let mut second_l = vec![-1i32; nx * ny]; // highest level of any group other than best_g
    let gmax = gid.iter().cloned().max().unwrap_or(0) as usize + 1;
    let mut glevels: Vec<Vec<i32>> = vec![Vec::new(); gmax]; // per group: its levels, ascending
    let mut level = vec![0i32; p];
    let mut nlev: i32 = 0;
    for i in 0..p {
        let g = gid[i];
        let ix0 = clampi((bb[[i, 0]] - pad - x0) / cell, nx);
        let iy0 = clampi((bb[[i, 1]] - pad - y0) / cell, ny);
        let ix1 = clampi((bb[[i, 2]] + pad - x0) / cell, nx);
        let iy1 = clampi((bb[[i, 3]] + pad - y0) / cell, ny);
        let mut m = -1i32;
        for cy in iy0..=iy1 {
            let row = cy * nx;
            for cx in ix0..=ix1 {
                let k = row + cx;
                let v = if best_g[k] != g { best_l[k] } else { second_l[k] };
                if v > m {
                    m = v;
                }
            }
        }
        let lv = &mut glevels[g as usize];
        let pos = lv.partition_point(|&c| c <= m);
        let l = if pos < lv.len() {
            lv[pos]
        } else {
            let l = nlev;
            nlev += 1;
            lv.push(l);
            l
        };
        level[i] = l;
        for cy in iy0..=iy1 {
            let row = cy * nx;
            for cx in ix0..=ix1 {
                let k = row + cx;
                let (bl, bg, sc) = (best_l[k], best_g[k], second_l[k]);
                if l > bl {
                    if bg != g {
                        second_l[k] = bl;
                    }
                    best_l[k] = l;
                    best_g[k] = g;
                } else if bg != g && l > sc {
                    second_l[k] = l;
                }
            }
        }
    }
    Ok(level.into_pyarray(py))
}

// ------------------------------------------------------------------------------------------------ DXF reader PoC
#[derive(Default)]
struct Ent {
    ty: u8, // 1 LINE 2 CIRCLE 3 ARC 4 LWPOLYLINE
    layer: u32,
    color: i32,
    has_true: bool,
    v: [f64; 8], // x0 y0 x1 y1 r a0 a1 const_w
    flags: i32,
    pts_start: usize,
    pts_n: usize,
}

fn parse_f(s: &[u8]) -> f64 {
    std::str::from_utf8(s).ok().and_then(|t| t.trim().parse::<f64>().ok()).unwrap_or(0.0)
}

fn parse_i(s: &[u8]) -> i32 {
    std::str::from_utf8(s).ok().and_then(|t| t.trim().parse::<i32>().ok()).unwrap_or(0)
}

type Parsed = (Vec<Ent>, Vec<String>, Vec<[f64; 3]>, BTreeMap<String, usize>);

fn parse(data: &[u8]) -> Parsed {
    let mut ents: Vec<Ent> = Vec::new();
    let mut layers: Vec<String> = Vec::new();
    let mut lmap: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut pts: Vec<[f64; 3]> = Vec::new();
    let mut other: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = data.split(|&b| b == b'\n').map(|l| if l.last() == Some(&b'\r') { &l[..l.len() - 1] } else { l });
    let mut in_entities = false;
    let mut section_start = false;
    let mut cur: Option<Ent> = None;
    let mut cur_pt = [0.0f64; 3];
    let mut have_pt = false;
    while let (Some(code_line), Some(val)) = (lines.next(), lines.next()) {
        let code = parse_i(code_line);
        if code == 0 {
            if let Some(mut e) = cur.take() {
                if e.ty == 4 && have_pt {
                    pts.push(cur_pt);
                    e.pts_n += 1;
                }
                ents.push(e);
            }
            have_pt = false;
            let v = std::str::from_utf8(val).unwrap_or("").trim();
            if v == "SECTION" {
                section_start = true;
                continue;
            }
            if v == "ENDSEC" {
                in_entities = false;
                continue;
            }
            if !in_entities {
                continue;
            }
            cur = match v {
                "LINE" => Some(Ent { ty: 1, color: 256, ..Default::default() }),
                "CIRCLE" => Some(Ent { ty: 2, color: 256, ..Default::default() }),
                "ARC" => Some(Ent { ty: 3, color: 256, ..Default::default() }),
                "LWPOLYLINE" => Some(Ent { ty: 4, color: 256, pts_start: pts.len(), ..Default::default() }),
                other_t => {
                    *other.entry(other_t.to_string()).or_insert(0) += 1;
                    None
                }
            };
            continue;
        }
        if section_start && code == 2 {
            in_entities = std::str::from_utf8(val).unwrap_or("").trim() == "ENTITIES";
            section_start = false;
            continue;
        }
        let e = match cur.as_mut() {
            Some(e) => e,
            None => continue,
        };
        match code {
            8 => {
                let id = *lmap.entry(val.to_vec()).or_insert_with(|| {
                    layers.push(String::from_utf8_lossy(val).trim().to_string());
                    (layers.len() - 1) as u32
                });
                e.layer = id;
            }
            62 => e.color = parse_i(val),
            420 => e.has_true = true,
            70 => e.flags = parse_i(val),
            43 => e.v[7] = parse_f(val),
            10 => {
                if e.ty == 4 {
                    if have_pt {
                        pts.push(cur_pt);
                        e.pts_n += 1;
                    }
                    cur_pt = [parse_f(val), 0.0, 0.0];
                    have_pt = true;
                } else {
                    e.v[0] = parse_f(val);
                }
            }
            20 => {
                if e.ty == 4 {
                    cur_pt[1] = parse_f(val);
                } else {
                    e.v[1] = parse_f(val);
                }
            }
            42 => {
                if e.ty == 4 {
                    cur_pt[2] = parse_f(val);
                }
            }
            11 => e.v[2] = parse_f(val),
            21 => e.v[3] = parse_f(val),
            40 => {
                if e.ty != 4 {
                    e.v[4] = parse_f(val);
                }
            }
            50 => e.v[5] = parse_f(val),
            51 => e.v[6] = parse_f(val),
            _ => {}
        }
    }
    if let Some(mut e) = cur.take() {
        if e.ty == 4 && have_pt {
            pts.push(cur_pt);
            e.pts_n += 1;
        }
        ents.push(e);
    }
    (ents, layers, pts, other)
}

/// Reads an ASCII DXF and returns the simple entities of the ENTITIES section as numpy arrays (proof of concept).
#[pyfunction]
fn read_bulk<'py>(py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyDict>> {
    let data = std::fs::read(path).map_err(|e| PyIOError::new_err(e.to_string()))?;
    let (ents, layers, pts, other) = py.detach(|| parse(&data));
    let n = ents.len();
    let out = PyDict::new(py);
    out.set_item("kind", ents.iter().map(|e| e.ty).collect::<Vec<u8>>().into_pyarray(py))?;
    out.set_item("layer", ents.iter().map(|e| e.layer).collect::<Vec<u32>>().into_pyarray(py))?;
    out.set_item("color", ents.iter().map(|e| e.color).collect::<Vec<i32>>().into_pyarray(py))?;
    out.set_item("flags", ents.iter().map(|e| e.flags).collect::<Vec<i32>>().into_pyarray(py))?;
    out.set_item("true_color", ents.iter().map(|e| e.has_true).collect::<Vec<bool>>().into_pyarray(py))?;
    let flat: Vec<f64> = ents.iter().flat_map(|e| e.v.iter().cloned()).collect();
    out.set_item("geo", flat.into_pyarray(py).reshape([n, 8])?)?;
    out.set_item("pts_start", ents.iter().map(|e| e.pts_start as i64).collect::<Vec<i64>>().into_pyarray(py))?;
    out.set_item("pts_n", ents.iter().map(|e| e.pts_n as i64).collect::<Vec<i64>>().into_pyarray(py))?;
    let np_ = pts.len();
    let flatp: Vec<f64> = pts.iter().flat_map(|p| p.iter().cloned()).collect();
    out.set_item("pts", flatp.into_pyarray(py).reshape([np_, 3])?)?;
    out.set_item("layer_names", layers)?;
    out.set_item("other_types", other)?;
    out.set_item("n", n)?;
    Ok(out)
}

#[pymodule]
fn rdxf(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("API", API)?;
    m.add_function(wrap_pyfunction!(assign_levels, m)?)?;
    m.add_function(wrap_pyfunction!(read_bulk, m)?)?;
    Ok(())
}
