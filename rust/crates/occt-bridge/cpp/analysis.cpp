// Picked-entity data, BRepExtrema distance, face normals and the interference check.
// Behaviour reference: stepview/analysis.py (the formatting and relation logic is ported on the Rust side).
#include "bridge.h"
#include "common.h"
#include "occt-bridge/src/ffi.rs.h"

#include <algorithm>
#include <cmath>

#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepAlgoAPI_Common.hxx>
#include <BRepBndLib.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GCPnts_AbscissaPoint.hxx>
#include <GProp_GProps.hxx>
#include <GeomAPI_ProjectPointOnSurf.hxx>
#include <GeomAbs_CurveType.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <GeomLProp_SLProps.hxx>
#include <Geom_Surface.hxx>
#include <NCollection_List.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Vertex.hxx>
#include <gp_Ax1.hxx>
#include <gp_Circ.hxx>
#include <gp_Cone.hxx>
#include <gp_Cylinder.hxx>
#include <gp_Dir.hxx>
#include <gp_Elips.hxx>
#include <gp_Lin.hxx>
#include <gp_Pln.hxx>
#include <gp_Pnt.hxx>
#include <gp_Sphere.hxx>
#include <gp_Torus.hxx>

namespace stepview {

namespace {

const double kRadToDeg = 180.0 / 3.14159265358979323846;  // math.degrees(x) == x * (180 / pi)

std::array<double, 3> arr(const gp_Pnt& p) { return {p.X(), p.Y(), p.Z()}; }
std::array<double, 3> arr(const gp_Dir& v) { return {v.X(), v.Y(), v.Z()}; }

TopoDS_Shape entity_shape(const Doc& d, uint32_t node, uint8_t kind, uint32_t index) {
  if (!d.tree_built || node >= d.nodes.size()) throw Fail("Neplatný uzel stromu.");
  const TNode& n = d.nodes[node];
  if (n.proto < 0) throw Fail("Vybraný uzel není díl.");
  Proto& p = *d.protos[static_cast<size_t>(n.proto)];
  d.proto_shape(p);
  d.ensure_maps(p);
  const ShapeMap& m = kind == 0 ? p.verts : (kind == 1 ? p.edges : p.faces);
  if (kind > 2 || index >= static_cast<uint32_t>(m.Extent())) throw Fail("Neplatný index entity.");
  TopoDS_Shape s = m.FindKey(static_cast<int>(index) + 1);
  return n.loc.IsIdentity() ? s : s.Moved(n.loc);
}

TopoDS_Shape shape_of_spec(const Doc& d, const ShapeSpec& s) {
  if (s.kind == 0) return entity_shape(d, s.node, s.ekind, s.index);
  if (s.kind == 1) {
    if (s.pts.size() < 3) throw Fail("Chybí souřadnice bodu.");
    return BRepBuilderAPI_MakeVertex(gp_Pnt(s.pts[0], s.pts[1], s.pts[2])).Vertex();
  }
  // viewer3d.py: polyline_wire
  BRepBuilderAPI_MakePolygon mp;
  const size_t n = s.pts.size() / 3;
  for (size_t i = 0; i < n; ++i) mp.Add(gp_Pnt(s.pts[i * 3], s.pts[i * 3 + 1], s.pts[i * 3 + 2]));
  if (s.closed && n > 2) mp.Close();
  return mp.Wire();
}

}  // namespace

// ------------------------------------------------------------------ single entity (analysis.py: Entity)

EntityRec doc_entity(const Doc& d, uint32_t node, uint8_t kind, uint32_t index) {
  BR_BEGIN(EntityRec)
  const TopoDS_Shape shape = entity_shape(d, node, kind, index);
  EntityRec r{};
  r.kind = kind;
  if (kind == 0) {
    r.point = arr(BRep_Tool::Pnt(TopoDS::Vertex(shape)));
  } else if (kind == 1) {
    BRepAdaptor_Curve c(TopoDS::Edge(shape));
    const GeomAbs_CurveType t = c.GetType();
    r.length = GCPnts_AbscissaPoint::Length(c);
    r.has_length = true;
    r.has_ends = true;
    r.p0 = arr(c.Value(c.FirstParameter()));
    r.p1 = arr(c.Value(c.LastParameter()));
    switch (t) {
      case GeomAbs_Line: {
        r.geom = 1;
        r.axis = arr(c.Line().Direction());
        r.has_axis = true;
        r.is_line = true;
        break;
      }
      case GeomAbs_Circle: {
        r.geom = 2;
        const gp_Circ ci = c.Circle();
        r.radius = ci.Radius();
        r.has_radius = true;
        r.center = arr(ci.Location());
        r.has_center = true;
        r.axis = arr(ci.Axis().Direction());
        r.has_axis = true;
        r.arc_deg = std::abs(c.LastParameter() - c.FirstParameter()) * kRadToDeg;
        r.has_arc = true;
        break;
      }
      case GeomAbs_Ellipse: {
        r.geom = 3;
        const gp_Elips el = c.Ellipse();
        r.center = arr(el.Location());
        r.has_center = true;
        r.axes = {el.MajorRadius(), el.MinorRadius()};
        r.has_axes = true;
        break;
      }
      case GeomAbs_BSplineCurve: r.geom = 4; break;
      default: r.geom = 0; break;
    }
  } else {
    const TopoDS_Face f = TopoDS::Face(shape);
    BRepAdaptor_Surface s(f);
    const GeomAbs_SurfaceType t = s.GetType();
    GProp_GProps g;
    BRepGProp::SurfaceProperties(f, g);
    r.area = g.Mass();
    r.has_area = true;
    r.centroid = arr(g.CentreOfMass());
    switch (t) {
      case GeomAbs_Plane: {
        r.geom = 10;
        const gp_Ax1 ax = s.Plane().Axis();
        gp_Dir dir = ax.Direction();
        if (f.Orientation() == TopAbs_REVERSED) dir.Reverse();
        r.axis = arr(dir);
        r.has_axis = true;
        r.center = arr(ax.Location());
        r.has_center = true;
        r.plane_point = arr(ax.Location());
        r.has_plane_point = true;
        break;
      }
      case GeomAbs_Cylinder: {
        r.geom = 11;
        const gp_Cylinder cy = s.Cylinder();
        r.radius = cy.Radius();
        r.has_radius = true;
        r.center = arr(cy.Location());
        r.has_center = true;
        r.axis = arr(cy.Axis().Direction());
        r.has_axis = true;
        r.is_line = true;
        break;
      }
      case GeomAbs_Cone: {
        r.geom = 12;
        const gp_Cone co = s.Cone();
        r.axis = arr(co.Axis().Direction());
        r.has_axis = true;
        r.center = arr(co.Apex());
        r.has_center = true;
        r.is_line = true;
        r.half_angle = co.SemiAngle() * kRadToDeg;
        r.has_half_angle = true;
        break;
      }
      case GeomAbs_Sphere: {
        r.geom = 13;
        const gp_Sphere sp = s.Sphere();
        r.radius = sp.Radius();
        r.has_radius = true;
        r.center = arr(sp.Location());
        r.has_center = true;
        break;
      }
      case GeomAbs_Torus: {
        r.geom = 14;
        const gp_Torus to = s.Torus();
        r.center = arr(to.Location());
        r.has_center = true;
        r.axis = arr(to.Axis().Direction());
        r.has_axis = true;
        r.radii = {to.MajorRadius(), to.MinorRadius()};
        r.has_radii = true;
        break;
      }
      case GeomAbs_BSplineSurface: r.geom = 15; break;
      default: r.geom = 0; break;
    }
  }
  return r;
  BR_END
}

DistRec doc_distance(const Doc& d, const ShapeSpec& a, const ShapeSpec& b) {
  BR_BEGIN(DistRec)
  const TopoDS_Shape sa = shape_of_spec(d, a);
  const TopoDS_Shape sb = shape_of_spec(d, b);
  BRepExtrema_DistShapeShape dss(sa, sb);
  DistRec r{};
  r.done = dss.IsDone();
  if (r.done) {
    r.nb_solutions = dss.NbSolution();
    if (r.nb_solutions > 0) {
      r.value = dss.Value();
      r.p1 = arr(dss.PointOnShape1(1));
      r.p2 = arr(dss.PointOnShape2(1));
    }
  }
  return r;
  BR_END
}

// analysis.py: face_normal_at. Outward normal of a B-rep face at a world point, reversed for REVERSED faces.
NormalRec doc_face_normal_at(const Doc& d, uint32_t node, uint32_t face, double x, double y, double z) {
  // BR_BEGIN turns a hardware fault inside OCCT (an access violation on a damaged surface) into an error, which the
  // Rust side reads as "no normal", instead of ending the process. A C++ exception still only means "no normal".
  BR_BEGIN(NormalRec)
  NormalRec out{};
  fault_point(FAULT_FACE_NORMAL);
  try {
    const TopoDS_Face f = TopoDS::Face(entity_shape(d, node, 2, face));
    occ::handle<Geom_Surface> surf = BRep_Tool::Surface(f);
    if (surf.IsNull()) return out;
    GeomAPI_ProjectPointOnSurf proj(gp_Pnt(x, y, z), surf);
    if (proj.NbPoints() < 1) return out;
    double u = 0.0, v = 0.0;
    proj.LowerDistanceParameters(u, v);
    GeomLProp_SLProps props(surf, u, v, 1, 1e-6);
    if (!props.IsNormalDefined()) return out;
    gp_Dir n = props.Normal();
    if (f.Orientation() == TopAbs_REVERSED) n.Reverse();
    out.valid = true;
    out.n = arr(n);
  } catch (...) {
    out.valid = false;
  }
  return out;
  BR_END
}

// ------------------------------------------------------------------ interference (analysis.py, collisions.py)

namespace {

struct PairResult {
  int kind = 0;  // 0 none, 1 touch ("dotyk"), 2 collision ("kolize")
  double volume = 0.0;
  TopoDS_Shape common;
};

// analysis.py: interference(s1, s2, touch, tol=1e-4). Any failure of the boolean means "no result".
PairResult interference(const TopoDS_Shape& s1, const TopoDS_Shape& s2, bool touch, double tol) {
  PairResult out;
  try {
    NCollection_List<TopoDS_Shape> args, tools;
    args.Append(s1);
    tools.Append(s2);
    BRepAlgoAPI_Common op;
    op.SetArguments(args);
    op.SetTools(tools);
    op.SetRunParallel(true);
    op.SetFuzzyValue(tol);
    op.Build();
    if (!op.IsDone()) return PairResult();
    TopoDS_Shape res = op.Shape();
    double vol = 0.0;
    if (!res.IsNull()) {
      GProp_GProps g;
      BRepGProp::VolumeProperties(res, g);
      vol = std::abs(g.Mass());
    }
    if (vol > 1e-3) {
      out.kind = 2;
      out.volume = vol;
      out.common = res;
      return out;
    }
    if (touch) {
      BRepExtrema_DistShapeShape dss(s1, s2);
      if (dss.IsDone() && dss.Value() <= 1e-3) {
        out.kind = 1;
        out.volume = 0.0;
        out.common = res;
        return out;
      }
    }
    out.volume = vol;
    return out;
  } catch (...) {
    return PairResult();
  }
}

std::vector<TopoDS_Shape> solids_of(const TopoDS_Shape& shape) {
  std::vector<TopoDS_Shape> out;
  for (TopExp_Explorer e(shape, TopAbs_SOLID); e.More(); e.Next()) out.push_back(e.Current());
  if (out.empty()) out.push_back(shape);
  return out;
}

Bnd_Box shape_box(const TopoDS_Shape& s, double gap) {
  Bnd_Box b;
  BRepBndLib::AddOptimal(s, b, true, false);
  if (gap != 0.0 && !b.IsVoid()) b.Enlarge(gap);
  return b;
}

// analysis.py: candidate_pairs. Sweep-and-prune on X, then Y and Z overlap.
std::vector<std::pair<int, int>> candidate_pairs(const std::vector<Bnd_Box>& boxes) {
  struct It {
    double x0, x1;
    int i;
    gp_Pnt lo, hi;
  };
  std::vector<It> items;
  for (size_t i = 0; i < boxes.size(); ++i) {
    if (boxes[i].IsVoid()) continue;
    const gp_Pnt lo = boxes[i].CornerMin();
    const gp_Pnt hi = boxes[i].CornerMax();
    items.push_back({lo.X(), hi.X(), static_cast<int>(i), lo, hi});
  }
  std::sort(items.begin(), items.end(), [](const It& a, const It& b) {
    if (a.x0 != b.x0) return a.x0 < b.x0;
    if (a.x1 != b.x1) return a.x1 < b.x1;
    return a.i < b.i;
  });
  std::vector<std::pair<int, int>> out;
  std::vector<It> active;
  for (const It& it : items) {
    std::vector<It> kept;
    kept.reserve(active.size());
    for (const It& a : active) {
      if (a.x1 >= it.x0) kept.push_back(a);
    }
    active.swap(kept);
    for (const It& a : active) {
      if (a.lo.Y() <= it.hi.Y() && it.lo.Y() <= a.hi.Y() && a.lo.Z() <= it.hi.Z() && it.lo.Z() <= a.hi.Z()) {
        out.emplace_back(a.i, it.i);
      }
    }
    active.push_back(it);
  }
  return out;
}

}  // namespace

std::unique_ptr<InterfRun> interf_begin(const Doc& d, rust::Slice<const uint32_t> nodes, double gap) {
  BR_BEGIN(std::unique_ptr<InterfRun>)
  auto run = std::make_unique<InterfRun>();
  if (!d.tree_built) throw Fail("Strom modelu ještě není sestaven.");
  // unique leaves in order of first appearance (main.py: _unique_leaves); STL meshes have no solids
  std::vector<uint32_t> leaves;
  for (uint32_t n : nodes) d.leaves_of(n, leaves);
  std::vector<uint32_t> uniq;
  {
    std::vector<char> seen(d.nodes.size(), 0);
    for (uint32_t l : leaves) {
      if (!seen[l]) {
        seen[l] = 1;
        uniq.push_back(l);
      }
    }
  }
  std::vector<Bnd_Box> boxes;
  for (uint32_t l : uniq) {
    const TNode& n = d.nodes[l];
    if (n.proto < 0) continue;
    Proto& p = *d.protos[static_cast<size_t>(n.proto)];
    const TopoDS_Shape& base = d.proto_shape(p);
    if (base.IsNull() || p.mesh_only) continue;
    const TopoDS_Shape located = d.located_shape(n);
    const uint32_t part = static_cast<uint32_t>(run->part_nodes.size());
    run->part_nodes.push_back(l);
    for (const TopoDS_Shape& so : solids_of(located)) {
      run->items.push_back({part, so});
      boxes.push_back(shape_box(so, gap));
    }
  }
  std::map<std::pair<uint32_t, uint32_t>, int> part_pair_set;
  for (const auto& pr : candidate_pairs(boxes)) {
    const uint32_t pa = run->items[static_cast<size_t>(pr.first)].part;
    const uint32_t pb = run->items[static_cast<size_t>(pr.second)].part;
    if (pa == pb) continue;
    run->pairs.push_back(pr);
    part_pair_set[{std::min(pa, pb), std::max(pa, pb)}] = 1;
  }
  run->part_pairs = part_pair_set.size();
  return run;
  BR_END
}

uint64_t interf_pair_count(const InterfRun& r) { return static_cast<uint64_t>(r.pairs.size()); }
uint64_t interf_part_pair_count(const InterfRun& r) { return r.part_pairs; }
uint64_t interf_collisions(const InterfRun& r) { return r.collisions; }

void interf_step(const InterfRun& cr, uint64_t k, bool touch) {
  BR_BEGIN(void)
  InterfRun& r = const_cast<InterfRun&>(cr);
  if (k >= r.pairs.size()) throw Fail("Neplatný index dvojice těles.");
  const auto [i, j] = r.pairs[static_cast<size_t>(k)];
  const uint32_t pa = r.items[static_cast<size_t>(i)].part;
  const uint32_t pb = r.items[static_cast<size_t>(j)].part;
  const std::pair<uint32_t, uint32_t> key{std::min(pa, pb), std::max(pa, pb)};
  auto it = r.index.find(key);
  const bool have = it != r.index.end();
  // one touch test per part pair is enough
  PairResult res = interference(r.items[static_cast<size_t>(i)].solid, r.items[static_cast<size_t>(j)].solid, touch && !have, 1e-4);
  if (res.kind == 0) return;
  if (!have) {
    r.found.push_back({key.first, key.second, false, 0.0, {}});
    it = r.index.emplace(key, r.found.size() - 1).first;
  }
  InterfRun::Found& f = r.found[it->second];
  if (res.kind == 2) {
    if (!f.collision) {
      f.collision = true;
      r.collisions += 1;
    }
    f.volume += res.volume;
    f.commons.push_back(res.common);
  }
  BR_END
}

rust::Vec<InterfRec> interf_results(const InterfRun& r) {
  BR_BEGIN(rust::Vec<InterfRec>)
  rust::Vec<InterfRec> out;
  BRep_Builder b;
  for (const InterfRun::Found& f : r.found) {
    InterfRec rec{};
    rec.a = r.part_nodes[f.a];
    rec.b = r.part_nodes[f.b];
    rec.collision = f.collision;
    rec.volume = f.collision ? f.volume : 0.0;
    if (!f.commons.empty()) {
      TopoDS_Shape common;
      if (f.commons.size() == 1) {
        common = f.commons[0];
      } else {
        TopoDS_Compound c;
        b.MakeCompound(c);
        for (const TopoDS_Shape& s : f.commons) b.Add(c, s);
        common = c;
      }
      Bnd_Box box;
      try {
        BRepBndLib::AddOptimal(common, box, false, false);
      } catch (...) {
      }
      if (!box.IsVoid()) {
        const gp_Pnt lo = box.CornerMin();
        const gp_Pnt hi = box.CornerMax();
        rec.has_bounds = true;
        rec.min = arr(lo);
        rec.max = arr(hi);
      }
    }
    out.push_back(rec);
  }
  return out;
  BR_END
}

// Overlay of the common volume(s) of two parts (viewer3d.py: show_shapes meshes with 0.05 / 0.3).
MeshRec doc_interference_mesh(const Doc& d, uint32_t a, uint32_t b) {
  BR_BEGIN(MeshRec)
  MeshRec out{};
  if (!d.tree_built || a >= d.nodes.size() || b >= d.nodes.size()) throw Fail("Neplatný uzel stromu.");
  const TNode& na = d.nodes[a];
  const TNode& nb = d.nodes[b];
  if (na.proto < 0 || nb.proto < 0) throw Fail("Vybraný uzel není díl.");
  const std::vector<TopoDS_Shape> sa = solids_of(d.located_shape(na));
  const std::vector<TopoDS_Shape> sb = solids_of(d.located_shape(nb));
  std::vector<TopoDS_Shape> commons;
  for (const TopoDS_Shape& x : sa) {
    const Bnd_Box bx = shape_box(x, 0.01);
    for (const TopoDS_Shape& y : sb) {
      const Bnd_Box by = shape_box(y, 0.01);
      if (bx.IsVoid() || by.IsVoid()) continue;
      const gp_Pnt alo = bx.CornerMin(), ahi = bx.CornerMax(), blo = by.CornerMin(), bhi = by.CornerMax();
      if (!(alo.X() <= bhi.X() && blo.X() <= ahi.X() && alo.Y() <= bhi.Y() && blo.Y() <= ahi.Y() &&
            alo.Z() <= bhi.Z() && blo.Z() <= ahi.Z())) {
        continue;
      }
      PairResult r = interference(x, y, false, 1e-4);
      if (r.kind == 2) commons.push_back(r.common);
    }
  }
  if (commons.empty()) return out;
  BRep_Builder bld;
  TopoDS_Compound comp;
  bld.MakeCompound(comp);
  for (const TopoDS_Shape& s : commons) bld.Add(comp, s);
  BRepMesh_IncrementalMesh(comp, 0.05, false, 0.3, true);
  ShapeMap faces, edges, verts;
  TopExp::MapShapes(comp, TopAbs_FACE, faces);
  TopExp::MapShapes(comp, TopAbs_EDGE, edges);
  TopExp::MapShapes(comp, TopAbs_VERTEX, verts);
  extract_mesh(comp, faces, edges, verts, false, nullptr, 0.05, out);
  return out;
  BR_END
}

}  // namespace stepview
