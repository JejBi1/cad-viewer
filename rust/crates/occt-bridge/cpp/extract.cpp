// Mesh extraction (faces, edges, vertices in TopExp::MapShapes order), mass properties and bounds.
#include "bridge.h"
#include "common.h"
#include "occt-bridge/src/ffi.rs.h"

#include <algorithm>
#include <cmath>
#include <unordered_map>

#include <BRepAdaptor_Curve.hxx>
#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <BRepLib_ToolTriangulatedShape.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_TangentialDeflection.hxx>
#include <GProp_GProps.hxx>
#include <NCollection_IndexedDataMap.hxx>
#include <NCollection_List.hxx>
#include <NCollection_Sequence.hxx>
#include <Poly_Polygon3D.hxx>
#include <Poly_PolygonOnTriangulation.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Vertex.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_Vec.hxx>

namespace stepview {

namespace {

void push3(rust::Vec<float>& v, double x, double y, double z) {
  v.push_back(static_cast<float>(x));
  v.push_back(static_cast<float>(y));
  v.push_back(static_cast<float>(z));
}

// Deflection for edge discretisation when a part carries no polygons (same formula as the meshing deflection).
double fallback_deflection(const TopoDS_Shape& shape) {
  Bnd_Box box;
  BRepBndLib::Add(shape, box, false);
  if (box.IsVoid()) return 0.01;
  const gp_Pnt a = box.CornerMin();
  const gp_Pnt b = box.CornerMax();
  const double size = std::max({b.X() - a.X(), b.Y() - a.Y(), b.Z() - a.Z()});
  return std::min(std::max(size * 0.001, 0.002), 0.5);
}

}  // namespace

void extract_mesh(const TopoDS_Shape& shape, const ShapeMap& faces, const ShapeMap& edges, const ShapeMap& verts,
                  bool mesh_only, const FaceColors* colors, double edge_defl, MeshRec& out) {
  out.mesh_only = mesh_only;

  // ---- faces
  const int nf = faces.Extent();
  for (int fi = 1; fi <= nf; ++fi) {
    const TopoDS_Face face = TopoDS::Face(faces.FindKey(fi));
    const uint32_t first = static_cast<uint32_t>(out.indices.size());
    uint32_t count = 0;

    TopLoc_Location loc;
    const occ::handle<Poly_Triangulation>& tri = BRep_Tool::Triangulation(face, loc);
    if (!tri.IsNull() && tri->NbTriangles() > 0 && tri->NbNodes() > 0) {
      if (!tri->HasNormals()) {
        try {
          if (BRep_Tool::IsGeometric(face)) {
            BRepLib_ToolTriangulatedShape::ComputeNormals(face, tri);
          } else {
            tri->ComputeNormals();
          }
        } catch (...) {
        }
      }
      const bool have_n = tri->HasNormals();
      const bool rev = face.Orientation() == TopAbs_REVERSED;
      const gp_Trsf tr = loc.Transformation();
      const uint32_t base = static_cast<uint32_t>(out.positions.size() / 3);
      const int nn = tri->NbNodes();
      std::vector<gp_Pnt> pts(static_cast<size_t>(nn));
      for (int i = 1; i <= nn; ++i) {
        pts[static_cast<size_t>(i - 1)] = tri->Node(i).Transformed(tr);
        const gp_Pnt& p = pts[static_cast<size_t>(i - 1)];
        push3(out.positions, p.X(), p.Y(), p.Z());
      }
      // triangles (winding follows the face orientation)
      std::vector<gp_Vec> acc;
      if (!have_n) acc.assign(static_cast<size_t>(nn), gp_Vec(0, 0, 0));
      const int nt = tri->NbTriangles();
      for (int t = 1; t <= nt; ++t) {
        int a = 0, b = 0, c = 0;
        tri->Triangle(t).Get(a, b, c);
        if (a < 1 || b < 1 || c < 1 || a > nn || b > nn || c > nn) continue;
        if (rev) std::swap(b, c);
        out.indices.push_back(base + static_cast<uint32_t>(a - 1));
        out.indices.push_back(base + static_cast<uint32_t>(b - 1));
        out.indices.push_back(base + static_cast<uint32_t>(c - 1));
        count += 3;
        if (!have_n) {
          const gp_Vec n = gp_Vec(pts[static_cast<size_t>(a - 1)], pts[static_cast<size_t>(b - 1)])
                               .Crossed(gp_Vec(pts[static_cast<size_t>(a - 1)], pts[static_cast<size_t>(c - 1)]));
          acc[static_cast<size_t>(a - 1)] += n;
          acc[static_cast<size_t>(b - 1)] += n;
          acc[static_cast<size_t>(c - 1)] += n;
        }
      }
      // normals
      for (int i = 1; i <= nn; ++i) {
        gp_Vec n(0, 0, 1);
        if (have_n) {
          NCollection_Vec3<float> v;
          tri->Normal(i, v);
          n = gp_Vec(v.x(), v.y(), v.z());
          n.Transform(tr);
          if (rev) n.Reverse();
        } else {
          n = acc[static_cast<size_t>(i - 1)];
        }
        const double m = n.Magnitude();
        if (m > 1e-12) {
          n.Divide(m);
        } else {
          n = gp_Vec(0, 0, 1);
        }
        push3(out.normals, n.X(), n.Y(), n.Z());
      }
    }
    out.face_first.push_back(first);
    out.face_count.push_back(count);
    if (colors != nullptr && static_cast<size_t>(fi - 1) < colors->has.size() && colors->has[static_cast<size_t>(fi - 1)]) {
      out.face_has_color.push_back(1);
      for (int k = 0; k < 3; ++k) out.face_color.push_back(colors->rgb[static_cast<size_t>(fi - 1) * 3 + static_cast<size_t>(k)]);
    } else {
      out.face_has_color.push_back(0);
      for (int k = 0; k < 3; ++k) out.face_color.push_back(0.0f);
    }
  }

  // ---- edges: Poly_Polygon3D, else polygon on a face triangulation, else a discretisation of the curve
  NCollection_IndexedDataMap<TopoDS_Shape, NCollection_List<TopoDS_Shape>, TopTools_ShapeMapHasher> ancestors;
  bool ancestors_ready = false;
  const int ne = edges.Extent();
  for (int ei = 1; ei <= ne; ++ei) {
    const TopoDS_Edge edge = TopoDS::Edge(edges.FindKey(ei));
    std::vector<gp_Pnt> pts;
    try {
      if (!BRep_Tool::Degenerated(edge)) {
        TopLoc_Location l;
        const occ::handle<Poly_Polygon3D>& poly = BRep_Tool::Polygon3D(edge, l);
        if (!poly.IsNull() && poly->NbNodes() >= 2) {
          const gp_Trsf tr = l.Transformation();
          const NCollection_Array1<gp_Pnt>& nodes = poly->Nodes();
          for (int i = nodes.Lower(); i <= nodes.Upper(); ++i) pts.push_back(nodes.Value(i).Transformed(tr));
        } else {
          if (!ancestors_ready) {
            TopExp::MapShapesAndAncestors(shape, TopAbs_EDGE, TopAbs_FACE, ancestors);
            ancestors_ready = true;
          }
          const int ai = ancestors.FindIndex(edge);
          if (ai > 0) {
            for (const TopoDS_Shape& f : ancestors.FindFromIndex(ai)) {
              TopLoc_Location fl;
              const occ::handle<Poly_Triangulation>& t = BRep_Tool::Triangulation(TopoDS::Face(f), fl);
              if (t.IsNull()) continue;
              const occ::handle<Poly_PolygonOnTriangulation>& pot = BRep_Tool::PolygonOnTriangulation(edge, t, fl);
              if (pot.IsNull() || pot->NbNodes() < 2) continue;
              const gp_Trsf tr = fl.Transformation();
              const NCollection_Array1<int>& idx = pot->Nodes();
              for (int i = idx.Lower(); i <= idx.Upper(); ++i) {
                const int k = idx.Value(i);
                if (k >= 1 && k <= t->NbNodes()) pts.push_back(t->Node(k).Transformed(tr));
              }
              break;
            }
          }
        }
        if (pts.size() < 2) {
          pts.clear();
          BRepAdaptor_Curve c(edge);
          GCPnts_TangentialDeflection td(c, 0.35, edge_defl);
          for (int i = 1; i <= td.NbPoints(); ++i) pts.push_back(td.Value(i));
        }
      }
    } catch (...) {
      pts.clear();
    }
    out.edge_first.push_back(static_cast<uint32_t>(out.edge_points.size() / 3));
    out.edge_count.push_back(static_cast<uint32_t>(pts.size()));
    for (const gp_Pnt& p : pts) push3(out.edge_points, p.X(), p.Y(), p.Z());
  }

  // ---- vertices
  const int nv = verts.Extent();
  for (int vi = 1; vi <= nv; ++vi) {
    const gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(verts.FindKey(vi)));
    push3(out.vertices, p.X(), p.Y(), p.Z());
  }
}

namespace {

// Face colours that XCAF stores on sub-shape labels (coarser shapes first, so a face colour wins).
void collect_face_colors(Proto& p, FaceColors& fc) {
  const int nf = p.faces.Extent();
  fc.has.assign(static_cast<size_t>(nf), 0);
  fc.rgb.assign(static_cast<size_t>(nf) * 3, 0.0f);
  if (nf == 0) return;
  NCollection_Sequence<TDF_Label> subs;
  if (!XCAFDoc_ShapeTool::GetSubShapes(p.label, subs)) return;
  struct Item {
    TopoDS_Shape shape;
    Quantity_Color color;
  };
  std::vector<Item> items;
  for (int i = 1; i <= subs.Length(); ++i) {
    Quantity_Color c;
    if (!label_color(subs.Value(i), c)) continue;
    TopoDS_Shape s = XCAFDoc_ShapeTool::GetShape(subs.Value(i));
    if (s.IsNull()) continue;
    items.push_back({s, c});
  }
  if (items.empty()) return;
  std::stable_sort(items.begin(), items.end(),
                   [](const Item& a, const Item& b) { return static_cast<int>(a.shape.ShapeType()) < static_cast<int>(b.shape.ShapeType()); });

  std::unordered_multimap<const void*, int> by_tshape;  // fallback: same TShape, different location
  bool by_tshape_ready = false;
  auto paint = [&](const TopoDS_Shape& face, const Quantity_Color& c) {
    int idx = p.faces.FindIndex(face);
    if (idx > 0) {
      const size_t k = static_cast<size_t>(idx - 1);
      fc.has[k] = 1;
      fc.rgb[k * 3 + 0] = static_cast<float>(c.Red());
      fc.rgb[k * 3 + 1] = static_cast<float>(c.Green());
      fc.rgb[k * 3 + 2] = static_cast<float>(c.Blue());
      return;
    }
    if (!by_tshape_ready) {
      for (int i = 1; i <= nf; ++i) by_tshape.emplace(p.faces.FindKey(i).TShape().get(), i);
      by_tshape_ready = true;
    }
    auto range = by_tshape.equal_range(face.TShape().get());
    for (auto it = range.first; it != range.second; ++it) {
      const size_t k = static_cast<size_t>(it->second - 1);
      fc.has[k] = 1;
      fc.rgb[k * 3 + 0] = static_cast<float>(c.Red());
      fc.rgb[k * 3 + 1] = static_cast<float>(c.Green());
      fc.rgb[k * 3 + 2] = static_cast<float>(c.Blue());
    }
  };
  for (const Item& it : items) {
    const TopAbs_ShapeEnum t = it.shape.ShapeType();
    if (t == TopAbs_FACE) {
      paint(it.shape, it.color);
    } else if (t < TopAbs_FACE) {
      for (TopExp_Explorer e(it.shape, TopAbs_FACE); e.More(); e.Next()) paint(e.Current(), it.color);
    }
  }
}

}  // namespace

MeshRec doc_proto_mesh(const Doc& d, uint32_t id) {
  BR_BEGIN(MeshRec)
  Proto& p = d.proto(id);
  MeshRec out{};
  const TopoDS_Shape& shape = d.proto_shape(p);
  if (shape.IsNull()) return out;
  d.ensure_maps(p);
  FaceColors fc;
  if (!p.mesh_only) collect_face_colors(p, fc);
  extract_mesh(shape, p.faces, p.edges, p.verts, p.mesh_only, p.mesh_only ? nullptr : &fc, fallback_deflection(shape), out);
  return out;
  BR_END
}

MassRec doc_mass(const Doc& d, uint32_t id) {
  BR_BEGIN(MassRec)
  Proto& p = d.proto(id);
  MassRec out{};
  const TopoDS_Shape& shape = d.proto_shape(p);
  if (shape.IsNull()) return out;
  // analysis.py: proto_props. Exact volume / centroid; the area comes from the display mesh.
  GProp_GProps g;
  BRepGProp::VolumeProperties(shape, g, false, false, p.mesh_only);
  const double vol = g.Mass();
  gp_Pnt cog;
  if (std::abs(vol) > 1e-12) cog = g.CentreOfMass();
  GProp_GProps g2;
  BRepGProp::SurfaceProperties(shape, g2, false, true);
  uint32_t ns = 0, nfaces = 0;
  for (TopExp_Explorer e(shape, TopAbs_SOLID); e.More(); e.Next()) ++ns;
  for (TopExp_Explorer e(shape, TopAbs_FACE); e.More(); e.Next()) ++nfaces;
  out.volume = std::abs(vol);
  out.area = g2.Mass();
  out.centroid = {cog.X(), cog.Y(), cog.Z()};
  out.solids = ns;
  out.faces = nfaces;
  return out;
  BR_END
}

BoundsRec doc_node_bounds(const Doc& d, uint32_t node) {
  BR_BEGIN(BoundsRec)
  BoundsRec out{};
  if (!d.tree_built || node >= d.nodes.size()) return out;
  std::vector<uint32_t> leaves;
  d.leaves_of(node, leaves);
  Bnd_Box box;
  for (uint32_t i : leaves) {
    const TNode& n = d.nodes[i];
    if (n.proto < 0) continue;
    TopoDS_Shape s = d.located_shape(n);
    if (s.IsNull()) continue;
    BRepBndLib::AddOptimal(s, box, true, false);
  }
  if (box.IsVoid()) return out;
  const gp_Pnt a = box.CornerMin();
  const gp_Pnt b = box.CornerMax();
  out.valid = true;
  out.min = {a.X(), a.Y(), a.Z()};
  out.max = {b.X(), b.Y(), b.Z()};
  return out;
  BR_END
}

}  // namespace stepview
