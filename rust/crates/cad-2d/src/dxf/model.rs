//! In-memory DXF document: tables, blocks, layouts and typed entities.
use std::collections::HashMap;

pub use crate::geom::{V3, Z_AXIS};

/// Case-insensitive name table (layer / linetype / style names). Ids are dense and stable.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub names: Vec<String>,
    index: HashMap<String, u32>,
}

impl Names {
    pub fn key(name: &str) -> String {
        name.to_lowercase()
    }

    pub fn intern(&mut self, name: &str) -> u32 {
        let k = Self::key(name);
        if let Some(&i) = self.index.get(&k) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(name.to_string());
        self.index.insert(k, i);
        i
    }

    pub fn find(&self, name: &str) -> Option<u32> {
        self.index.get(&Self::key(name)).copied()
    }

    pub fn name(&self, id: u32) -> &str {
        self.names.get(id as usize).map(|s| s.as_str()).unwrap_or("0")
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Attributes every graphic entity has.
#[derive(Clone, Debug)]
pub struct Common {
    pub handle: u64,
    pub owner: u64,
    pub layer: u32,
    pub linetype: u32,
    /// ACI: 0 BYBLOCK, 256 BYLAYER, negative = layer off (layers only).
    pub color: i32,
    pub true_color: Option<u32>,
    /// 1/100 mm; -1 BYLAYER, -2 BYBLOCK, -3 DEFAULT.
    pub lineweight: i32,
    pub ltscale: f64,
    pub invisible: bool,
    pub paperspace: bool,
    /// Raw group 440 value.
    pub transparency: Option<i64>,
    pub extrusion: V3,
    /// The entity carries an explicit `linetype` tag (otherwise BYLAYER).
    pub has_linetype: bool,
}

impl Common {
    pub fn new(layer0: u32, bylayer: u32) -> Self {
        Common {
            handle: 0,
            owner: 0,
            layer: layer0,
            linetype: bylayer,
            color: 256,
            true_color: None,
            lineweight: -1,
            ltscale: 1.0,
            invisible: false,
            paperspace: false,
            transparency: None,
            extrusion: Z_AXIS,
            has_linetype: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub c: Common,
    pub d: Data,
}

impl Entity {
    pub fn dxftype(&self) -> &str {
        self.d.dxftype()
    }
}

#[derive(Clone, Debug)]
pub enum Data {
    Line { start: V3, end: V3 },
    /// XLINE and RAY: `start` is the base point, `dir` the unit direction.
    Xline { start: V3, dir: V3, ray: bool },
    /// `angle`: orientation of the point symbol in degrees (group 50)
    Point { loc: V3, angle: f64 },
    Circle { center: V3, radius: f64 },
    Arc { center: V3, radius: f64, start: f64, end: f64 },
    Ellipse { center: V3, major: V3, ratio: f64, start: f64, end: f64 },
    Spline(Box<SplineData>),
    LwPolyline(Box<LwPoly>),
    Polyline(Box<PolylineData>),
    Solid { v: [V3; 4], trace: bool },
    Face3d { v: [V3; 4], edge_flags: i32 },
    Text(Box<TextData>),
    MText(Box<MTextData>),
    Insert(Box<InsertData>),
    Dimension(Box<DimData>),
    Leader(Box<LeaderData>),
    MLeader(Box<MLeaderData>),
    MLine(Box<MLineData>),
    Hatch(Box<HatchData>),
    Image(Box<ImageData>),
    Wipeout(Box<ImageData>),
    Viewport(Box<ViewportData>),
    Mesh(Box<MeshData>),
    /// Entity types ezdxf cannot draw (3DSOLID, REGION, proxy entities ...): counted, not drawn.
    Other(String),
}

impl Data {
    pub fn dxftype(&self) -> &str {
        match self {
            Data::Line { .. } => "LINE",
            Data::Xline { ray: false, .. } => "XLINE",
            Data::Xline { ray: true, .. } => "RAY",
            Data::Point { .. } => "POINT",
            Data::Circle { .. } => "CIRCLE",
            Data::Arc { .. } => "ARC",
            Data::Ellipse { .. } => "ELLIPSE",
            Data::Spline(s) => {
                if s.helix {
                    "HELIX"
                } else {
                    "SPLINE"
                }
            }
            Data::LwPolyline(_) => "LWPOLYLINE",
            Data::Polyline(_) => "POLYLINE",
            Data::Solid { trace: false, .. } => "SOLID",
            Data::Solid { trace: true, .. } => "TRACE",
            Data::Face3d { .. } => "3DFACE",
            Data::Text(t) => match t.kind {
                TextKind::Text => "TEXT",
                TextKind::Attrib => "ATTRIB",
                TextKind::Attdef => "ATTDEF",
            },
            Data::MText(_) => "MTEXT",
            Data::Insert(i) => {
                if i.from_inventor {
                    "ACIDBLOCKREFERENCE"
                } else if i.table {
                    "ACAD_TABLE"
                } else {
                    "INSERT"
                }
            }
            Data::Dimension(_) => "DIMENSION",
            Data::Leader(_) => "LEADER",
            Data::MLeader(m) => {
                if m.old_name {
                    "MULTILEADER"
                } else {
                    "MLEADER"
                }
            }
            Data::MLine(_) => "MLINE",
            Data::Hatch(h) => {
                if h.mpolygon {
                    "MPOLYGON"
                } else {
                    "HATCH"
                }
            }
            Data::Image(_) => "IMAGE",
            Data::Wipeout(_) => "WIPEOUT",
            Data::Viewport(_) => "VIEWPORT",
            Data::Mesh(_) => "MESH",
            Data::Other(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SplineData {
    pub helix: bool,
    /// 1 closed, 2 periodic, 4 rational, 8 planar, 16 linear
    pub flags: i32,
    pub degree: i32,
    pub knots: Vec<f64>,
    pub weights: Vec<f64>,
    pub control_points: Vec<V3>,
    pub fit_points: Vec<V3>,
    pub start_tangent: Option<V3>,
    pub end_tangent: Option<V3>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LwVertex {
    pub x: f64,
    pub y: f64,
    pub start_width: f64,
    pub end_width: f64,
    pub bulge: f64,
}

#[derive(Clone, Debug, Default)]
pub struct LwPoly {
    pub closed: bool,
    pub const_width: f64,
    pub elevation: f64,
    pub points: Vec<LwVertex>,
}

#[derive(Clone, Debug, Default)]
pub struct Vertex {
    pub loc: V3,
    pub start_width: f64,
    pub end_width: f64,
    pub bulge: f64,
    pub flags: i32,
    /// Polyface mesh face vertex indices (groups 71 - 74), 0 when unused.
    pub idx: [i32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct PolylineData {
    pub flags: i32,
    pub elevation: V3,
    pub has_elevation: bool,
    pub default_start_width: f64,
    pub default_end_width: f64,
    pub m_count: i32,
    pub n_count: i32,
    pub vertices: Vec<Vertex>,
}

impl PolylineData {
    pub const CLOSED: i32 = 1;
    pub const IS_3D: i32 = 8;
    pub const IS_POLYMESH: i32 = 16;
    pub const MESH_CLOSED_N: i32 = 32;
    pub const IS_POLYFACE: i32 = 64;

    pub fn is_closed(&self) -> bool {
        self.flags & Self::CLOSED != 0
    }
    pub fn is_3d(&self) -> bool {
        self.flags & Self::IS_3D != 0
    }
    pub fn is_polymesh(&self) -> bool {
        self.flags & Self::IS_POLYMESH != 0
    }
    pub fn is_polyface(&self) -> bool {
        self.flags & Self::IS_POLYFACE != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextKind {
    Text,
    Attrib,
    Attdef,
}

#[derive(Clone, Debug)]
pub struct TextData {
    pub kind: TextKind,
    pub insert: V3,
    pub align_point: V3,
    pub has_align_point: bool,
    pub height: f64,
    pub text: String,
    pub tag: String,
    pub rotation: f64,
    pub width_factor: f64,
    pub oblique: f64,
    pub style: u32,
    /// 2 mirror x (backward), 4 mirror y (upside down)
    pub generation: i32,
    pub halign: i32,
    pub valign: i32,
    /// ATTRIB / ATTDEF flags: 1 invisible
    pub attr_flags: i32,
    /// ATTRIB / ATTDEF text defined as MTEXT in AutoCAD 2018+ (ignored, the plain text is drawn)
    pub embedded_mtext: bool,
}

#[derive(Clone, Debug)]
pub struct MTextData {
    pub insert: V3,
    pub char_height: f64,
    pub width: f64,
    pub attachment: i32,
    pub flow_direction: i32,
    pub text: String,
    pub style: u32,
    pub text_direction: Option<V3>,
    pub rotation: f64,
    pub line_spacing_factor: f64,
    pub bg_fill: i32,
    pub bg_color: Option<i32>,
    pub bg_true_color: Option<u32>,
    pub box_fill_scale: f64,
    pub column_type: i32,
    pub column_count: i32,
    pub column_width: f64,
    pub column_gutter: f64,
    pub column_heights: Vec<f64>,
    pub auto_height: bool,
    /// Defined annotation height (group 46): height of every column of static / auto height columns
    pub defined_height: f64,
}

#[derive(Clone, Debug)]
pub struct InsertData {
    pub name: String,
    pub insert: V3,
    pub scale: V3,
    pub rotation: f64,
    pub col_count: i32,
    pub row_count: i32,
    pub col_spacing: f64,
    pub row_spacing: f64,
    pub attribs: Vec<Entity>,
    /// ACAD_TABLE entity drawn through its anonymous block.
    pub table: bool,
    /// Autodesk Inventor ACIDBLOCKREFERENCE converted to a plain block reference.
    pub from_inventor: bool,
    /// Block clipping (XCLIP), resolved from the extension dictionary of the INSERT after the OBJECTS section was read.
    pub clip: Option<XClip>,
    /// Handle of the extension dictionary (`102 {ACAD_XDICTIONARY` / `360`), 0 when there is none.
    pub xdict: u64,
}

impl InsertData {
    /// Columns and rows of a MINSERT that have a position of their own. A spacing of zero puts all instances of that
    /// direction on one spot, and ezdxf (`Insert.multi_insert`) yields every location once. A single INSERT is 1 x 1.
    pub fn grid(&self) -> (usize, usize) {
        if self.col_count <= 1 && self.row_count <= 1 {
            return (1, 1);
        }
        let cols = if self.col_spacing != 0.0 { self.col_count.max(1) as usize } else { 1 };
        let rows = if self.row_spacing != 0.0 { self.row_count.max(1) as usize } else { 1 };
        (cols, rows)
    }

    /// Number of block instances the entity draws.
    pub fn instances(&self) -> usize {
        let (c, r) = self.grid();
        c.saturating_mul(r)
    }
}

/// Block clipping path of an INSERT (SPATIAL_FILTER of its ACAD_FILTER dictionary, ezdxf `XClip`). All polygons are in
/// block coordinates: the inverse insert matrix of the filter has been applied.
#[derive(Clone, Debug, Default)]
pub struct XClip {
    /// `is_clipping_enabled` of the filter (group 71): a disabled clip draws the block unclipped.
    pub enabled: bool,
    /// Clipping polygon (two corner points are already expanded to the rectangle around them).
    pub polygon: Vec<[f64; 2]>,
    /// `ACAD_INVERTEDCLIP_ROUNDTRIP` section of the filter was found: everything outside of the inner polygon is shown.
    pub inverted: bool,
    /// Inner polygon of an inverted clip (`ACAD_INVERTEDCLIP_ROUNDTRIP`).
    pub inverted_clip: Vec<[f64; 2]>,
    /// Combined inner and outer boundary of an inverted clip (`ACAD_INVERTEDCLIP_ROUNDTRIP_COMPARE`); its bounding box is
    /// the outer limit of the shown area.
    pub inverted_compare: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Default)]
pub struct DimData {
    pub block: String,
    pub insert: Option<V3>,
    pub dimtype: i32,
    pub text: String,
    pub text_midpoint: V3,
    pub defpoint: V3,
    pub defpoint2: V3,
    pub defpoint3: V3,
    pub defpoint4: V3,
    pub defpoint5: V3,
    pub angle: f64,
    pub text_rotation: f64,
    pub actual_measurement: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct LeaderData {
    pub arrow_head: bool,
    pub spline_path: bool,
    pub hookline: bool,
    pub vertices: Vec<V3>,
    pub text_height: f64,
    pub dimstyle: String,
    pub annotation_type: i32,
}

/// One LEADER{} block of the MLEADER context: its leader lines and the connection to the content.
#[derive(Clone, Debug, Default)]
pub struct MLeaderLine {
    pub vertices: Vec<Vec<V3>>,
    pub last_point: V3,
    pub dogleg: V3,
    pub has_dogleg_vector: bool,
    pub dogleg_length: f64,
}

#[derive(Clone, Debug, Default)]
pub struct MLeaderData {
    pub old_name: bool,
    pub content_type: i32,
    pub text: String,
    pub text_location: V3,
    pub text_height: f64,
    pub text_rotation: f64,
    pub text_width: f64,
    pub text_attachment: i32,
    pub text_direction: V3,
    pub block_name: Option<String>,
    pub block_location: V3,
    pub block_scale: V3,
    pub block_rotation: f64,
    pub block_handle: u64,
    pub leaders: Vec<MLeaderLine>,
    pub arrow_size: f64,
    pub scale: f64,
    pub has_context: bool,
    pub landing_gap: f64,
    pub dogleg: bool,
    pub dogleg_length: f64,
    pub text_style: u32,
    pub text_color: Option<i32>,
    pub arrow_head: bool,
    /// 0 invisible, 1 straight, 2 spline
    pub leader_type: i32,
}

#[derive(Clone, Debug, Default)]
pub struct MLineData {
    pub style_name: String,
    pub scale: f64,
    pub justification: i32,
    pub closed: bool,
    pub start: V3,
    pub vertices: Vec<MLineVertex>,
}

#[derive(Clone, Debug, Default)]
pub struct MLineVertex {
    pub location: V3,
    pub direction: V3,
    pub miter: V3,
    /// per element: (offset along miter, ...) parameters; first is the segment parameter list
    pub elements: Vec<Vec<f64>>,
}

#[derive(Clone, Debug, Default)]
pub struct HatchData {
    pub mpolygon: bool,
    pub elevation: f64,
    pub solid: bool,
    pub pattern_name: String,
    pub pattern_angle: f64,
    pub pattern_scale: f64,
    pub pattern_double: bool,
    pub hatch_style: i32,
    pub paths: Vec<HatchPath>,
    pub pattern: Vec<PatternLine>,
    pub gradient: bool,
    /// MPOLYGON: offset vector (group 11, OCS), added to every boundary path.
    pub offset: Option<V3>,
    /// MPOLYGON: solid fill colour as ACI (group 63).
    pub fill_color: Option<i32>,
    /// Number of colours of the gradient record (group 453); with a solid MPOLYGON fill a positive count means the
    /// fill colour is the true colour `gradient_color1`.
    pub gradient_colors: i32,
    /// First true colour (group 421) of the gradient record.
    pub gradient_color1: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum HatchPath {
    Poly { flags: i32, closed: bool, vertices: Vec<[f64; 3]> },
    Edges { flags: i32, edges: Vec<HatchEdge> },
}

impl HatchPath {
    pub fn flags(&self) -> i32 {
        match self {
            HatchPath::Poly { flags, .. } | HatchPath::Edges { flags, .. } => *flags,
        }
    }
}

#[derive(Clone, Debug)]
pub enum HatchEdge {
    Line { start: [f64; 2], end: [f64; 2] },
    Arc { center: [f64; 2], radius: f64, start: f64, end: f64, ccw: bool },
    Ellipse { center: [f64; 2], major: [f64; 2], ratio: f64, start: f64, end: f64, ccw: bool },
    Spline {
        degree: i32,
        rational: bool,
        periodic: bool,
        knots: Vec<f64>,
        control_points: Vec<[f64; 2]>,
        weights: Vec<f64>,
        fit_points: Vec<[f64; 2]>,
        start_tangent: Option<[f64; 2]>,
        end_tangent: Option<[f64; 2]>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct PatternLine {
    pub angle: f64,
    pub base: [f64; 2],
    pub offset: [f64; 2],
    pub dashes: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ImageData {
    pub insert: V3,
    pub u: V3,
    pub v: V3,
    pub size: [f64; 2],
    pub def_handle: u64,
    pub flags: i32,
    pub clipping: bool,
    pub clip_type: i32,
    pub clip_vertices: Vec<[f64; 2]>,
    pub clip_mode: i32,
    pub fade: i32,
    /// DXF 281 (0-100, default 50).
    pub brightness: i32,
    /// DXF 282 (0-100, default 50).
    pub contrast: i32,
}

#[derive(Clone, Debug, Default)]
pub struct ViewportData {
    pub center: V3,
    pub width: f64,
    pub height: f64,
    pub status: i32,
    pub id: i32,
    pub view_center: [f64; 2],
    pub view_direction: V3,
    pub view_target: V3,
    pub view_height: f64,
    pub twist_angle: f64,
    pub flags: i32,
    pub clip_handle: u64,
    pub frozen_layers: Vec<u64>,
    pub frozen_layer_names: Vec<String>,
    pub render_mode: i32,
}

impl ViewportData {
    /// Viewport looks straight down the WCS z-axis (the only kind the drawing add-on renders).
    pub fn is_top_view(&self) -> bool {
        let d = self.view_direction;
        d[0].abs() < 1e-9 && d[1].abs() < 1e-9 && d[2] > 0.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<V3>,
    pub faces: Vec<Vec<u32>>,
}

// ----------------------------------------------------------------------------------------------- tables
#[derive(Clone, Debug)]
pub struct LayerDef {
    pub name: String,
    pub id: u32,
    pub handle: u64,
    pub flags: i32,
    pub color: i32,
    pub true_color: Option<u32>,
    pub linetype: u32,
    pub lineweight: i32,
    pub transparency: f64,
    pub plot: bool,
}

impl LayerDef {
    pub fn is_on(&self) -> bool {
        self.color >= 0
    }
    pub fn is_frozen(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn is_locked(&self) -> bool {
        self.flags & 4 != 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct LinetypeDef {
    pub name: String,
    pub description: String,
    pub pattern_length: f64,
    pub elements: Vec<f64>,
    /// Complex linetype (text / shape elements): drawn continuous
    pub complex: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StyleDef {
    pub name: String,
    pub font: String,
    pub bigfont: String,
    pub fixed_height: f64,
    pub width_factor: f64,
    pub oblique: f64,
    pub generation: i32,
    /// TrueType family stored in XDATA (ACAD application) when `font` is empty
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// The dimension style values the viewer needs (arrow size, scale, colour of dimension lines).
#[derive(Clone, Debug)]
pub struct DimStyleDef {
    pub name: String,
    pub scale: f64,
    pub arrow_size: f64,
    pub gap: f64,
    pub text_above: i32,
    /// DIMCLRD: 0 BYBLOCK, 256 BYLAYER, else an ACI
    pub line_color: i32,
}

impl Default for DimStyleDef {
    fn default() -> Self {
        DimStyleDef { name: String::new(), scale: 1.0, arrow_size: 0.18, gap: 0.09, text_above: 0, line_color: 0 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct BlockDef {
    pub name: String,
    pub base_point: V3,
    pub flags: i32,
    pub handle: u64,
    /// Handle of the block record (owner of the entities).
    pub record: u64,
    pub entities: Vec<Entity>,
    /// Anonymous or layout block (*D1, *U2, *Model_Space ...)
    pub is_layout: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutDef {
    pub name: String,
    pub handle: u64,
    pub tab_order: i32,
    pub block_record: u64,
    pub plot_paper_units: i32,
    pub is_model: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ImageDef {
    pub handle: u64,
    pub filename: String,
    pub size: [f64; 2],
}

/// How the file was read.
#[derive(Clone, Debug, Default)]
pub struct Header {
    pub version: String,
    pub values: HashMap<String, Vec<f64>>,
    pub strings: HashMap<String, String>,
}

impl Header {
    pub fn num(&self, key: &str) -> Option<f64> {
        self.values.get(key).and_then(|v| v.first()).copied()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Document {
    pub header: Header,
    pub layer_names: Names,
    pub linetype_names: Names,
    pub style_names: Names,
    pub layers: Vec<LayerDef>,
    pub linetypes: Vec<LinetypeDef>,
    pub styles: Vec<StyleDef>,
    pub dimstyles: HashMap<String, DimStyleDef>,
    /// Blocks by lower case name.
    pub blocks: HashMap<String, BlockDef>,
    /// Block names in file order.
    pub block_order: Vec<String>,
    pub layouts: Vec<LayoutDef>,
    /// Entities of every layout: model space under key "model", paper space layouts under their lower case name.
    pub layout_entities: HashMap<String, Vec<Entity>>,
    pub image_defs: HashMap<u64, ImageDef>,
    /// SORTENTSTABLE draw order: block record handle -> (entity handle -> sort handle)
    pub sort_tables: HashMap<u64, HashMap<u64, u64>>,
    /// Handle -> (layer id) of LAYER table records, for frozen layer handles of viewports
    pub layer_by_handle: HashMap<u64, u32>,
    /// Entity handle -> (space key, index) for entities referenced by handle (viewport clip boundaries ...)
    pub handle_index: HashMap<u64, (String, usize)>,
    /// Block record handle -> lower case block name (MLEADER blocks are referenced by the handle of the record).
    pub block_by_record: HashMap<u64, String>,
    /// Layer id -> position in `layers`.
    pub layer_pos: HashMap<u32, usize>,
    /// Number of structure errors that were repaired while reading.
    pub errors: u32,
    pub bylayer: u32,
    pub byblock: u32,
    pub continuous: u32,
    pub layer0: u32,
    pub standard_style: u32,
    /// Number of ACIDBLOCKREFERENCE entities converted to inserts.
    pub inventor: u32,
    pub dxf_version_code: String,
}

impl Document {
    pub fn new() -> Self {
        let mut d = Document::default();
        d.layer0 = d.layer_names.intern("0");
        d.bylayer = d.linetype_names.intern("BYLAYER");
        d.byblock = d.linetype_names.intern("BYBLOCK");
        d.continuous = d.linetype_names.intern("Continuous");
        d.standard_style = d.style_names.intern("Standard");
        d
    }

    pub fn layer(&self, id: u32) -> Option<&LayerDef> {
        match self.layer_pos.get(&id).and_then(|&i| self.layers.get(i)) {
            Some(l) if l.id == id => Some(l),
            _ => self.layers.iter().find(|l| l.id == id),
        }
    }

    /// Adds a LAYER record; a second record of the same name replaces the first. Constant time.
    pub fn add_layer(&mut self, def: LayerDef) {
        match self.layer_pos.get(&def.id) {
            Some(&i) if i < self.layers.len() => self.layers[i] = def,
            _ => {
                self.layer_pos.insert(def.id, self.layers.len());
                self.layers.push(def);
            }
        }
    }

    pub fn block(&self, name: &str) -> Option<&BlockDef> {
        self.blocks.get(&name.to_lowercase())
    }

    pub fn style(&self, id: u32) -> Option<&StyleDef> {
        self.styles.iter().find(|s| s.name.eq_ignore_ascii_case(self.style_names.name(id)))
    }
}
