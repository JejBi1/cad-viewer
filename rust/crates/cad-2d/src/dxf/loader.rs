//! DXF section parser: header, tables, blocks, entities and the objects the viewer needs.
use super::model::*;
use super::tags::*;
use std::collections::HashMap;

/// One group-0 record: the type name and every tag up to the next group 0.
pub type Rec<'a> = Vec<Tag<'a>>;

struct Cursor<'a> {
    r: TagReader<'a>,
    peeked: Option<Tag<'a>>,
}

impl<'a> Cursor<'a> {
    fn next(&mut self) -> Option<Tag<'a>> {
        self.peeked.take().or_else(|| self.r.next_tag())
    }

    fn peek_code(&mut self) -> Option<i32> {
        if self.peeked.is_none() {
            self.peeked = self.r.next_tag();
        }
        self.peeked.as_ref().map(|t| t.code)
    }

    /// Reads the next record: a group 0 tag and everything up to the next group 0.
    fn record(&mut self, buf: &mut Rec<'a>) -> bool {
        buf.clear();
        loop {
            match self.next() {
                Some(t) if t.code == 0 => {
                    buf.push(t);
                    break;
                }
                Some(_) => self.r.errors += 1, // stray tag outside of a record
                None => return false,
            }
        }
        while let Some(code) = self.peek_code() {
            if code == 0 {
                break;
            }
            if let Some(t) = self.next() {
                buf.push(t);
            }
        }
        true
    }
}

/// Entity list under construction. `open` is a composite entity (INSERT with ATTRIBs, POLYLINE with VERTEXes) that
/// still collects children until SEQEND.
#[derive(Default)]
struct Sink {
    list: Vec<Entity>,
    open: Option<usize>,
}

pub struct Loader<'a> {
    cur: Cursor<'a>,
    pub(super) enc: TextEncoding,
    pub(super) doc: Document,
    entities: Sink,
    record_names: HashMap<u64, String>,
    record_by_name: HashMap<String, u64>,
    version: i32,
    pub(super) extra_errors: u32,
    /// DICTIONARY objects: handle -> (entry name, handle of the entry)
    dicts: HashMap<u64, Vec<(String, u64)>>,
    /// SPATIAL_FILTER objects (block clipping paths) by handle
    spatial: HashMap<u64, SpatialRaw>,
    /// XRECORDs that carry an inverted clipping path, by handle
    xrecords: HashMap<u64, InvertedRaw>,
}

/// A SPATIAL_FILTER as stored in the file.
#[derive(Default)]
struct SpatialRaw {
    boundary: Vec<[f64; 2]>,
    /// 3 x 4 matrix (row major, as the DXF lists it) from the filter coordinate system to block coordinates; None
    /// when the file lists fewer than 24 matrix values
    inverse_insert: Option<[f64; 12]>,
    enabled: bool,
    xdict: u64,
}

/// The two sections of an `ACAD_XREC_ROUNDTRIP` XRECORD that describe an inverted clipping path.
#[derive(Default)]
struct InvertedRaw {
    inverted_clip: Option<Vec<[f64; 2]>>,
    compare: Vec<[f64; 2]>,
}

pub fn load_document(data: &[u8]) -> Document {
    let mut l = Loader {
        cur: Cursor { r: TagReader::new(data), peeked: None },
        enc: TextEncoding(encoding_rs::WINDOWS_1252),
        doc: Document::new(),
        entities: Sink::default(),
        record_names: HashMap::new(),
        record_by_name: HashMap::new(),
        version: 0,
        extra_errors: 0,
        dicts: HashMap::new(),
        spatial: HashMap::new(),
        xrecords: HashMap::new(),
    };
    l.run();
    l.finish()
}

pub(super) fn hex(t: &Tag) -> u64 {
    let s = trim_ascii(&t.val);
    std::str::from_utf8(s).ok().and_then(|s| u64::from_str_radix(s, 16).ok()).unwrap_or(0)
}

/// `AC1027` -> 1027, `AC1009` -> 1009
fn version_number(s: &str) -> i32 {
    s.trim().trim_start_matches("AC").parse::<i32>().unwrap_or(0)
}

impl<'a> Loader<'a> {
    pub(super) fn s(&self, t: &Tag) -> String {
        unescape_unicode(&self.enc.decode(&t.val))
    }

    fn run(&mut self) {
        let mut rec: Rec = Vec::new();
        let mut section = String::new();
        let mut table = String::new();
        let mut block: Option<(BlockDef, Sink)> = None;
        let mut saw_eof = false;
        while self.cur.record(&mut rec) {
            let ty = String::from_utf8_lossy(trim_ascii(&rec[0].val)).to_ascii_uppercase();
            match ty.as_str() {
                "EOF" => {
                    saw_eof = true;
                    break;
                }
                "SECTION" => {
                    section = rec.iter().find(|t| t.code == 2).map(|t| self.s(t).to_ascii_uppercase()).unwrap_or_default();
                    if section == "HEADER" {
                        self.parse_header(&rec);
                    }
                    continue;
                }
                "ENDSEC" => {
                    if let Some((b, sink)) = block.take() {
                        self.close_block(b, sink); // BLOCKS section without ENDBLK
                    }
                    section.clear();
                    continue;
                }
                _ => {}
            }
            match section.as_str() {
                "TABLES" => match ty.as_str() {
                    "TABLE" => {
                        table = rec.iter().find(|t| t.code == 2).map(|t| self.s(t).to_ascii_uppercase()).unwrap_or_default();
                    }
                    "ENDTAB" => table.clear(),
                    _ => self.parse_table_entry(&table, &ty, &rec),
                },
                "BLOCKS" => match ty.as_str() {
                    "BLOCK" => {
                        if let Some((b, sink)) = block.take() {
                            self.close_block(b, sink);
                        }
                        block = Some((self.parse_block_head(&rec), Sink::default()));
                    }
                    "ENDBLK" => {
                        if let Some((b, sink)) = block.take() {
                            self.close_block(b, sink);
                        }
                    }
                    _ => {
                        if let Some((_, sink)) = block.as_mut() {
                            let mut taken = std::mem::take(sink);
                            self.entity_record(&ty, &rec, &mut taken);
                            *sink = taken;
                        }
                    }
                },
                "ENTITIES" => {
                    let mut taken = std::mem::take(&mut self.entities);
                    self.entity_record(&ty, &rec, &mut taken);
                    self.entities = taken;
                }
                "OBJECTS" => self.parse_object(&ty, &rec),
                _ => {}
            }
        }
        if !saw_eof {
            self.extra_errors += 1;
        }
        if let Some((b, sink)) = block.take() {
            self.close_block(b, sink);
        }
    }

    fn close_block(&mut self, mut b: BlockDef, sink: Sink) {
        b.entities = sink.list;
        let key = b.name.to_lowercase();
        if !self.doc.blocks.contains_key(&key) {
            self.doc.block_order.push(key.clone());
        }
        self.doc.blocks.insert(key, b);
    }

    // ------------------------------------------------------------------------------------------ header
    fn parse_header(&mut self, rec: &[Tag]) {
        let mut name = String::new();
        let mut vals: Vec<f64> = Vec::new();
        let mut strs: Vec<String> = Vec::new();
        fn flush(this: &mut Loader, name: &mut String, vals: &mut Vec<f64>, strs: &mut Vec<String>) {
            if !name.is_empty() {
                this.doc.header.values.insert(name.clone(), std::mem::take(vals));
                if let Some(s) = strs.first() {
                    this.doc.header.strings.insert(name.clone(), s.clone());
                }
                strs.clear();
            }
        }
        for t in rec {
            if t.code == 9 {
                flush(self, &mut name, &mut vals, &mut strs);
                name = String::from_utf8_lossy(trim_ascii(&t.val)).to_string();
                continue;
            }
            if t.code == 0 || (t.code == 2 && name.is_empty()) {
                continue;
            }
            match kind_of(t.code) {
                Kind::Str => strs.push(self.s(t)),
                Kind::Chunk => {}
                _ => vals.push(t.f64()),
            }
        }
        flush(self, &mut name, &mut vals, &mut strs);
        let ver = self.doc.header.strings.get("$ACADVER").cloned().unwrap_or_default();
        self.version = version_number(&ver);
        self.doc.dxf_version_code = ver.trim().to_string();
        self.doc.header.version = ver;
        // R2007 and newer are UTF-8, older files use the code page of $DWGCODEPAGE
        if self.version >= 1021 {
            self.enc = TextEncoding::utf8();
        } else if let Some(cp) = self.doc.header.strings.get("$DWGCODEPAGE") {
            if let Some(e) = TextEncoding::from_codepage(cp) {
                self.enc = e;
            }
        }
    }

    // ------------------------------------------------------------------------------------------ tables
    fn parse_table_entry(&mut self, table: &str, ty: &str, rec: &[Tag]) {
        match (table, ty) {
            ("LAYER", "LAYER") => {
                let mut name = String::new();
                let mut def = LayerDef {
                    name: String::new(),
                    id: 0,
                    handle: 0,
                    flags: 0,
                    color: 7,
                    true_color: None,
                    linetype: self.doc.continuous,
                    lineweight: -3,
                    transparency: 0.0,
                    plot: true,
                };
                let mut lt = String::from("Continuous");
                // XDATA of the application AcCmTransparency: the first value is the layer transparency (ezdxf
                // `Layer.transparency`), 0x020000AA with AA = alpha (255 opaque). Group 440 does not exist in a LAYER record.
                let mut in_transparency = false;
                let mut transparency_done = false;
                for t in rec {
                    if t.code >= 1000 {
                        if t.code == 1001 {
                            in_transparency = !transparency_done && self.s(t) == "AcCmTransparency";
                        } else if in_transparency {
                            in_transparency = false;
                            transparency_done = true;
                            let raw = t.i64();
                            if raw & 0x0200_0000 != 0 {
                                def.transparency = 1.0 - (raw & 0xFF) as f64 / 255.0;
                            }
                        }
                        continue;
                    }
                    match t.code {
                        2 => name = self.s(t),
                        5 => def.handle = hex(t),
                        70 => def.flags = t.i32(),
                        62 => def.color = t.i32(),
                        420 => def.true_color = Some(t.i64() as u32 & 0x00FF_FFFF),
                        6 => lt = self.s(t),
                        370 => def.lineweight = t.i32(),
                        290 => def.plot = t.i32() != 0,
                        _ => {}
                    }
                }
                def.id = self.doc.layer_names.intern(&name);
                def.name = name;
                def.linetype = self.doc.linetype_names.intern(&lt);
                self.doc.layer_by_handle.insert(def.handle, def.id);
                // a duplicate layer name replaces the earlier one
                self.doc.add_layer(def);
            }
            ("LTYPE", "LTYPE") => {
                let mut def = LinetypeDef::default();
                for t in rec {
                    match t.code {
                        2 => def.name = self.s(t),
                        3 => def.description = self.s(t),
                        40 => def.pattern_length = t.f64(),
                        49 => def.elements.push(t.f64()),
                        340 => def.complex = true,
                        _ => {}
                    }
                }
                self.doc.linetype_names.intern(&def.name);
                self.doc.linetypes.push(def);
            }
            ("STYLE", "STYLE") => {
                let mut def = StyleDef { width_factor: 1.0, ..Default::default() };
                let mut in_acad_xdata = false;
                let mut xdata_n = 0;
                for t in rec {
                    match t.code {
                        2 => def.name = self.s(t),
                        3 => def.font = self.s(t),
                        4 => def.bigfont = self.s(t),
                        40 => def.fixed_height = t.f64(),
                        41 => def.width_factor = t.f64(),
                        50 => def.oblique = t.f64(),
                        71 => def.generation = t.i32(),
                        1001 => {
                            in_acad_xdata = self.s(t).eq_ignore_ascii_case("ACAD");
                            xdata_n = 0;
                        }
                        1000 if in_acad_xdata && xdata_n == 0 => {
                            def.family = self.s(t);
                            xdata_n = 1;
                        }
                        1071 if in_acad_xdata && xdata_n == 1 => {
                            let v = t.i64();
                            def.italic = v & 0x0100_0000 != 0;
                            def.bold = v & 0x0200_0000 != 0;
                            xdata_n = 2;
                        }
                        _ => {}
                    }
                }
                if def.width_factor == 0.0 {
                    def.width_factor = 1.0;
                }
                self.doc.style_names.intern(&def.name);
                self.doc.styles.push(def);
            }
            ("DIMSTYLE", "DIMSTYLE") => {
                let mut d = DimStyleDef::default();
                for t in rec {
                    match t.code {
                        2 => d.name = self.s(t),
                        40 => d.scale = t.f64(),
                        41 => d.arrow_size = t.f64(),
                        77 => d.text_above = t.i32(),
                        147 => d.gap = t.f64(),
                        176 => d.line_color = t.i32(),
                        _ => {}
                    }
                }
                self.doc.dimstyles.insert(d.name.to_lowercase(), d);
            }
            ("BLOCK_RECORD", "BLOCK_RECORD") => {
                let mut handle = 0;
                let mut name = String::new();
                for t in rec {
                    match t.code {
                        2 => name = self.s(t),
                        5 => handle = hex(t),
                        _ => {}
                    }
                }
                self.record_by_name.insert(name.to_lowercase(), handle);
                self.record_names.insert(handle, name);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------ blocks
    fn parse_block_head(&mut self, rec: &[Tag]) -> BlockDef {
        let mut b = BlockDef::default();
        let mut base = [0.0; 3];
        for t in rec {
            match t.code {
                2 => b.name = self.s(t),
                5 => b.handle = hex(t),
                70 => b.flags = t.i32(),
                10 => base[0] = t.f64(),
                20 => base[1] = t.f64(),
                30 => base[2] = t.f64(),
                330 => b.record = hex(t),
                _ => {}
            }
        }
        b.base_point = base;
        let lname = b.name.to_lowercase();
        b.is_layout = lname.starts_with("*model_space") || lname.starts_with("*paper_space");
        if b.record == 0 {
            b.record = self.record_by_name.get(&lname).copied().unwrap_or(0);
        }
        b
    }

    // ------------------------------------------------------------------------------------------ objects
    fn parse_object(&mut self, ty: &str, rec: &[Tag]) {
        match ty {
            "LAYOUT" => {
                let mut l = LayoutDef { plot_paper_units: 1, ..Default::default() };
                let mut sub = String::new();
                for t in rec {
                    match t.code {
                        100 => sub = self.s(t),
                        1 if sub == "AcDbLayout" => l.name = self.s(t),
                        5 => l.handle = hex(t),
                        71 if sub == "AcDbLayout" => l.tab_order = t.i32(),
                        72 if sub == "AcDbPlotSettings" => l.plot_paper_units = t.i32(),
                        330 if sub == "AcDbLayout" => l.block_record = hex(t),
                        _ => {}
                    }
                }
                l.is_model = l.name.eq_ignore_ascii_case("Model");
                self.doc.layouts.push(l);
            }
            "IMAGEDEF" => {
                let mut d = ImageDef::default();
                for t in rec {
                    match t.code {
                        5 => d.handle = hex(t),
                        1 => d.filename = self.s(t),
                        10 => d.size[0] = t.f64(),
                        20 => d.size[1] = t.f64(),
                        _ => {}
                    }
                }
                self.doc.image_defs.insert(d.handle, d);
            }
            "DICTIONARY" => {
                let (mut handle, mut name, mut in_ext) = (0u64, None::<String>, false);
                let mut entries: Vec<(String, u64)> = Vec::new();
                for t in rec {
                    match t.code {
                        5 if handle == 0 => handle = hex(t),
                        102 => in_ext = self.s(t).starts_with('{'),
                        3 if !in_ext => name = Some(self.s(t)),
                        350 | 360 if !in_ext => {
                            if let Some(n) = name.take() {
                                entries.push((n, hex(t)));
                            }
                        }
                        _ => {}
                    }
                }
                if handle != 0 {
                    self.dicts.insert(handle, entries);
                }
            }
            "SPATIAL_FILTER" => {
                let (mut handle, mut in_sub, mut in_ext) = (0u64, false, false);
                let mut sp = SpatialRaw { enabled: true, ..Default::default() };
                let mut matrix: Vec<f64> = Vec::new();
                for t in rec {
                    match t.code {
                        5 if handle == 0 => handle = hex(t),
                        100 => in_sub = self.s(t) == "AcDbSpatialFilter",
                        102 => in_ext = self.s(t).starts_with("{ACAD_XDICTIONARY"),
                        360 if in_ext => sp.xdict = hex(t),
                        10 if in_sub => sp.boundary.push([t.f64(), 0.0]),
                        20 if in_sub => {
                            if let Some(v) = sp.boundary.last_mut() {
                                v[1] = t.f64();
                            }
                        }
                        71 if in_sub => sp.enabled = t.i32() != 0,
                        40 if in_sub => matrix.push(t.f64()),
                        _ => {}
                    }
                }
                // the last 24 values of group 40 are the inverse insert matrix and the transform matrix, 12 each
                if matrix.len() >= 24 {
                    let mut m = [0.0; 12];
                    m.copy_from_slice(&matrix[matrix.len() - 24..matrix.len() - 12]);
                    sp.inverse_insert = Some(m);
                }
                if handle != 0 {
                    self.spatial.insert(handle, sp);
                }
            }
            "XRECORD" => {
                let mut handle = 0u64;
                let mut inv = InvertedRaw::default();
                // 0 outside of a section, 1 ACAD_INVERTEDCLIP_ROUNDTRIP, 2 ACAD_INVERTEDCLIP_ROUNDTRIP_COMPARE
                let mut section = 0;
                for t in rec {
                    match t.code {
                        5 if handle == 0 => handle = hex(t),
                        102 => {
                            let name = self.s(t);
                            section = match name.as_str() {
                                "ACAD_INVERTEDCLIP_ROUNDTRIP" => {
                                    inv.inverted_clip.get_or_insert_with(Vec::new);
                                    1
                                }
                                "ACAD_INVERTEDCLIP_ROUNDTRIP_COMPARE" => 2,
                                _ => 0,
                            };
                        }
                        10 | 20 => {
                            let pts = match section {
                                1 => inv.inverted_clip.as_mut(),
                                2 => Some(&mut inv.compare),
                                _ => None,
                            };
                            if let Some(v) = pts {
                                if t.code == 10 {
                                    v.push([t.f64(), 0.0]);
                                } else if let Some(last) = v.last_mut() {
                                    last[1] = t.f64();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if handle != 0 && inv.inverted_clip.is_some() {
                    self.xrecords.insert(handle, inv);
                }
            }
            "SORTENTSTABLE" => {
                // pairs of (331 entity handle, 5 sort handle) after the owner (330 block record)
                let mut block_record = 0;
                let mut owners = 0;
                let mut ent = 0u64;
                let mut table: HashMap<u64, u64> = HashMap::new();
                let mut in_sub = false;
                for t in rec {
                    match t.code {
                        100 => in_sub = self.s(t) == "AcDbSortentsTable",
                        330 if in_sub => {
                            owners += 1;
                            if owners == 1 {
                                block_record = hex(t);
                            }
                        }
                        331 if in_sub => ent = hex(t),
                        5 if in_sub && ent != 0 => {
                            table.insert(ent, hex(t));
                            ent = 0;
                        }
                        _ => {}
                    }
                }
                if block_record != 0 {
                    self.doc.sort_tables.insert(block_record, table);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------------------------ entities
    pub(super) fn new_common(&self) -> Common {
        Common::new(self.doc.layer0, self.doc.bylayer)
    }

    /// Group codes shared by every graphic entity. Returns false when `t` is not one of them.
    pub(super) fn common_tag(&mut self, c: &mut Common, t: &Tag) -> bool {
        match t.code {
            5 => c.handle = hex(t),
            330 => {
                if c.owner == 0 {
                    c.owner = hex(t)
                }
            }
            8 => c.layer = self.doc.layer_names.intern(&self.s(t)),
            6 => {
                c.linetype = self.doc.linetype_names.intern(&self.s(t));
                c.has_linetype = true;
            }
            62 => c.color = t.i32(),
            370 => c.lineweight = t.i32(),
            48 => c.ltscale = t.f64(),
            60 => c.invisible = t.i32() != 0,
            67 => c.paperspace = t.i32() != 0,
            420 => c.true_color = Some(t.i64() as u32 & 0x00FF_FFFF),
            440 => c.transparency = Some(t.i64()),
            210 => c.extrusion[0] = t.f64(),
            220 => c.extrusion[1] = t.f64(),
            230 => c.extrusion[2] = t.f64(),
            _ => return false,
        }
        true
    }

    fn entity_record(&mut self, ty: &str, rec: &[Tag], sink: &mut Sink) {
        match ty {
            "SEQEND" => {
                sink.open = None;
                return;
            }
            "VERTEX" => {
                let v = self.parse_vertex(rec);
                match sink.open.and_then(|i| sink.list.get_mut(i)) {
                    Some(Entity { d: Data::Polyline(p), .. }) => p.vertices.push(v),
                    _ => self.extra_errors += 1,
                }
                return;
            }
            "ATTRIB" => {
                let e = self.parse_text(rec, TextKind::Attrib);
                match sink.open.and_then(|i| sink.list.get_mut(i)) {
                    Some(Entity { d: Data::Insert(ins), .. }) => ins.attribs.push(e),
                    _ => sink.list.push(e), // loose ATTRIB (damaged file): drawn on its own
                }
                return;
            }
            "ENDBLK" | "ENDSEC" | "ENDTAB" => return,
            _ => {}
        }
        // a new entity while a composite is still open: SEQEND was missing
        sink.open = None;
        let Some(e) = self.parse_entity(ty, rec) else { return };
        let composite = match &e.d {
            Data::Polyline(_) => true,
            Data::Insert(i) => !i.table && rec.iter().any(|t| t.code == 66 && t.i32() != 0),
            _ => false,
        };
        sink.list.push(e);
        if composite {
            sink.open = Some(sink.list.len() - 1);
        }
    }

    // ------------------------------------------------------------------------------------------ XCLIP
    fn dict_get(&self, dict: u64, key: &str) -> Option<u64> {
        self.dicts.get(&dict)?.iter().find(|(n, _)| n == key).map(|(_, h)| *h)
    }

    /// Block clipping path of an INSERT with the extension dictionary `xdict` (ezdxf `XClip`): INSERT -> its dictionary ->
    /// ACAD_FILTER -> SPATIAL (the SPATIAL_FILTER object); an inverted path is stored in an XRECORD of the filter.
    fn resolve_xclip(&self, xdict: u64) -> Option<XClip> {
        let filter = self.dict_get(xdict, "ACAD_FILTER")?;
        let sf = self.spatial.get(&self.dict_get(filter, "SPATIAL")?)?;
        let m = sf.inverse_insert.unwrap_or([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let to_block = |p: &[f64; 2]| [p[0] * m[0] + p[1] * m[1] + m[3], p[0] * m[4] + p[1] * m[5] + m[7]];
        let mut polygon: Vec<[f64; 2]> = sf.boundary.iter().map(to_block).collect();
        if polygon.len() == 2 {
            // two corner points: the rectangle around them
            let (a, b) = (polygon[0], polygon[1]);
            let (x0, y0, x1, y1) = (a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1]));
            polygon = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        }
        if polygon.len() < 3 {
            return None;
        }
        let mut clip = XClip { enabled: sf.enabled, polygon, ..Default::default() };
        let roundtrip = self.dict_get(sf.xdict, "ACAD_XREC_ROUNDTRIP").and_then(|h| self.xrecords.get(&h));
        if let Some((inner, compare)) = roundtrip.and_then(|x| x.inverted_clip.as_ref().map(|c| (c, &x.compare))) {
            clip.inverted = true;
            clip.inverted_clip = inner.iter().map(to_block).collect();
            clip.inverted_compare = compare.iter().map(to_block).collect();
        }
        Some(clip)
    }

    // ------------------------------------------------------------------------------------------ assembly
    fn finish(mut self) -> Document {
        let mut doc = std::mem::take(&mut self.doc);
        doc.errors = self.cur.r.errors + self.extra_errors;

        // layouts: Model plus the paper space layouts in tab order
        let mut layouts = std::mem::take(&mut doc.layouts);
        if !layouts.iter().any(|l| l.is_model) {
            layouts.push(LayoutDef { name: "Model".into(), is_model: true, tab_order: 0, plot_paper_units: 1, ..Default::default() });
        }
        let model_record = self
            .record_by_name
            .get("*model_space")
            .copied()
            .or_else(|| layouts.iter().find(|l| l.is_model).map(|l| l.block_record).filter(|&h| h != 0))
            .unwrap_or(0);
        if !layouts.iter().any(|l| !l.is_model) {
            // R12 and files without LAYOUT objects get the default paper space "Layout1"
            let rec = self.record_by_name.get("*paper_space").copied().unwrap_or(0);
            layouts.push(LayoutDef { name: "Layout1".into(), tab_order: 1, block_record: rec, plot_paper_units: 1, ..Default::default() });
        }
        for l in layouts.iter_mut() {
            if l.block_record == 0 {
                l.block_record = if l.is_model { model_record } else { self.record_by_name.get("*paper_space").copied().unwrap_or(0) };
            }
        }
        layouts.sort_by_key(|l| (if l.is_model { -1 } else { l.tab_order as i64 }, 0));
        // block record -> layout key
        let mut key_of_record: HashMap<u64, String> = HashMap::new();
        for l in &layouts {
            let key = if l.is_model { "model".to_string() } else { l.name.to_lowercase() };
            if l.block_record != 0 {
                key_of_record.insert(l.block_record, key);
            }
        }
        if model_record != 0 {
            key_of_record.insert(model_record, "model".into());
        }
        // name based fallback for blocks of layouts: "*Paper_Space" is the first paper layout in tab order
        let paper_keys: Vec<String> = layouts.iter().filter(|l| !l.is_model).map(|l| l.name.to_lowercase()).collect();
        let active_paper = paper_keys.first().cloned().unwrap_or_else(|| "layout1".into());
        for key in std::iter::once("model".to_string()).chain(paper_keys.iter().cloned()) {
            doc.layout_entities.entry(key).or_default();
        }

        let key_for_block = |b: &BlockDef, key_of_record: &HashMap<u64, String>| -> Option<String> {
            let n = b.name.to_lowercase();
            if n == "*model_space" {
                return Some("model".into());
            }
            if let Some(k) = key_of_record.get(&b.record) {
                return Some(k.clone());
            }
            if n == "*paper_space" {
                return Some(active_paper.clone());
            }
            None
        };

        // entities of the layout blocks first (BLOCKS section), then the ENTITIES section
        let names: Vec<String> = doc.block_order.clone();
        for n in names {
            let Some(b) = doc.blocks.get(&n) else { continue };
            if !b.is_layout {
                continue;
            }
            let Some(key) = key_for_block(b, &key_of_record) else { continue };
            let ents = doc.blocks.get_mut(&n).map(|b| std::mem::take(&mut b.entities)).unwrap_or_default();
            doc.layout_entities.entry(key).or_default().extend(ents);
        }
        for e in std::mem::take(&mut self.entities.list) {
            let key = if let Some(k) = key_of_record.get(&e.c.owner).filter(|_| e.c.owner != 0) {
                k.clone()
            } else if e.c.paperspace {
                active_paper.clone()
            } else {
                "model".into()
            };
            doc.layout_entities.entry(key).or_default().push(e);
        }

        // block clipping paths (XCLIP) of the INSERTs: the OBJECTS section comes after the entities, so it is done here
        if !self.dicts.is_empty() {
            for ents in doc.layout_entities.values_mut().chain(doc.blocks.values_mut().map(|b| &mut b.entities)) {
                for e in ents.iter_mut() {
                    if let Data::Insert(i) = &mut e.d {
                        if i.xdict != 0 && !i.table && !i.from_inventor {
                            i.clip = self.resolve_xclip(i.xdict);
                        }
                    }
                }
            }
        }
        for (key, b) in &doc.blocks {
            if b.record != 0 {
                doc.block_by_record.insert(b.record, key.clone());
            }
        }

        // Autodesk Inventor drawing views: ACIDBLOCKREFERENCE -> a plain INSERT next to it (layer "0", appended last)
        let mut inventor = 0;
        for ents in doc.layout_entities.values_mut() {
            let mut extra = Vec::new();
            for e in ents.iter() {
                if let Data::Insert(i) = &e.d {
                    if i.from_inventor && doc.blocks.contains_key(&i.name.to_lowercase()) {
                        let mut ins = (**i).clone();
                        ins.from_inventor = false;
                        extra.push(Entity { c: Common::new(doc.layer0, doc.bylayer), d: Data::Insert(Box::new(ins)) });
                        inventor += 1;
                    }
                }
            }
            ents.extend(extra);
        }
        doc.inventor = inventor;

        // draw order (SORTENTSTABLE) is applied per layout when the scene is built; keep the handle index
        for (key, ents) in &doc.layout_entities {
            for (i, e) in ents.iter().enumerate() {
                if e.c.handle != 0 {
                    doc.handle_index.insert(e.c.handle, (key.clone(), i));
                }
            }
        }
        // layout list for the consumers
        doc.layouts = layouts;
        // layer table order is the file order; layers referenced by entities but not in the table are appended by the renderer
        doc
    }
}
