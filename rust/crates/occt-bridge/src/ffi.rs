//! Raw cxx declarations. Private to the crate: `lib.rs` wraps them into the public API.
#![allow(clippy::too_many_arguments)]

#[allow(clippy::module_inception)]
#[cxx::bridge(namespace = "stepview")]
pub(crate) mod ffi {
    /// Result of the instance count. `status`: 0 ok, 1 cyclic, 2 too deep, 3 over the limit, 4 amplification.
    struct CountRec {
        status: u8,
        total: u64,
        stored: u64,
    }

    struct NodeRec {
        /// -1 for a root.
        parent: i32,
        name: String,
        proto_name: String,
        key: String,
        is_assembly: bool,
        /// -1 for an assembly.
        proto: i32,
        /// Column-major 4x4.
        location: [f64; 16],
        has_color: bool,
        color: [f32; 3],
        own_style: bool,
    }

    struct TreeRec {
        nodes: Vec<NodeRec>,
        proto_instances: Vec<u32>,
    }

    struct MeshRec {
        positions: Vec<f32>,
        normals: Vec<f32>,
        indices: Vec<u32>,
        face_first: Vec<u32>,
        face_count: Vec<u32>,
        /// 3 floats per face (linear RGB), meaningful where `face_has_color` is 1.
        face_color: Vec<f32>,
        face_has_color: Vec<u8>,
        edge_points: Vec<f32>,
        edge_first: Vec<u32>,
        edge_count: Vec<u32>,
        vertices: Vec<f32>,
        mesh_only: bool,
    }

    struct MassRec {
        volume: f64,
        area: f64,
        centroid: [f64; 3],
        solids: u32,
        faces: u32,
    }

    struct BoundsRec {
        valid: bool,
        min: [f64; 3],
        max: [f64; 3],
    }

    /// Raw data of one picked B-rep entity in world coordinates (see `measure::Entity`).
    /// `kind`: 0 vertex, 1 edge, 2 face. `geom`: 0 other, 1 line, 2 circle, 3 ellipse, 4 B-spline curve,
    /// 10 plane, 11 cylinder, 12 cone, 13 sphere, 14 torus, 15 B-spline surface.
    struct EntityRec {
        kind: u8,
        geom: u8,
        point: [f64; 3],
        has_axis: bool,
        axis: [f64; 3],
        has_center: bool,
        center: [f64; 3],
        has_radius: bool,
        radius: f64,
        has_length: bool,
        length: f64,
        has_ends: bool,
        p0: [f64; 3],
        p1: [f64; 3],
        is_line: bool,
        has_arc: bool,
        arc_deg: f64,
        has_axes: bool,
        axes: [f64; 2],
        has_half_angle: bool,
        half_angle: f64,
        has_radii: bool,
        radii: [f64; 2],
        has_area: bool,
        area: f64,
        centroid: [f64; 3],
        has_plane_point: bool,
        plane_point: [f64; 3],
    }

    /// A shape for BRepExtrema: `kind` 0 B-rep entity (node/ekind/index), 1 point (`pts` = x y z),
    /// 2 polyline wire (`pts` = x y z ...; closed when `closed`).
    struct ShapeSpec {
        kind: u8,
        node: u32,
        ekind: u8,
        index: u32,
        pts: Vec<f64>,
        closed: bool,
    }

    struct DistRec {
        done: bool,
        nb_solutions: i32,
        value: f64,
        p1: [f64; 3],
        p2: [f64; 3],
    }

    struct NormalRec {
        valid: bool,
        n: [f64; 3],
    }

    struct InterfRec {
        a: u32,
        b: u32,
        collision: bool,
        volume: f64,
        has_bounds: bool,
        min: [f64; 3],
        max: [f64; 3],
    }

    unsafe extern "C++" {
        include!("bridge.h");

        type Doc;
        type CadReader;
        type InterfRun;

        /// Test support: the next `fault_point(id)` of the C++ side on this thread raises an access violation once (0: off).
        fn test_inject_fault(id: u32);
        fn new_doc() -> Result<UniquePtr<Doc>>;
        /// Null when the file cannot be read or has no triangles.
        fn read_stl(path: &str, name: &str) -> Result<UniquePtr<Doc>>;
        /// Always returns a document; `status` receives the reader status name.
        fn load_xbf(path: &str, status: &mut String) -> Result<UniquePtr<Doc>>;
        /// Returns the store status name.
        fn doc_save_xbf(d: &Doc, path: &str) -> Result<String>;

        fn new_reader(iges: bool) -> Result<UniquePtr<CadReader>>;
        fn reader_read_file(r: &CadReader, path: &str) -> Result<bool>;
        fn reader_relax_shapefix(r: &CadReader) -> Result<()>;
        fn reader_transfer(r: &CadReader, d: &Doc) -> Result<bool>;

        fn doc_count_instances(
            d: &Doc,
            limit: u64,
            amplify_min: u64,
            amplify_ratio: u64,
            max_depth: i32,
        ) -> Result<CountRec>;
        /// Size (largest bounding-box extent) of every shape that will be meshed, in meshing order.
        fn doc_mesh_items(d: &Doc) -> Result<Vec<f64>>;
        fn doc_mesh_run(
            d: &Doc,
            keys: &[i32],
            no_delabella: bool,
            delabella_min_faces: u32,
        ) -> Result<()>;
        fn doc_tree(d: &Doc) -> Result<TreeRec>;
        fn doc_proto_count(d: &Doc) -> Result<u32>;

        fn doc_proto_mesh(d: &Doc, id: u32) -> Result<MeshRec>;
        fn doc_mass(d: &Doc, id: u32) -> Result<MassRec>;
        fn doc_node_bounds(d: &Doc, node: u32) -> Result<BoundsRec>;

        fn doc_entity(d: &Doc, node: u32, kind: u8, index: u32) -> Result<EntityRec>;
        fn doc_distance(d: &Doc, a: &ShapeSpec, b: &ShapeSpec) -> Result<DistRec>;
        fn doc_face_normal_at(
            d: &Doc,
            node: u32,
            face: u32,
            x: f64,
            y: f64,
            z: f64,
        ) -> Result<NormalRec>;

        fn interf_begin(d: &Doc, nodes: &[u32], gap: f64) -> Result<UniquePtr<InterfRun>>;
        fn interf_pair_count(r: &InterfRun) -> u64;
        fn interf_part_pair_count(r: &InterfRun) -> u64;
        fn interf_collisions(r: &InterfRun) -> u64;
        fn interf_step(r: &InterfRun, k: u64, touch: bool) -> Result<()>;
        fn interf_results(r: &InterfRun) -> Result<Vec<InterfRec>>;
        fn doc_interference_mesh(d: &Doc, a: u32, b: u32) -> Result<MeshRec>;
    }
}

// OCCT objects are reference counted with atomic counters and the bridge never shares one document between
// threads without `&mut`-like exclusivity (`Document` is `Send` but not `Sync`).
unsafe impl Send for ffi::Doc {}
unsafe impl Send for ffi::CadReader {}
unsafe impl Send for ffi::InterfRun {}
