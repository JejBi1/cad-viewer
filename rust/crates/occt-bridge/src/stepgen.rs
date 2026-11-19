//! Test-only generator of small STEP (AP214) files: boxes, assemblies of boxes and colours. The files are written by
//! the tests themselves, so the bridge is exercised with real STEP input.
use std::collections::HashMap;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct Part {
    pub name: String,
    pub dims: [f64; 3],
    /// Colour of the whole solid.
    pub color: Option<[f64; 3]>,
    /// Colour of the top face (z = dz) only.
    pub top_color: Option<[f64; 3]>,
    /// A cylinder of radius `dims[0]` and height `dims[2]` along z instead of a box.
    pub cylinder: bool,
}

#[derive(Clone, Debug)]
pub struct Usage {
    /// Index of an earlier product.
    pub child: usize,
    pub name: String,
    pub offset: [f64; 3],
}

#[derive(Clone, Debug)]
pub enum Product {
    Part(Part),
    Assembly { name: String, usages: Vec<Usage> },
}

pub fn part(name: &str, dims: [f64; 3]) -> Product {
    Product::Part(Part {
        name: name.to_string(),
        dims,
        color: None,
        top_color: None,
        cylinder: false,
    })
}

/// Cylinder of radius `r` and height `h` (axis z, base at z = 0).
pub fn cylinder(name: &str, r: f64, h: f64) -> Product {
    Product::Part(Part {
        name: name.to_string(),
        dims: [r, r, h],
        color: None,
        top_color: None,
        cylinder: true,
    })
}

pub fn assembly(name: &str, usages: Vec<Usage>) -> Product {
    Product::Assembly {
        name: name.to_string(),
        usages,
    }
}

pub fn usage(child: usize, name: &str, offset: [f64; 3]) -> Usage {
    Usage {
        child,
        name: name.to_string(),
        offset,
    }
}

/// STEP string with characters outside ASCII written as `\X2\....\X0\`.
fn enc(s: &str) -> String {
    let mut out = String::new();
    let mut wide = String::new();
    let flush = |out: &mut String, wide: &mut String| {
        if !wide.is_empty() {
            out.push_str("\\X2\\");
            for u in wide.encode_utf16() {
                let _ = write!(out, "{u:04X}");
            }
            out.push_str("\\X0\\");
            wide.clear();
        }
    };
    for ch in s.chars() {
        if ch.is_ascii() && !ch.is_ascii_control() {
            flush(&mut out, &mut wide);
            match ch {
                '\'' => out.push_str("''"),
                '\\' => out.push_str("\\\\"),
                c => out.push(c),
            }
        } else {
            wide.push(ch);
        }
    }
    flush(&mut out, &mut wide);
    out
}

fn real(x: f64) -> String {
    format!("{x:?}")
}

struct B {
    out: String,
    n: u32,
}

impl B {
    fn add(&mut self, body: &str) -> u32 {
        self.n += 1;
        let _ = writeln!(self.out, "#{} = {};", self.n, body);
        self.n
    }
    fn point(&mut self, p: [f64; 3]) -> u32 {
        self.add(&format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(p[0]),
            real(p[1]),
            real(p[2])
        ))
    }
    fn dir(&mut self, d: [f64; 3]) -> u32 {
        self.add(&format!(
            "DIRECTION('',({},{},{}))",
            real(d[0]),
            real(d[1]),
            real(d[2])
        ))
    }
    fn axis(&mut self, o: [f64; 3]) -> u32 {
        let p = self.point(o);
        let z = self.dir([0.0, 0.0, 1.0]);
        let x = self.dir([1.0, 0.0, 0.0]);
        self.add(&format!("AXIS2_PLACEMENT_3D('',#{p},#{z},#{x})"))
    }
}

struct PartIds {
    solid: u32,
    top_face: u32,
}

fn unit(v: [f64; 3]) -> ([f64; 3], f64) {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    ([v[0] / l, v[1] / l, v[2] / l], l)
}

/// Closed box [0,dx] x [0,dy] x [0,dz] as a MANIFOLD_SOLID_BREP (6 ADVANCED_FACEs, 12 EDGE_CURVEs).
fn write_box(b: &mut B, name: &str, d: [f64; 3]) -> PartIds {
    let idx = |i: usize, j: usize, k: usize| i * 4 + j * 2 + k;
    let mut cp = Vec::new();
    let mut vp = Vec::new();
    let mut co = Vec::new();
    for i in 0..2 {
        for j in 0..2 {
            for k in 0..2 {
                let p = [i as f64 * d[0], j as f64 * d[1], k as f64 * d[2]];
                co.push(p);
                let c = b.point(p);
                cp.push(c);
                vp.push(b.add(&format!("VERTEX_POINT('',#{c})")));
            }
        }
    }
    let mut edges: HashMap<(usize, usize), u32> = HashMap::new();
    let loops: [([usize; 4], [f64; 3]); 6] = [
        (
            [idx(0, 0, 0), idx(0, 1, 0), idx(1, 1, 0), idx(1, 0, 0)],
            [0.0, 0.0, -1.0],
        ),
        (
            [idx(0, 0, 1), idx(1, 0, 1), idx(1, 1, 1), idx(0, 1, 1)],
            [0.0, 0.0, 1.0],
        ),
        (
            [idx(0, 0, 0), idx(1, 0, 0), idx(1, 0, 1), idx(0, 0, 1)],
            [0.0, -1.0, 0.0],
        ),
        (
            [idx(0, 1, 0), idx(0, 1, 1), idx(1, 1, 1), idx(1, 1, 0)],
            [0.0, 1.0, 0.0],
        ),
        (
            [idx(0, 0, 0), idx(0, 0, 1), idx(0, 1, 1), idx(0, 1, 0)],
            [-1.0, 0.0, 0.0],
        ),
        (
            [idx(1, 0, 0), idx(1, 1, 0), idx(1, 1, 1), idx(1, 0, 1)],
            [1.0, 0.0, 0.0],
        ),
    ];
    let mut faces = Vec::new();
    let mut top_face = 0;
    for (n, (lp, normal)) in loops.iter().enumerate() {
        let mut oriented = Vec::new();
        for s in 0..4 {
            let (u, v) = (lp[s], lp[(s + 1) % 4]);
            let (a, c) = if u < v { (u, v) } else { (v, u) };
            let ec = *edges.entry((a, c)).or_insert_with(|| {
                let (dir, len) = unit([
                    co[c][0] - co[a][0],
                    co[c][1] - co[a][1],
                    co[c][2] - co[a][2],
                ]);
                let dd = b.dir(dir);
                let vec = b.add(&format!("VECTOR('',#{dd},{})", real(len)));
                let line = b.add(&format!("LINE('',#{},#{vec})", cp[a]));
                b.add(&format!("EDGE_CURVE('',#{},#{},#{line},.T.)", vp[a], vp[c]))
            });
            let fwd = if u < v { ".T." } else { ".F." };
            oriented.push(b.add(&format!("ORIENTED_EDGE('',*,*,#{ec},{fwd})")));
        }
        let list: Vec<String> = oriented.iter().map(|o| format!("#{o}")).collect();
        let el = b.add(&format!("EDGE_LOOP('',({}))", list.join(",")));
        let bound = b.add(&format!("FACE_OUTER_BOUND('',#{el},.T.)"));
        let nd = b.dir(*normal);
        let (rd, _) = unit([
            co[lp[1]][0] - co[lp[0]][0],
            co[lp[1]][1] - co[lp[0]][1],
            co[lp[1]][2] - co[lp[0]][2],
        ]);
        let rdi = b.dir(rd);
        let ax = b.add(&format!(
            "AXIS2_PLACEMENT_3D('',#{},#{nd},#{rdi})",
            cp[lp[0]]
        ));
        let plane = b.add(&format!("PLANE('',#{ax})"));
        let face = b.add(&format!("ADVANCED_FACE('',(#{bound}),#{plane},.T.)"));
        if n == 1 {
            top_face = face;
        }
        faces.push(face);
    }
    let fl: Vec<String> = faces.iter().map(|f| format!("#{f}")).collect();
    let shell = b.add(&format!("CLOSED_SHELL('',({}))", fl.join(",")));
    let solid = b.add(&format!("MANIFOLD_SOLID_BREP('{}',#{shell})", enc(name)));
    PartIds { solid, top_face }
}

/// Cylinder with a circle at each end and a seam line: 3 faces (plane, cylinder, plane), 3 edges, 2 vertices.
fn write_cylinder(b: &mut B, name: &str, r: f64, h: f64) -> PartIds {
    let p0 = b.point([r, 0.0, 0.0]);
    let p1 = b.point([r, 0.0, h]);
    let v0 = b.add(&format!("VERTEX_POINT('',#{p0})"));
    let v1 = b.add(&format!("VERTEX_POINT('',#{p1})"));
    let ax0 = b.axis([0.0, 0.0, 0.0]);
    let ax1 = b.axis([0.0, 0.0, h]);
    let c0 = b.add(&format!("CIRCLE('',#{ax0},{})", real(r)));
    let c1 = b.add(&format!("CIRCLE('',#{ax1},{})", real(r)));
    let dz = b.dir([0.0, 0.0, 1.0]);
    let vec = b.add(&format!("VECTOR('',#{dz},{})", real(h)));
    let line = b.add(&format!("LINE('',#{p0},#{vec})"));
    let e0 = b.add(&format!("EDGE_CURVE('',#{v0},#{v0},#{c0},.T.)"));
    let e1 = b.add(&format!("EDGE_CURVE('',#{v1},#{v1},#{c1},.T.)"));
    let es = b.add(&format!("EDGE_CURVE('',#{v0},#{v1},#{line},.T.)"));
    let face = |b: &mut B, loop_edges: &[(u32, bool)], surface: u32| -> u32 {
        let oriented: Vec<String> = loop_edges
            .iter()
            .map(|(e, fwd)| {
                format!(
                    "#{}",
                    b.add(&format!(
                        "ORIENTED_EDGE('',*,*,#{e},{})",
                        if *fwd { ".T." } else { ".F." }
                    ))
                )
            })
            .collect();
        let el = b.add(&format!("EDGE_LOOP('',({}))", oriented.join(",")));
        let bound = b.add(&format!("FACE_OUTER_BOUND('',#{el},.T.)"));
        b.add(&format!("ADVANCED_FACE('',(#{bound}),#{surface},.T.)"))
    };
    let plane = |b: &mut B, z: f64, nz: f64| -> u32 {
        let o = b.point([0.0, 0.0, z]);
        let n = b.dir([0.0, 0.0, nz]);
        let x = b.dir([1.0, 0.0, 0.0]);
        let ax = b.add(&format!("AXIS2_PLACEMENT_3D('',#{o},#{n},#{x})"));
        b.add(&format!("PLANE('',#{ax})"))
    };
    let bottom_plane = plane(b, 0.0, -1.0);
    let top_plane = plane(b, h, 1.0);
    let cyl = b.add(&format!("CYLINDRICAL_SURFACE('',#{ax0},{})", real(r)));
    // outward normals: the bottom circle runs clockwise seen from +z, the top one counter-clockwise
    let bottom = face(b, &[(e0, false)], bottom_plane);
    let top = face(b, &[(e1, true)], top_plane);
    let side = face(b, &[(e0, true), (es, true), (e1, false), (es, false)], cyl);
    let shell = b.add(&format!("CLOSED_SHELL('',(#{bottom},#{top},#{side}))"));
    let solid = b.add(&format!("MANIFOLD_SOLID_BREP('{}',#{shell})", enc(name)));
    PartIds {
        solid,
        top_face: top,
    }
}

fn style(b: &mut B, target: u32, rgb: [f64; 3]) -> u32 {
    let c = b.add(&format!(
        "COLOUR_RGB('',{},{},{})",
        real(rgb[0]),
        real(rgb[1]),
        real(rgb[2])
    ));
    let fc = b.add(&format!("FILL_AREA_STYLE_COLOUR('',#{c})"));
    let fs = b.add(&format!("FILL_AREA_STYLE('',(#{fc}))"));
    let sf = b.add(&format!("SURFACE_STYLE_FILL_AREA(#{fs})"));
    let ss = b.add(&format!("SURFACE_SIDE_STYLE('',(#{sf}))"));
    let su = b.add(&format!("SURFACE_STYLE_USAGE(.BOTH.,#{ss})"));
    let psa = b.add(&format!("PRESENTATION_STYLE_ASSIGNMENT((#{su}))"));
    b.add(&format!("STYLED_ITEM('color',(#{psa}),#{target})"))
}

/// Complete STEP file text. Products must be ordered children first.
pub fn write_step(products: &[Product]) -> String {
    let mut b = B {
        out: String::new(),
        n: 0,
    };
    let app = b.add("APPLICATION_CONTEXT('core data for automotive mechanical design processes')");
    b.add(&format!(
        "APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})"
    ));
    let pctx = b.add(&format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));
    let pdctx = b.add(&format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));
    let len = b.add("( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) )");
    let ang = b.add("( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) )");
    let sr = b.add("( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() )");
    let unc = b.add(&format!(
        "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),#{len},'distance_accuracy_value','confusion accuracy')"
    ));
    let ctx = b.add(&format!(
        "( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) \
         GLOBAL_UNIT_ASSIGNED_CONTEXT((#{len},#{ang},#{sr})) REPRESENTATION_CONTEXT('Context3D','3D Context') )"
    ));

    struct Ids {
        pd: u32,
        rep: u32,
        axis: u32,
    }
    let mut ids: Vec<Ids> = Vec::new();
    let mut styled = Vec::new();
    for p in products {
        let name = match p {
            Product::Part(p) => &p.name,
            Product::Assembly { name, .. } => name,
        };
        let n = enc(name);
        let prod = b.add(&format!("PRODUCT('{n}','{n}','',(#{pctx}))"));
        let pdf = b.add(&format!("PRODUCT_DEFINITION_FORMATION('','',#{prod})"));
        let pd = b.add(&format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdctx})"));
        let pds = b.add(&format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
        let axis = b.axis([0.0, 0.0, 0.0]);
        let rep = match p {
            Product::Part(part) => {
                let g = if part.cylinder {
                    write_cylinder(&mut b, &part.name, part.dims[0], part.dims[2])
                } else {
                    write_box(&mut b, &part.name, part.dims)
                };
                if let Some(c) = part.color {
                    styled.push(style(&mut b, g.solid, c));
                }
                if let Some(c) = part.top_color {
                    styled.push(style(&mut b, g.top_face, c));
                }
                b.add(&format!(
                    "ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{},#{axis}),#{ctx})",
                    g.solid
                ))
            }
            Product::Assembly { usages, .. } => {
                let places: Vec<u32> = usages.iter().map(|u| b.axis(u.offset)).collect();
                let mut items = vec![format!("#{axis}")];
                items.extend(places.iter().map(|p| format!("#{p}")));
                let rep = b.add(&format!(
                    "SHAPE_REPRESENTATION('',({}),#{ctx})",
                    items.join(",")
                ));
                for (u, place) in usages.iter().zip(&places) {
                    let child = &ids[u.child];
                    let nauo = b.add(&format!(
                        "NEXT_ASSEMBLY_USAGE_OCCURRENCE('{}','{}','',#{pd},#{},$)",
                        enc(&u.name),
                        enc(&u.name),
                        child.pd
                    ));
                    let nds = b.add(&format!(
                        "PRODUCT_DEFINITION_SHAPE('Placement','Placement of an item',#{nauo})"
                    ));
                    let idt = b.add(&format!(
                        "ITEM_DEFINED_TRANSFORMATION('','',#{},#{place})",
                        child.axis
                    ));
                    let rr = b.add(&format!(
                        "( REPRESENTATION_RELATIONSHIP('','',#{},#{rep}) \
                         REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{idt}) SHAPE_REPRESENTATION_RELATIONSHIP() )",
                        child.rep
                    ));
                    b.add(&format!(
                        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{rr},#{nds})"
                    ));
                }
                rep
            }
        };
        b.add(&format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
        ids.push(Ids { pd, rep, axis });
    }
    if !styled.is_empty() {
        let list: Vec<String> = styled.iter().map(|s| format!("#{s}")).collect();
        b.add(&format!(
            "MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{ctx})",
            list.join(",")
        ));
    }
    format!(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('occt-bridge test'),'2;1');\n\
         FILE_NAME('test.step','2026-01-01T00:00:00',(''),(''),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN {{ 1 0 10303 214 1 1 1 1 }}'));\nENDSEC;\nDATA;\n{}ENDSEC;\nEND-ISO-10303-21;\n",
        b.out
    )
}

/// Single box part.
pub fn box_step(name: &str, dims: [f64; 3]) -> String {
    write_step(&[part(name, dims)])
}

/// One IGES line (entity 110) from (0,0,0) to (10,0,0), in millimetres.
pub fn iges_line() -> String {
    fn rec(text: &str, sec: char, seq: usize) -> String {
        format!(
            "{text:<72}{sec}{seq:>7}
"
        )
    }
    let global_params = [
        "1H,",
        "1H;",
        "4HTEST",
        "8Htest.igs",
        "4HTEST",
        "4HTEST",
        "32",
        "38",
        "6",
        "308",
        "15",
        "4HTEST",
        "1.",
        "2",
        "2HMM",
        "1",
        "1.",
        "13H20260101.000000",
        "1.E-07",
        "1000.",
        "4HTEST",
        "4HTEST",
        "11",
        "0",
        "13H20260101.000000",
    ];
    let mut glines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, p) in global_params.iter().enumerate() {
        // "1H," and "1H;" are the delimiters themselves, each followed by the separator
        let piece = format!(
            "{p}{}",
            if i + 1 == global_params.len() {
                ";"
            } else {
                ","
            }
        );
        if cur.len() + piece.len() > 72 {
            glines.push(std::mem::take(&mut cur));
        }
        cur.push_str(&piece);
    }
    glines.push(cur);
    let mut out = rec("occt-bridge test", 'S', 1);
    for (i, l) in glines.iter().enumerate() {
        out.push_str(&rec(l, 'G', i + 1));
    }
    let d1 = format!(
        "{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
        110, 1, 0, 0, 0, 0, 0, 0, "00000000"
    );
    let d2 = format!(
        "{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}{:>8}",
        110, 0, 0, 1, 0, "", "", "", 0
    );
    out.push_str(&rec(&d1, 'D', 1));
    out.push_str(&rec(&d2, 'D', 2));
    let p = format!("{:<64}{:>8}", "110,0.0,0.0,0.0,10.0,0.0,0.0;", 1);
    out.push_str(&rec(&p, 'P', 1));
    let t = format!("S{:>7}G{:>7}D{:>7}P{:>7}", 1, glines.len(), 2, 1);
    out.push_str(&rec(&t, 'T', 1));
    out
}

/// Binary STL of the box [0,dx] x [0,dy] x [0,dz] (12 triangles, outward winding).
pub fn box_stl(d: [f64; 3]) -> Vec<u8> {
    let v = |i: usize, j: usize, k: usize| {
        [
            i as f32 * d[0] as f32,
            j as f32 * d[1] as f32,
            k as f32 * d[2] as f32,
        ]
    };
    let quads: [([[f32; 3]; 4], [f32; 3]); 6] = [
        (
            [v(0, 0, 0), v(0, 1, 0), v(1, 1, 0), v(1, 0, 0)],
            [0.0, 0.0, -1.0],
        ),
        (
            [v(0, 0, 1), v(1, 0, 1), v(1, 1, 1), v(0, 1, 1)],
            [0.0, 0.0, 1.0],
        ),
        (
            [v(0, 0, 0), v(1, 0, 0), v(1, 0, 1), v(0, 0, 1)],
            [0.0, -1.0, 0.0],
        ),
        (
            [v(0, 1, 0), v(0, 1, 1), v(1, 1, 1), v(1, 1, 0)],
            [0.0, 1.0, 0.0],
        ),
        (
            [v(0, 0, 0), v(0, 0, 1), v(0, 1, 1), v(0, 1, 0)],
            [-1.0, 0.0, 0.0],
        ),
        (
            [v(1, 0, 0), v(1, 1, 0), v(1, 1, 1), v(1, 0, 1)],
            [1.0, 0.0, 0.0],
        ),
    ];
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&12u32.to_le_bytes());
    for (q, n) in quads {
        for tri in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            for c in n {
                out.extend_from_slice(&c.to_le_bytes());
            }
            for p in tri {
                for c in p {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    out
}
