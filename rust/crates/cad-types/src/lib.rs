//! Data shared by every crate of the viewer. This is the contract between crates: change it only together with
//! docs/rust-port/ARCHITECTURE.md. No logic and no dependencies on OCCT, wgpu or egui here.
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type P3 = [f64; 3];
/// Column-major 4x4 transform (model units: mm).
pub type Mat4 = [f64; 16];
pub const IDENTITY: Mat4 = [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];

/// Linear RGB, 0..1 (OCCT `Quantity_Color` components as read from the document).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgb(pub f32, pub f32, pub f32);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: P3,
    pub max: P3,
}

// ------------------------------------------------------------------ 3D model

/// Index of a prototype (a distinct part shape) in `Model::protos`.
pub type ProtoId = u32;
/// Index of a node in `ModelTree::nodes`.
pub type NodeId = u32;

/// Triangulated part in its own coordinates. Faces, edges and vertices keep the OCCT topology order, so that
/// `EntityRef` indices can be handed back to the bridge for exact measurement.
#[derive(Clone, Debug, Default)]
pub struct ProtoMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub faces: Vec<FaceSpan>,
    /// Polyline points of all B-rep edges; `edges` slices this.
    pub edge_points: Vec<[f32; 3]>,
    pub edges: Vec<EdgeSpan>,
    pub vertices: Vec<[f32; 3]>,
    pub bounds: Option<Aabb>,
    /// STL-like shape: triangulation only, no analytic surfaces.
    pub mesh_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceSpan {
    pub first_index: u32,
    pub index_count: u32,
    pub color: Option<Rgb>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSpan {
    pub first_point: u32,
    pub point_count: u32,
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Display name (instance name unless automatic, then prototype name, then "(bez názvu)").
    pub name: String,
    pub proto_name: String,
    /// OCCT label entry of the prototype; stable key for per-part settings such as materials.
    pub key: String,
    pub is_assembly: bool,
    /// Set for leaves only.
    pub proto: Option<ProtoId>,
    /// World transform of this node.
    pub location: Mat4,
    pub color: Option<Rgb>,
    /// The instance carries its own colour.
    pub own_style: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ModelTree {
    pub roots: Vec<NodeId>,
    pub nodes: Vec<TreeNode>,
    /// Instances per prototype, indexed by `ProtoId`.
    pub proto_instances: Vec<u32>,
}

/// A topological entity of a prototype; indices follow `ProtoMesh`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityKind {
    Vertex,
    Edge,
    Face,
}

/// A picked entity of one placed instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityRef {
    pub node: NodeId,
    pub kind: EntityKind,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MassProps {
    pub volume_mm3: f64,
    pub area_mm2: f64,
    pub centroid: P3,
    pub solids: u32,
    pub faces: u32,
}

/// One interfering pair found by the collision check.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interference {
    pub a: NodeId,
    pub b: NodeId,
    pub volume_mm3: f64,
    pub bounds: Option<Aabb>,
}

// ------------------------------------------------------------------ 2D drawing

pub type LayerId = u32;

/// Primitives are grouped into batches in z-order; a batch never reorders entities across layers that overlap.
/// Coordinates are relative to `Scene2d::origin`.
#[derive(Clone, Debug, Default)]
pub struct DrawBatch {
    pub layer: LayerId,
    pub color: [u8; 4],
    /// Width of a cosmetic (screen space) pen in pixels, the same at every zoom level, as the Qt viewer drew it
    /// (`lineweight / 0.3527` px). 0 means a hairline (1 pixel). A line pair with two equal points is a dot of this width.
    /// Renderers may interpret it in drawing units instead (`cad-render`: `LineWidthMode::DrawingUnits`).
    pub width: f32,
    /// Line segments as point pairs.
    pub lines: Vec<[f32; 2]>,
    /// Filled triangles as point triples.
    pub fills: Vec<[f32; 2]>,
}

/// A raster picture of a DXF IMAGE entity, already decoded, with clipping and transparency applied to its alpha.
#[derive(Clone, Debug)]
pub struct DrawImage {
    pub layer: LayerId,
    /// Drawn after the first `after` batches of `Scene2d::batches` and before the others (z-order).
    pub after: u32,
    pub width: u32,
    pub height: u32,
    /// Straight (not premultiplied) RGBA, 8 bits per channel, `width * height * 4` bytes, first row at the top.
    pub rgba: Arc<[u8]>,
    /// Corners relative to `Scene2d::origin`: top-left, top-right, bottom-right, bottom-left. They are the texel
    /// corners (0, 0), (width, 0), (width, height) and (0, height) of the picture.
    pub corners: [[f32; 2]; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub entities: u32,
    pub visible: bool,
    /// Switched off or frozen in the file itself.
    pub off_in_file: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Scene2d {
    pub layers: Vec<Layer>,
    pub batches: Vec<DrawBatch>,
    pub bounds: Option<([f64; 2], [f64; 2])>,
    pub layout: String,
    pub layouts: Vec<String>,
    /// Raster pictures in z-order with the batches (`DrawImage::after`).
    pub images: Vec<DrawImage>,
    /// World point subtracted from the coordinates of `batches` and `images`, so that f32 keeps centimetre precision
    /// far from the world origin. Zero for scenes built in world coordinates. `bounds` stays in world coordinates.
    pub origin: [f64; 2],
}
