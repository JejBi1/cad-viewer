//! Typed parsing of DXF entity records.
use super::loader::{hex, Loader};
use super::model::*;
use super::tags::*;

/// Pushes / updates points given as separate x (push), y and z groups.
fn vec_tag(list: &mut Vec<V3>, axis: usize, val: f64) {
    match axis {
        0 => list.push([val, 0.0, 0.0]),
        a => {
            if let Some(last) = list.last_mut() {
                last[a] = val;
            }
        }
    }
}

fn vec2_tag(list: &mut Vec<[f64; 2]>, axis: usize, val: f64) {
    match axis {
        0 => list.push([val, 0.0]),
        _ => {
            if let Some(last) = list.last_mut() {
                last[1] = val;
            }
        }
    }
}

fn set_axis(v: &mut V3, axis: usize, val: f64) {
    v[axis] = val;
}

impl<'a> Loader<'a> {
    pub(super) fn parse_vertex(&mut self, rec: &[Tag]) -> Vertex {
        let mut v = Vertex::default();
        for t in rec {
            match t.code {
                10 => v.loc[0] = t.f64(),
                20 => v.loc[1] = t.f64(),
                30 => v.loc[2] = t.f64(),
                40 => v.start_width = t.f64(),
                41 => v.end_width = t.f64(),
                42 => v.bulge = t.f64(),
                70 => v.flags = t.i32(),
                71 => v.idx[0] = t.i32(),
                72 => v.idx[1] = t.i32(),
                73 => v.idx[2] = t.i32(),
                74 => v.idx[3] = t.i32(),
                _ => {}
            }
        }
        v
    }

    pub(super) fn parse_text(&mut self, rec: &[Tag], kind: TextKind) -> Entity {
        let mut c = self.new_common();
        let mut t = TextData {
            kind,
            insert: [0.0; 3],
            align_point: [0.0; 3],
            has_align_point: false,
            height: 2.5,
            text: String::new(),
            tag: String::new(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: self.doc.standard_style,
            generation: 0,
            halign: 0,
            valign: 0,
            attr_flags: 0,
            embedded_mtext: false,
        };
        let valign_code = if kind == TextKind::Text { 73 } else { 74 };
        let mut seen_height = false;
        for tag in &rec[1..] {
            if self.common_tag(&mut c, tag) {
                continue;
            }
            match tag.code {
                10 => t.insert[0] = tag.f64(),
                20 => t.insert[1] = tag.f64(),
                30 => t.insert[2] = tag.f64(),
                11 => {
                    t.align_point[0] = tag.f64();
                    t.has_align_point = true;
                }
                21 => t.align_point[1] = tag.f64(),
                31 => t.align_point[2] = tag.f64(),
                40 => {
                    t.height = tag.f64();
                    seen_height = true;
                }
                1 => t.text = self.s(tag),
                2 if kind != TextKind::Text => t.tag = self.s(tag),
                50 => t.rotation = tag.f64(),
                41 => t.width_factor = tag.f64(),
                51 => t.oblique = tag.f64(),
                7 => t.style = self.doc.style_names.intern(&self.s(tag)),
                71 => t.generation = tag.i32(),
                72 => t.halign = tag.i32(),
                70 if kind != TextKind::Text => t.attr_flags = tag.i32(),
                101 => t.embedded_mtext = true,
                code if code == valign_code => t.valign = tag.i32(),
                _ => {}
            }
        }
        let _ = seen_height;
        Entity { c, d: Data::Text(Box::new(t)) }
    }

    fn parse_mtext(&mut self, rec: &[Tag]) -> Entity {
        let mut c = self.new_common();
        let mut m = MTextData {
            insert: [0.0; 3],
            char_height: 2.5,
            width: 0.0,
            attachment: 1,
            flow_direction: 1,
            text: String::new(),
            style: self.doc.standard_style,
            text_direction: None,
            rotation: 0.0,
            line_spacing_factor: 1.0,
            bg_fill: 0,
            bg_color: None,
            bg_true_color: None,
            box_fill_scale: 1.5,
            column_type: 0,
            column_count: 0,
            column_width: 0.0,
            column_gutter: 0.0,
            column_heights: Vec::new(),
            auto_height: false,
            defined_height: 0.0,
        };
        let mut chunks = String::new();
        let mut last = String::new();
        let mut dir = [0.0f64; 3];
        let mut has_dir = false;
        let mut in_columns = false;
        let mut n_heights = 0;
        let mut embedded = false;
        for tag in &rec[1..] {
            if tag.code == 101 {
                embedded = true; // the embedded object carries the column definition
                continue;
            }
            if embedded {
                match tag.code {
                    71 => m.column_type = tag.i32(),
                    72 => m.column_count = tag.i32(),
                    44 => m.column_width = tag.f64(),
                    45 => m.column_gutter = tag.f64(),
                    73 => m.auto_height = tag.i32() != 0,
                    46 => m.column_heights.push(tag.f64()),
                    _ => {}
                }
                continue;
            }
            if self.common_tag(&mut c, tag) {
                continue;
            }
            match tag.code {
                10 => m.insert[0] = tag.f64(),
                20 => m.insert[1] = tag.f64(),
                30 => m.insert[2] = tag.f64(),
                40 => m.char_height = tag.f64(),
                41 => m.width = tag.f64(),
                46 => m.defined_height = tag.f64(),
                71 => m.attachment = tag.i32(),
                72 => m.flow_direction = tag.i32(),
                3 => chunks.push_str(&self.s(tag)),
                1 => last = self.s(tag),
                7 => m.style = self.doc.style_names.intern(&self.s(tag)),
                11 => {
                    dir[0] = tag.f64();
                    has_dir = true;
                }
                21 => dir[1] = tag.f64(),
                31 => dir[2] = tag.f64(),
                44 => m.line_spacing_factor = tag.f64(),
                90 => m.bg_fill = tag.i32(),
                63 => m.bg_color = Some(tag.i32()),
                45 => m.box_fill_scale = tag.f64(),
                421 => m.bg_true_color = Some(tag.i64() as u32 & 0x00FF_FFFF),
                75 => {
                    m.column_type = tag.i32();
                    in_columns = true;
                }
                76 => m.column_count = tag.i32(),
                79 => m.auto_height = tag.i32() != 0,
                48 => m.column_width = tag.f64(),
                49 => m.column_gutter = tag.f64(),
                50 => {
                    if in_columns && n_heights > 0 {
                        m.column_heights.push(tag.f64());
                    } else if in_columns && m.column_type != 0 {
                        // first 50 in the column block is the count of heights
                        n_heights = tag.i32().max(0) as usize;
                        if n_heights == 0 {
                            n_heights = 1;
                        }
                    } else {
                        m.rotation = tag.f64();
                    }
                }
                _ => {}
            }
        }
        m.text = chunks + &last;
        if has_dir {
            m.text_direction = Some(dir);
        }
        Entity { c, d: Data::MText(Box::new(m)) }
    }

    pub(super) fn parse_entity(&mut self, ty: &str, rec: &[Tag]) -> Option<Entity> {
        let mut c = self.new_common();
        let d = match ty {
            "LINE" => {
                let (mut s, mut e) = ([0.0; 3], [0.0; 3]);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => s[0] = t.f64(),
                        20 => s[1] = t.f64(),
                        30 => s[2] = t.f64(),
                        11 => e[0] = t.f64(),
                        21 => e[1] = t.f64(),
                        31 => e[2] = t.f64(),
                        _ => {}
                    }
                }
                Data::Line { start: s, end: e }
            }
            "XLINE" | "RAY" => {
                let (mut s, mut dir) = ([0.0; 3], [1.0, 0.0, 0.0]);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => s[0] = t.f64(),
                        20 => s[1] = t.f64(),
                        30 => s[2] = t.f64(),
                        11 => dir[0] = t.f64(),
                        21 => dir[1] = t.f64(),
                        31 => dir[2] = t.f64(),
                        _ => {}
                    }
                }
                Data::Xline { start: s, dir, ray: ty == "RAY" }
            }
            "POINT" => {
                let mut p = [0.0; 3];
                let mut angle = 0.0;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => p[0] = t.f64(),
                        20 => p[1] = t.f64(),
                        30 => p[2] = t.f64(),
                        50 => angle = t.f64(),
                        _ => {}
                    }
                }
                Data::Point { loc: p, angle }
            }
            "CIRCLE" => {
                let (mut ctr, mut r) = ([0.0; 3], 1.0);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => ctr[0] = t.f64(),
                        20 => ctr[1] = t.f64(),
                        30 => ctr[2] = t.f64(),
                        40 => r = t.f64(),
                        _ => {}
                    }
                }
                Data::Circle { center: ctr, radius: r }
            }
            "ARC" => {
                let (mut ctr, mut r, mut a0, mut a1) = ([0.0; 3], 1.0, 0.0, 360.0);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => ctr[0] = t.f64(),
                        20 => ctr[1] = t.f64(),
                        30 => ctr[2] = t.f64(),
                        40 => r = t.f64(),
                        50 => a0 = t.f64(),
                        51 => a1 = t.f64(),
                        _ => {}
                    }
                }
                Data::Arc { center: ctr, radius: r, start: a0, end: a1 }
            }
            "ELLIPSE" => {
                let (mut ctr, mut major, mut ratio, mut p0, mut p1) =
                    ([0.0; 3], [1.0, 0.0, 0.0], 1.0, 0.0, std::f64::consts::TAU);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => ctr[0] = t.f64(),
                        20 => ctr[1] = t.f64(),
                        30 => ctr[2] = t.f64(),
                        11 => major[0] = t.f64(),
                        21 => major[1] = t.f64(),
                        31 => major[2] = t.f64(),
                        40 => ratio = t.f64(),
                        41 => p0 = t.f64(),
                        42 => p1 = t.f64(),
                        _ => {}
                    }
                }
                Data::Ellipse { center: ctr, major, ratio, start: p0, end: p1 }
            }
            "SPLINE" | "HELIX" => {
                let mut s = SplineData { helix: ty == "HELIX", degree: 3, ..Default::default() };
                let mut st = [0.0; 3];
                let mut en = [0.0; 3];
                let (mut has_st, mut has_en) = (false, false);
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        70 => s.flags = t.i32(),
                        71 => s.degree = t.i32(),
                        40 => s.knots.push(t.f64()),
                        41 => s.weights.push(t.f64()),
                        10 => vec_tag(&mut s.control_points, 0, t.f64()),
                        20 => vec_tag(&mut s.control_points, 1, t.f64()),
                        30 => vec_tag(&mut s.control_points, 2, t.f64()),
                        11 => vec_tag(&mut s.fit_points, 0, t.f64()),
                        21 => vec_tag(&mut s.fit_points, 1, t.f64()),
                        31 => vec_tag(&mut s.fit_points, 2, t.f64()),
                        12 => {
                            st[0] = t.f64();
                            has_st = true;
                        }
                        22 => st[1] = t.f64(),
                        32 => st[2] = t.f64(),
                        13 => {
                            en[0] = t.f64();
                            has_en = true;
                        }
                        23 => en[1] = t.f64(),
                        33 => en[2] = t.f64(),
                        _ => {}
                    }
                }
                if has_st {
                    s.start_tangent = Some(st);
                }
                if has_en {
                    s.end_tangent = Some(en);
                }
                Data::Spline(Box::new(s))
            }
            "LWPOLYLINE" => {
                let mut p = LwPoly::default();
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        70 => p.closed = t.i32() & 1 != 0,
                        43 => p.const_width = t.f64(),
                        38 => p.elevation = t.f64(),
                        10 => p.points.push(LwVertex { x: t.f64(), ..Default::default() }),
                        20 => {
                            if let Some(v) = p.points.last_mut() {
                                v.y = t.f64()
                            }
                        }
                        40 => {
                            if let Some(v) = p.points.last_mut() {
                                v.start_width = t.f64()
                            }
                        }
                        41 => {
                            if let Some(v) = p.points.last_mut() {
                                v.end_width = t.f64()
                            }
                        }
                        42 => {
                            if let Some(v) = p.points.last_mut() {
                                v.bulge = t.f64()
                            }
                        }
                        _ => {}
                    }
                }
                Data::LwPolyline(Box::new(p))
            }
            "POLYLINE" => {
                let mut p = PolylineData::default();
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => p.elevation[0] = t.f64(),
                        20 => p.elevation[1] = t.f64(),
                        30 => {
                            p.elevation[2] = t.f64();
                            p.has_elevation = true;
                        }
                        70 => p.flags = t.i32(),
                        40 => p.default_start_width = t.f64(),
                        41 => p.default_end_width = t.f64(),
                        71 => p.m_count = t.i32(),
                        72 => p.n_count = t.i32(),
                        _ => {}
                    }
                }
                Data::Polyline(Box::new(p))
            }
            "SOLID" | "TRACE" => {
                let mut v = [[0.0; 3]; 4];
                let mut has3 = false;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => v[0][0] = t.f64(),
                        20 => v[0][1] = t.f64(),
                        30 => v[0][2] = t.f64(),
                        11 => v[1][0] = t.f64(),
                        21 => v[1][1] = t.f64(),
                        31 => v[1][2] = t.f64(),
                        12 => v[2][0] = t.f64(),
                        22 => v[2][1] = t.f64(),
                        32 => v[2][2] = t.f64(),
                        13 => {
                            v[3][0] = t.f64();
                            has3 = true;
                        }
                        23 => v[3][1] = t.f64(),
                        33 => v[3][2] = t.f64(),
                        _ => {}
                    }
                }
                if !has3 {
                    v[3] = v[2];
                }
                Data::Solid { v, trace: ty == "TRACE" }
            }
            "3DFACE" => {
                let mut v = [[0.0; 3]; 4];
                let mut flags = 0;
                let mut has3 = false;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => v[0][0] = t.f64(),
                        20 => v[0][1] = t.f64(),
                        30 => v[0][2] = t.f64(),
                        11 => v[1][0] = t.f64(),
                        21 => v[1][1] = t.f64(),
                        31 => v[1][2] = t.f64(),
                        12 => v[2][0] = t.f64(),
                        22 => v[2][1] = t.f64(),
                        32 => v[2][2] = t.f64(),
                        13 => {
                            v[3][0] = t.f64();
                            has3 = true;
                        }
                        23 => v[3][1] = t.f64(),
                        33 => v[3][2] = t.f64(),
                        70 => flags = t.i32(),
                        _ => {}
                    }
                }
                if !has3 {
                    v[3] = v[2];
                }
                Data::Face3d { v, edge_flags: flags }
            }
            "TEXT" => return Some(self.parse_text(rec, TextKind::Text)),
            "ATTDEF" => return Some(self.parse_text(rec, TextKind::Attdef)),
            "MTEXT" => return Some(self.parse_mtext(rec)),
            "INSERT" | "ACAD_TABLE" | "ACIDBLOCKREFERENCE" => {
                let mut i = InsertData {
                    name: String::new(),
                    insert: [0.0; 3],
                    scale: [1.0; 3],
                    rotation: 0.0,
                    col_count: 1,
                    row_count: 1,
                    col_spacing: 0.0,
                    row_spacing: 0.0,
                    attribs: Vec::new(),
                    table: ty == "ACAD_TABLE",
                    from_inventor: ty == "ACIDBLOCKREFERENCE",
                    clip: None,
                    xdict: 0,
                };
                let mut in_table_sub = false;
                let mut in_xdict = false;
                for t in &rec[1..] {
                    if ty == "ACIDBLOCKREFERENCE" {
                        // the entity is unknown to ezdxf: take what looks like an INSERT, last value wins
                        match t.code {
                            2 => i.name = self.s(t),
                            10 => i.insert[0] = t.f64(),
                            20 => i.insert[1] = t.f64(),
                            30 => i.insert[2] = t.f64(),
                            41 => i.scale[0] = t.f64(),
                            42 => i.scale[1] = t.f64(),
                            43 => i.scale[2] = t.f64(),
                            50 => i.rotation = t.f64(),
                            8 => c.layer = self.doc.layer_names.intern(&self.s(t)),
                            5 => c.handle = hex(t),
                            _ => {}
                        }
                        continue;
                    }
                    if t.code == 100 {
                        in_table_sub = ty == "ACAD_TABLE" && self.s(t) != "AcDbBlockReference";
                    }
                    // extension dictionary: 102 {ACAD_XDICTIONARY, 360 <handle>, 102 }
                    if t.code == 102 {
                        in_xdict = self.s(t).starts_with("{ACAD_XDICTIONARY");
                        continue;
                    }
                    if t.code == 360 && in_xdict {
                        i.xdict = hex(t);
                        continue;
                    }
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    if in_table_sub {
                        continue;
                    }
                    match t.code {
                        2 => i.name = self.s(t),
                        10 => i.insert[0] = t.f64(),
                        20 => i.insert[1] = t.f64(),
                        30 => i.insert[2] = t.f64(),
                        41 => i.scale[0] = t.f64(),
                        42 => i.scale[1] = t.f64(),
                        43 => i.scale[2] = t.f64(),
                        50 => i.rotation = t.f64(),
                        70 => i.col_count = t.i32().max(1),
                        71 => i.row_count = t.i32().max(1),
                        44 => i.col_spacing = t.f64(),
                        45 => i.row_spacing = t.f64(),
                        _ => {}
                    }
                }
                Data::Insert(Box::new(i))
            }
            "DIMENSION" => {
                let mut d = DimData::default();
                let mut ins = [0.0; 3];
                let mut has_ins = false;
                let mut sub = String::new();
                for t in &rec[1..] {
                    if t.code == 100 {
                        sub = self.s(t);
                    }
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        2 => d.block = self.s(t),
                        10 => d.defpoint[0] = t.f64(),
                        20 => d.defpoint[1] = t.f64(),
                        30 => d.defpoint[2] = t.f64(),
                        11 => d.text_midpoint[0] = t.f64(),
                        21 => d.text_midpoint[1] = t.f64(),
                        31 => d.text_midpoint[2] = t.f64(),
                        12 => {
                            ins[0] = t.f64();
                            has_ins = true;
                        }
                        22 => ins[1] = t.f64(),
                        32 => ins[2] = t.f64(),
                        13 => d.defpoint2[0] = t.f64(),
                        23 => d.defpoint2[1] = t.f64(),
                        33 => d.defpoint2[2] = t.f64(),
                        14 => d.defpoint3[0] = t.f64(),
                        24 => d.defpoint3[1] = t.f64(),
                        34 => d.defpoint3[2] = t.f64(),
                        15 => d.defpoint4[0] = t.f64(),
                        25 => d.defpoint4[1] = t.f64(),
                        35 => d.defpoint4[2] = t.f64(),
                        16 => d.defpoint5[0] = t.f64(),
                        26 => d.defpoint5[1] = t.f64(),
                        36 => d.defpoint5[2] = t.f64(),
                        70 if sub == "AcDbDimension" || sub.is_empty() => d.dimtype = t.i32(),
                        1 => d.text = self.s(t),
                        50 => d.angle = t.f64(),
                        53 => d.text_rotation = t.f64(),
                        42 => d.actual_measurement = Some(t.f64()),
                        _ => {}
                    }
                }
                if has_ins {
                    d.insert = Some(ins);
                }
                Data::Dimension(Box::new(d))
            }
            "LEADER" => {
                let mut l = LeaderData { arrow_head: true, ..Default::default() };
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        3 => l.dimstyle = self.s(t),
                        71 => l.arrow_head = t.i32() != 0,
                        72 => l.spline_path = t.i32() != 0,
                        73 => l.annotation_type = t.i32(),
                        75 => l.hookline = t.i32() != 0,
                        40 => l.text_height = t.f64(),
                        10 => vec_tag(&mut l.vertices, 0, t.f64()),
                        20 => vec_tag(&mut l.vertices, 1, t.f64()),
                        30 => vec_tag(&mut l.vertices, 2, t.f64()),
                        _ => {}
                    }
                }
                Data::Leader(Box::new(l))
            }
            "MULTILEADER" | "MLEADER" => Data::MLeader(Box::new(self.parse_mleader(&mut c, rec, ty == "MULTILEADER"))),
            "MLINE" => {
                let mut m = MLineData { scale: 1.0, ..Default::default() };
                let mut cur: Option<MLineVertex> = None;
                let mut params: Vec<f64> = Vec::new();
                let mut remaining = 0i64;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        2 => m.style_name = self.s(t),
                        40 if cur.is_none() => m.scale = t.f64(),
                        70 => m.justification = t.i32(),
                        71 => m.closed = t.i32() & 2 != 0,
                        10 => m.start[0] = t.f64(),
                        20 => m.start[1] = t.f64(),
                        30 => m.start[2] = t.f64(),
                        11 => {
                            if let Some(v) = cur.take() {
                                m.vertices.push(v);
                            }
                            cur = Some(MLineVertex { location: [t.f64(), 0.0, 0.0], ..Default::default() });
                        }
                        21 => {
                            if let Some(v) = cur.as_mut() {
                                v.location[1] = t.f64()
                            }
                        }
                        31 => {
                            if let Some(v) = cur.as_mut() {
                                v.location[2] = t.f64()
                            }
                        }
                        12 | 22 | 32 => {
                            if let Some(v) = cur.as_mut() {
                                v.direction[(t.code / 10 - 1) as usize] = t.f64()
                            }
                        }
                        13 | 23 | 33 => {
                            if let Some(v) = cur.as_mut() {
                                v.miter[(t.code / 10 - 1) as usize] = t.f64()
                            }
                        }
                        74 => {
                            remaining = t.i64();
                            params = Vec::new();
                            if remaining == 0 {
                                if let Some(v) = cur.as_mut() {
                                    v.elements.push(Vec::new());
                                }
                            }
                        }
                        41 if cur.is_some() && remaining > 0 => {
                            params.push(t.f64());
                            remaining -= 1;
                            if remaining == 0 {
                                if let Some(v) = cur.as_mut() {
                                    v.elements.push(std::mem::take(&mut params));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(v) = cur.take() {
                    m.vertices.push(v);
                }
                Data::MLine(Box::new(m))
            }
            "HATCH" | "MPOLYGON" => Data::Hatch(Box::new(self.parse_hatch(&mut c, rec, ty == "MPOLYGON"))),
            "IMAGE" | "WIPEOUT" => {
                let mut im = ImageData { u: [1.0, 0.0, 0.0], v: [0.0, 1.0, 0.0], size: [1.0, 1.0], flags: 3, brightness: 50, contrast: 50, ..Default::default() };
                let mut clip: Vec<[f64; 2]> = Vec::new();
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => im.insert[0] = t.f64(),
                        20 => im.insert[1] = t.f64(),
                        30 => im.insert[2] = t.f64(),
                        11 => im.u[0] = t.f64(),
                        21 => im.u[1] = t.f64(),
                        31 => im.u[2] = t.f64(),
                        12 => im.v[0] = t.f64(),
                        22 => im.v[1] = t.f64(),
                        32 => im.v[2] = t.f64(),
                        13 => im.size[0] = t.f64(),
                        23 => im.size[1] = t.f64(),
                        340 => im.def_handle = hex(t),
                        70 => im.flags = t.i32(),
                        280 => im.clipping = t.i32() != 0,
                        281 => im.brightness = t.i32(),
                        282 => im.contrast = t.i32(),
                        283 => im.fade = t.i32(),
                        71 => im.clip_type = t.i32(),
                        290 => im.clip_mode = t.i32(),
                        14 => vec2_tag(&mut clip, 0, t.f64()),
                        24 => vec2_tag(&mut clip, 1, t.f64()),
                        _ => {}
                    }
                }
                im.clip_vertices = clip;
                if ty == "IMAGE" {
                    Data::Image(Box::new(im))
                } else {
                    Data::Wipeout(Box::new(im))
                }
            }
            "VIEWPORT" => {
                let mut v = ViewportData { view_direction: [0.0, 0.0, 1.0], view_height: 1.0, ..Default::default() };
                let mut had_status = false;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        10 => v.center[0] = t.f64(),
                        20 => v.center[1] = t.f64(),
                        30 => v.center[2] = t.f64(),
                        40 => v.width = t.f64(),
                        41 => v.height = t.f64(),
                        68 => {
                            v.status = t.i32();
                            had_status = true;
                        }
                        69 => v.id = t.i32(),
                        12 => v.view_center[0] = t.f64(),
                        22 => v.view_center[1] = t.f64(),
                        16 => v.view_direction[0] = t.f64(),
                        26 => v.view_direction[1] = t.f64(),
                        36 => v.view_direction[2] = t.f64(),
                        17 => v.view_target[0] = t.f64(),
                        27 => v.view_target[1] = t.f64(),
                        37 => v.view_target[2] = t.f64(),
                        45 => v.view_height = t.f64(),
                        51 => v.twist_angle = t.f64(),
                        90 => v.flags = t.i32(),
                        340 => v.clip_handle = hex(t),
                        331 => v.frozen_layers.push(hex(t)),
                        281 => v.render_mode = t.i32(),
                        _ => {}
                    }
                }
                let _ = had_status;
                Data::Viewport(Box::new(v))
            }
            "MESH" => {
                let mut m = MeshData::default();
                // sequence: 92 vertex count, vertices, 93 face list size, 90 values ...
                let mut mode = 0; // 0 none, 1 vertices, 2 faces
                let mut face_vals: Vec<i64> = Vec::new();
                let mut face_left = 0i64;
                let mut verts_left = 0i64;
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                    match t.code {
                        92 => {
                            verts_left = t.i64();
                            mode = 1;
                        }
                        10 if mode == 1 && verts_left > 0 => vec_tag(&mut m.vertices, 0, t.f64()),
                        20 if mode == 1 => vec_tag(&mut m.vertices, 1, t.f64()),
                        30 if mode == 1 => {
                            vec_tag(&mut m.vertices, 2, t.f64());
                            verts_left -= 1;
                        }
                        93 => {
                            face_left = t.i64();
                            mode = 2;
                        }
                        94 => mode = 3,
                        90 if mode == 2 && face_left > 0 => {
                            face_vals.push(t.i64());
                            face_left -= 1;
                        }
                        _ => {}
                    }
                }
                let mut i = 0usize;
                while i < face_vals.len() {
                    let n = face_vals[i].max(0) as usize;
                    i += 1;
                    if i + n > face_vals.len() {
                        break;
                    }
                    let f: Vec<u32> = face_vals[i..i + n].iter().map(|&v| v.max(0) as u32).collect();
                    i += n;
                    m.faces.push(f);
                }
                Data::Mesh(Box::new(m))
            }
            // entity types that are not graphic entities of a layout
            "ENDBLK" | "ENDSEC" | "ENDTAB" | "SEQEND" => return None,
            other => {
                for t in &rec[1..] {
                    if self.common_tag(&mut c, t) {
                        continue;
                    }
                }
                Data::Other(other.to_string())
            }
        };
        Some(Entity { c, d })
    }

    // ------------------------------------------------------------------------------------------ HATCH
    #[allow(unused_assignments)]
    fn parse_hatch(&mut self, c: &mut Common, rec: &[Tag], mpolygon: bool) -> HatchData {
        let mut h = HatchData { pattern_scale: 1.0, mpolygon, ..Default::default() };
        let mut gradient_seen = false;
        // state machine over the boundary path and pattern definition groups
        #[derive(PartialEq)]
        enum St {
            Head,
            Paths,
            Pattern,
        }
        let mut st = St::Head;
        let mut poly_verts: Vec<[f64; 3]> = Vec::new();
        let mut poly_closed = false;
        let mut poly_bulge = false;
        let mut path_flags = 0;
        let mut is_poly = false;
        let mut edges: Vec<HatchEdge> = Vec::new();
        let mut have_path = false;
        // edge under construction
        let mut etype = 0;
        let mut e_pts: Vec<[f64; 2]> = Vec::new(); // line: start, end / arc: center / ellipse: center, major / spline: control points, fit points
        let mut e_radius = 0.0;
        let mut e_ratio = 1.0;
        let mut e_a0 = 0.0;
        let mut e_a1 = 0.0;
        let mut e_ccw = true;
        let mut e_degree = 3;
        let mut e_rational = false;
        let mut e_periodic = false;
        let mut e_knots: Vec<f64> = Vec::new();
        let mut e_weights: Vec<f64> = Vec::new();
        let mut e_ctrl: Vec<[f64; 2]> = Vec::new();
        let mut e_fit: Vec<[f64; 2]> = Vec::new();
        let mut e_st: Option<[f64; 2]> = None;
        let mut e_en: Option<[f64; 2]> = None;
        let mut have_edge = false;
        let mut pat: Option<PatternLine> = None;

        macro_rules! finish_edge {
            () => {
                if have_edge {
                    match etype {
                        1 if e_pts.len() >= 2 => edges.push(HatchEdge::Line { start: e_pts[0], end: e_pts[1] }),
                        2 if !e_pts.is_empty() => edges.push(HatchEdge::Arc {
                            center: e_pts[0],
                            radius: e_radius,
                            start: e_a0,
                            end: e_a1,
                            ccw: e_ccw,
                        }),
                        3 if e_pts.len() >= 2 => edges.push(HatchEdge::Ellipse {
                            center: e_pts[0],
                            major: e_pts[1],
                            ratio: e_ratio,
                            start: e_a0,
                            end: e_a1,
                            ccw: e_ccw,
                        }),
                        4 => edges.push(HatchEdge::Spline {
                            degree: e_degree,
                            rational: e_rational,
                            periodic: e_periodic,
                            knots: std::mem::take(&mut e_knots),
                            control_points: std::mem::take(&mut e_ctrl),
                            weights: std::mem::take(&mut e_weights),
                            fit_points: std::mem::take(&mut e_fit),
                            start_tangent: e_st.take(),
                            end_tangent: e_en.take(),
                        }),
                        _ => {}
                    }
                }
                have_edge = false;
                e_pts.clear();
                e_knots.clear();
                e_weights.clear();
                e_ctrl.clear();
                e_fit.clear();
                e_st = None;
                e_en = None;
                e_radius = 0.0;
                e_ratio = 1.0;
                e_a0 = 0.0;
                e_a1 = 0.0;
                e_ccw = true;
            };
        }
        macro_rules! finish_path {
            () => {
                finish_edge!();
                if have_path {
                    if is_poly {
                        h.paths.push(HatchPath::Poly {
                            flags: path_flags,
                            closed: poly_closed,
                            vertices: std::mem::take(&mut poly_verts),
                        });
                    } else {
                        h.paths.push(HatchPath::Edges { flags: path_flags, edges: std::mem::take(&mut edges) });
                    }
                }
                have_path = false;
                poly_verts.clear();
                edges.clear();
            };
        }

        for t in &rec[1..] {
            if self.common_tag(c, t) {
                continue;
            }
            match t.code {
                30 if st == St::Head => h.elevation = t.f64(),
                2 => h.pattern_name = self.s(t),
                // HATCH: 70 solid fill. MPOLYGON: 70 is the version, 71 the solid fill flag (ezdxf `MPolygon`)
                70 if !mpolygon => h.solid = t.i32() != 0,
                71 if mpolygon => h.solid = t.i32() != 0,
                63 if mpolygon => h.fill_color = Some(t.i32()),
                11 if mpolygon && st != St::Paths => h.offset = Some([t.f64(), h.offset.map(|v| v[1]).unwrap_or(0.0), 0.0]),
                21 if mpolygon && st != St::Paths => h.offset = Some([h.offset.map(|v| v[0]).unwrap_or(0.0), t.f64(), 0.0]),
                91 => st = St::Paths,
                92 if st == St::Paths => {
                    finish_path!();
                    path_flags = t.i32();
                    is_poly = path_flags & 2 != 0;
                    have_path = true;
                    poly_closed = false;
                    poly_bulge = false;
                    etype = 0;
                }
                72 if st == St::Paths => {
                    if is_poly {
                        poly_bulge = t.i32() != 0;
                    } else {
                        finish_edge!();
                        etype = t.i32();
                        have_edge = true;
                    }
                }
                73 if st == St::Paths => {
                    if is_poly {
                        poly_closed = t.i32() != 0;
                    } else if etype == 4 {
                        e_rational = t.i32() != 0;
                    } else {
                        e_ccw = t.i32() != 0;
                    }
                }
                74 if st == St::Paths && !is_poly => e_periodic = t.i32() != 0,
                94 if st == St::Paths && !is_poly => e_degree = t.i32(),
                10 if st == St::Paths => {
                    if is_poly {
                        poly_verts.push([t.f64(), 0.0, 0.0]);
                    } else if etype == 4 {
                        e_ctrl.push([t.f64(), 0.0]);
                    } else {
                        e_pts.push([t.f64(), 0.0]);
                    }
                }
                20 if st == St::Paths => {
                    if is_poly {
                        if let Some(v) = poly_verts.last_mut() {
                            v[1] = t.f64();
                        }
                    } else if etype == 4 {
                        if let Some(v) = e_ctrl.last_mut() {
                            v[1] = t.f64();
                        }
                    } else if let Some(v) = e_pts.last_mut() {
                        v[1] = t.f64();
                    }
                }
                42 if st == St::Paths => {
                    if is_poly {
                        if poly_bulge {
                            if let Some(v) = poly_verts.last_mut() {
                                v[2] = t.f64();
                            }
                        }
                    } else {
                        e_weights.push(t.f64());
                    }
                }
                11 if st == St::Paths && !is_poly => {
                    if etype == 4 {
                        e_fit.push([t.f64(), 0.0]);
                    } else {
                        e_pts.push([t.f64(), 0.0]);
                    }
                }
                21 if st == St::Paths && !is_poly => {
                    if etype == 4 {
                        if let Some(v) = e_fit.last_mut() {
                            v[1] = t.f64();
                        }
                    } else if let Some(v) = e_pts.last_mut() {
                        v[1] = t.f64();
                    }
                }
                12 if st == St::Paths => e_st = Some([t.f64(), e_st.map(|v| v[1]).unwrap_or(0.0)]),
                22 if st == St::Paths => e_st = Some([e_st.map(|v| v[0]).unwrap_or(0.0), t.f64()]),
                13 if st == St::Paths => e_en = Some([t.f64(), e_en.map(|v| v[1]).unwrap_or(0.0)]),
                23 if st == St::Paths => e_en = Some([e_en.map(|v| v[0]).unwrap_or(0.0), t.f64()]),
                40 if st == St::Paths && !is_poly => match etype {
                    2 => e_radius = t.f64(),
                    3 => e_ratio = t.f64(),
                    4 => e_knots.push(t.f64()),
                    _ => {}
                },
                50 if st == St::Paths && !is_poly => e_a0 = t.f64(),
                51 if st == St::Paths && !is_poly => e_a1 = t.f64(),
                75 => {
                    finish_path!();
                    st = St::Head;
                    h.hatch_style = t.i32();
                }
                52 => h.pattern_angle = t.f64(),
                41 => h.pattern_scale = t.f64(),
                77 => h.pattern_double = t.i32() != 0,
                78 => st = St::Pattern,
                53 if st == St::Pattern => {
                    if let Some(p) = pat.take() {
                        h.pattern.push(p);
                    }
                    pat = Some(PatternLine { angle: t.f64(), ..Default::default() });
                }
                43 if st == St::Pattern => {
                    if let Some(p) = pat.as_mut() {
                        p.base[0] = t.f64()
                    }
                }
                44 if st == St::Pattern => {
                    if let Some(p) = pat.as_mut() {
                        p.base[1] = t.f64()
                    }
                }
                45 if st == St::Pattern => {
                    if let Some(p) = pat.as_mut() {
                        p.offset[0] = t.f64()
                    }
                }
                46 if st == St::Pattern => {
                    if let Some(p) = pat.as_mut() {
                        p.offset[1] = t.f64()
                    }
                }
                49 if st == St::Pattern => {
                    if let Some(p) = pat.as_mut() {
                        p.dashes.push(t.f64())
                    }
                }
                450 => {
                    h.gradient = t.i32() == 1;
                    gradient_seen = true;
                }
                453 if gradient_seen => h.gradient_colors = t.i32(),
                421 if gradient_seen && h.gradient_color1.is_none() => h.gradient_color1 = Some(t.i64() as u32 & 0x00FF_FFFF),
                _ => {}
            }
            // the groups of the pattern definition after the paths: 76 pattern type needs no state change
            if t.code == 76 && st == St::Paths {
                finish_path!();
                st = St::Head;
            }
        }
        finish_path!();
        if let Some(p) = pat.take() {
            h.pattern.push(p);
        }
        let _ = set_axis;
        h
    }

    // ------------------------------------------------------------------------------------------ MLEADER
    fn parse_mleader(&mut self, c: &mut Common, rec: &[Tag], old_name: bool) -> MLeaderData {
        let mut m = MLeaderData { old_name, leader_type: 1, scale: 1.0, text_direction: [1.0, 0.0, 0.0], block_scale: [1.0; 3], arrow_size: 4.0, ..Default::default() };
        // CONTEXT_DATA{ ... } holds the geometry; the entity level groups hold the style overrides
        let mut depth = 0; // 300 CONTEXT_DATA{ -> 1, 302 LEADER{ -> 2, 304 LEADER_LINE{ -> 3
        let mut cur_line: Vec<V3> = Vec::new();
        let mut cur_lines: Vec<Vec<V3>> = Vec::new();
        let mut cur_branch = MLeaderLine::default();
        let mut in_context = false;
        for t in &rec[1..] {
            if self.common_tag(c, t) {
                continue;
            }
            let s = if kind_of(t.code) == Kind::Str { self.s(t) } else { String::new() };
            match t.code {
                300 if s.starts_with("CONTEXT_DATA") => {
                    in_context = true;
                    depth = 1;
                    m.has_context = true;
                }
                301 if in_context && depth == 1 => in_context = false,
                302 if in_context && s.starts_with("LEADER") => depth = 2,
                303 if in_context && depth == 2 => {
                    cur_branch.vertices = std::mem::take(&mut cur_lines);
                    m.leaders.push(std::mem::take(&mut cur_branch));
                    depth = 1;
                }
                304 if in_context && depth == 2 && s.starts_with("LEADER_LINE") => {
                    depth = 3;
                    cur_line = Vec::new();
                }
                305 if in_context && depth == 3 => {
                    cur_lines.push(std::mem::take(&mut cur_line));
                    depth = 2;
                }
                _ => {}
            }
            if in_context {
                match (depth, t.code) {
                    (3, 10) => vec_tag(&mut cur_line, 0, t.f64()),
                    (3, 20) => vec_tag(&mut cur_line, 1, t.f64()),
                    (3, 30) => vec_tag(&mut cur_line, 2, t.f64()),
                    (1, 40) => m.scale = t.f64(),
                    (1, 41) => m.text_height = t.f64(),
                    (1, 140) => m.arrow_size = t.f64(),
                    (1, 145) => m.landing_gap = t.f64(),
                    (1, 290) => {
                        if t.i32() != 0 {
                            m.content_type = 2; // MTEXT
                        }
                    }
                    (1, 296) => {
                        if t.i32() != 0 {
                            m.content_type = 1; // BLOCK
                        }
                    }
                    (1, 304) => m.text = s,
                    (1, 13) => m.text_direction[0] = t.f64(),
                    (1, 23) => m.text_direction[1] = t.f64(),
                    (1, 33) => m.text_direction[2] = t.f64(),
                    (1, 12) => m.text_location[0] = t.f64(),
                    (1, 22) => m.text_location[1] = t.f64(),
                    (1, 32) => m.text_location[2] = t.f64(),
                    (1, 42) => m.text_rotation = t.f64(),
                    (1, 43) => m.text_width = t.f64(),
                    (1, 44) => {
                        if m.text_height == 0.0 {
                            m.text_height = t.f64()
                        }
                    }
                    (1, 171) => m.text_attachment = t.i32(),
                    (1, 341) => m.block_handle = hex(t),
                    (1, 15) => m.block_location[0] = t.f64(),
                    (1, 25) => m.block_location[1] = t.f64(),
                    (1, 35) => m.block_location[2] = t.f64(),
                    (1, 16) => m.block_scale[0] = t.f64(),
                    (1, 26) => m.block_scale[1] = t.f64(),
                    (1, 36) => m.block_scale[2] = t.f64(),
                    (1, 46) => m.block_rotation = t.f64(),
                    (2, 40) => cur_branch.dogleg_length = t.f64(),
                    (2, 10) => cur_branch.last_point[0] = t.f64(),
                    (2, 20) => cur_branch.last_point[1] = t.f64(),
                    (2, 30) => cur_branch.last_point[2] = t.f64(),
                    (2, 11) => {
                        cur_branch.dogleg[0] = t.f64();
                        cur_branch.has_dogleg_vector = true;
                    }
                    (2, 21) => cur_branch.dogleg[1] = t.f64(),
                    (2, 31) => cur_branch.dogleg[2] = t.f64(),
                    _ => {}
                }
            } else {
                match t.code {
                    172 => {
                        let v = t.i32();
                        if v == 1 || v == 2 {
                            // 1 block, 2 mtext, 3 tolerance, 0 none
                            m.content_type = v;
                        }
                    }
                    291 => m.dogleg = t.i32() != 0,
                    170 => m.leader_type = t.i32(),
                    41 => m.dogleg_length = t.f64(),
                    42 => m.arrow_size = t.f64(),
                    342 => m.arrow_head = true,
                    _ => {}
                }
            }
        }
        m
    }
}
