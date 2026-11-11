"""Geometry analysis: measuring picked vertices / edges / faces, mass properties, interference (collisions)."""
import math

from OCP.Bnd import Bnd_Box
from OCP.BRep import BRep_Tool
from OCP.BRepAdaptor import BRepAdaptor_Curve, BRepAdaptor_Surface
from OCP.BRepAlgoAPI import BRepAlgoAPI_Common
from OCP.BRepBndLib import BRepBndLib
from OCP.BRepExtrema import BRepExtrema_DistShapeShape
from OCP.BRepGProp import BRepGProp
from OCP.GCPnts import GCPnts_AbscissaPoint
from OCP.GeomAbs import (GeomAbs_BSplineCurve, GeomAbs_BSplineSurface, GeomAbs_Circle, GeomAbs_Cone,
                         GeomAbs_Cylinder, GeomAbs_Ellipse, GeomAbs_Line, GeomAbs_Plane, GeomAbs_Sphere,
                         GeomAbs_Torus)
from OCP.gp import gp_Pnt, gp_Vec
from OCP.GProp import GProp_GProps
from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_REVERSED, TopAbs_SOLID, TopAbs_VERTEX
from OCP.TopExp import TopExp_Explorer
from OCP.TopoDS import TopoDS
from OCP.collections import List_TopoDS_Shape as TopTools_ListOfShape

CURVE_NAMES = {GeomAbs_Line: "úsečka", GeomAbs_Circle: "kružnice", GeomAbs_Ellipse: "elipsa",
               GeomAbs_BSplineCurve: "spline"}
SURF_NAMES = {GeomAbs_Plane: "rovina", GeomAbs_Cylinder: "válec", GeomAbs_Cone: "kužel",
              GeomAbs_Sphere: "koule", GeomAbs_Torus: "anuloid", GeomAbs_BSplineSurface: "spline plocha"}


def _p(v, d=3):
    return f"{v:,.{d}f}".replace(",", " ").replace(".", ",")


def _pt(p):
    return f"{_p(p.X())}; {_p(p.Y())}; {_p(p.Z())}"


def _dir(d):
    return f"({d.X():.4f}; {d.Y():.4f}; {d.Z():.4f})".replace(".", ",")


# ---------------------------------------------------------------- single entity
class Entity:
    """A picked sub-shape in world coordinates plus what we know about it."""

    def __init__(self, shape):
        self.shape = shape
        self.kind = None          # "vertex" / "edge" / "face"
        self.type = ""            # "kružnice", "rovina", …
        self.point = None         # vertex point
        self.center = None        # circle / sphere / arc centre, or cylinder axis point
        self.axis = None          # gp_Dir: line direction, circle / cylinder axis, plane normal
        self.radius = None
        self.length = None
        self.area = None
        self.is_line = False      # axis is a line direction (edge line / cylinder axis), not a normal
        st = shape.ShapeType()
        if st == TopAbs_VERTEX:
            self.kind = "vertex"
            self.type = "bod"
            self.point = BRep_Tool.Pnt_s(TopoDS.Vertex(shape))
        elif st == TopAbs_EDGE:
            self.kind = "edge"
            self._edge(TopoDS.Edge(shape))
        elif st == TopAbs_FACE:
            self.kind = "face"
            self._face(TopoDS.Face(shape))

    def _edge(self, e):
        c = BRepAdaptor_Curve(e)
        t = c.GetType()
        self.type = CURVE_NAMES.get(t, "křivka")
        self.length = GCPnts_AbscissaPoint.Length_s(c)
        self.p0, self.p1 = c.Value(c.FirstParameter()), c.Value(c.LastParameter())
        if t == GeomAbs_Line:
            self.axis = c.Line().Direction()
            self.is_line = True
        elif t == GeomAbs_Circle:
            ci = c.Circle()
            self.radius = ci.Radius()
            self.center = ci.Location()
            self.axis = ci.Axis().Direction()
            self.arc = math.degrees(abs(c.LastParameter() - c.FirstParameter()))
            if self.arc < 359.9:
                self.type = "oblouk"
        elif t == GeomAbs_Ellipse:
            el = c.Ellipse()
            self.center = el.Location()
            self.axes = (el.MajorRadius(), el.MinorRadius())

    def _face(self, f):
        s = BRepAdaptor_Surface(f)
        t = s.GetType()
        self.type = SURF_NAMES.get(t, "plocha")
        g = GProp_GProps()
        BRepGProp.SurfaceProperties_s(f, g)
        self.area = g.Mass()
        self.centroid = g.CentreOfMass()
        if t == GeomAbs_Plane:
            ax = s.Plane().Axis()
            d = ax.Direction()
            self.axis = d.Reversed() if f.Orientation() == TopAbs_REVERSED else d
            self.center = ax.Location()
            self.plane_point = ax.Location()
        elif t == GeomAbs_Cylinder:
            cy = s.Cylinder()
            self.radius = cy.Radius()
            self.center = cy.Location()
            self.axis = cy.Axis().Direction()
            self.is_line = True
        elif t == GeomAbs_Cone:
            co = s.Cone()
            self.axis = co.Axis().Direction()
            self.center = co.Apex()
            self.is_line = True
            self.half_angle = math.degrees(co.SemiAngle())
        elif t == GeomAbs_Sphere:
            sp = s.Sphere()
            self.radius = sp.Radius()
            self.center = sp.Location()
        elif t == GeomAbs_Torus:
            to = s.Torus()
            self.center = to.Location()
            self.axis = to.Axis().Direction()
            self.radii = (to.MajorRadius(), to.MinorRadius())

    # ---- entities that are not B-rep sub-shapes: picked points, mesh loops (STL)
    @classmethod
    def from_item(cls, it):
        """it: viewer3d.PickItem."""
        if it.kind == "point":
            return cls.point(it.point, it.normal, mesh=it.mesh is not None, snapped=it.snapped, patch=it.patch)
        if it.kind == "loop":
            return cls.loop(it.loop, it.wire())
        return cls(it.shape)

    @classmethod
    def point(cls, p, normal=None, mesh=False, snapped=False, patch=None):
        """patch: (facets, area, is_face, centroid) of the flat mesh face under the point, or None."""
        e = cls.__new__(cls)
        e.__init_empty()
        from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeVertex
        e.kind = "point"
        e.type = ("vrchol sítě" if snapped else "bod na síti") if mesh else "bod na ploše"
        e.point = p
        e.shape = BRepBuilderAPI_MakeVertex(p).Vertex()
        if normal is not None:
            e.axis = normal
            e.plane_point = p                         # a point with a normal acts as a plane (thickness)
        if patch is not None:
            e.patch, e.patch_area = patch, patch[1]
            e.type += ", rovina"
        return e

    @classmethod
    def loop(cls, lp, wire):
        """Sharp-edge loop of a mesh; holes / pins come with a fitted circle."""
        e = cls.__new__(cls)
        e.__init_empty()
        e.kind = "edge"
        e.type = lp.kind() + " (síť)"
        e.shape = wire
        e.length = lp.length
        e.p0, e.p1 = gp_Pnt(*lp.pts[0]), gp_Pnt(*lp.pts[-1])
        e.loop_closed = lp.closed
        if lp.circle is not None:
            c, n, r, rms = lp.circle
            e.center, e.radius, e.fit_rms = gp_Pnt(*c), r, rms
            from OCP.gp import gp_Dir
            e.axis = gp_Dir(*n)
            e.arc = lp.coverage
        return e

    def __init_empty(self):
        self.shape = None
        self.kind = self.type = None
        self.point = self.center = self.axis = self.radius = self.length = self.area = None
        self.patch = self.patch_area = None
        self.is_line = False

    def short(self):
        """One line for the hover preview."""
        t = f"{self.kind_cz()}: {self.type}"
        if self.radius is not None:
            t += f"  Ø {_p(2 * self.radius, 3)}  R {_p(self.radius, 3)}"
        if self.kind == "edge" and self.length is not None:
            t += f"  L {_p(self.length, 3)}"
        if self.kind == "face" and self.area is not None:
            t += f"  A {_p(self.area, 2)} mm²"
        if self.kind in ("vertex", "point") and self.point is not None:
            t += f"  ({_pt(self.point)})"
        if getattr(self, "patch_area", None) is not None:
            t += f"  A {_p(self.patch_area, 2)} mm²"
        return t

    def label_pos(self):
        """Where a 3D label for this entity goes, and its text."""
        if self.kind == "edge":
            p = self.p0 if self.radius is not None else gp_Pnt((self.p0.X() + self.p1.X()) / 2,
                                                              (self.p0.Y() + self.p1.Y()) / 2,
                                                              (self.p0.Z() + self.p1.Z()) / 2)
            txt = f"Ø {_p(2 * self.radius, 2)}" if self.radius is not None else f"{_p(self.length, 2)}"
            return p, txt
        if self.kind == "face":
            txt = f"Ø {_p(2 * self.radius, 2)}" if self.radius is not None else f"{_p(self.area, 1)} mm²"
            return self.centroid, txt
        if getattr(self, "patch_area", None) is not None:
            return gp_Pnt(*self.patch[3]), f"{_p(self.patch_area, 1)} mm²"
        return None, None

    def rows(self):
        r = [("Entita", f"{self.kind_cz()} – {self.type}")]
        if self.kind in ("vertex", "point"):
            r.append(("X; Y; Z", _pt(self.point)))
            if self.kind == "point" and self.axis is not None:
                r.append(("Normála", _dir(self.axis)))
            if getattr(self, "patch_area", None) is not None:
                a = self.patch_area
                r.append(("Rovinná plocha (síť)", f"{_p(a, 2)} mm²  ({_p(a / 100.0, 3)} cm², {len(self.patch[0])} trojúh.)"))
        if self.kind == "edge":
            r.append(("Délka", f"{_p(self.length)} mm"))
            if self.radius is not None:
                r += [("Poloměr R", f"{_p(self.radius)} mm"), ("Průměr Ø", f"{_p(2 * self.radius)} mm"),
                      ("Střed", _pt(self.center)), ("Osa", _dir(self.axis))]
                if self.type.startswith("oblouk"):
                    r.append(("Úhel oblouku", f"{_p(self.arc, 2)}°"))
                if getattr(self, "fit_rms", None) is not None:
                    r.append(("Proložení kružnice", f"± {_p(self.fit_rms, 3)} mm (ze sítě)"))
            elif self.is_line:
                d = gp_Vec(self.p0, self.p1)
                r += [("Směr", _dir(self.axis)),
                      ("ΔX; ΔY; ΔZ", f"{_p(d.X())}; {_p(d.Y())}; {_p(d.Z())}")]
            if getattr(self, "axes", None):
                r.append(("Poloosy", f"{_p(self.axes[0])} / {_p(self.axes[1])} mm"))
            r += [("Začátek", _pt(self.p0)), ("Konec", _pt(self.p1))]
        if self.kind == "face":
            r.append(("Plocha", f"{_p(self.area, 2)} mm²  ({_p(self.area / 100.0, 3)} cm²)"))
            if self.radius is not None:
                r += [("Poloměr R", f"{_p(self.radius)} mm"), ("Průměr Ø", f"{_p(2 * self.radius)} mm")]
            if self.type == "rovina":
                r.append(("Normála", _dir(self.axis)))
            elif self.axis is not None:
                r.append(("Osa", _dir(self.axis)))
            if self.type == "kužel":
                r.append(("Poloviční úhel", f"{_p(self.half_angle, 2)}°"))
            if getattr(self, "radii", None):
                r.append(("R / r", f"{_p(self.radii[0])} / {_p(self.radii[1])} mm"))
            r.append(("Těžiště plochy", _pt(self.centroid)))
        return r

    def kind_cz(self):
        return {"vertex": "vrchol", "edge": "hrana", "face": "plocha", "point": "bod"}.get(self.kind, "?")

    def ref_point(self):
        """Characteristic point for centre-to-centre distances."""
        if self.kind in ("vertex", "point"):
            return self.point
        if self.radius is not None and self.center is not None and self.kind == "edge":
            return self.center
        if self.type == "koule":
            return self.center
        return None


def face_normal_at(face_shape, p):
    """Outward surface normal (gp_Dir) of a B-rep face at the point p, or None."""
    try:
        from OCP.GeomAPI import GeomAPI_ProjectPointOnSurf
        from OCP.GeomLProp import GeomLProp_SLProps
        f = TopoDS.Face(face_shape)
        surf = BRep_Tool.Surface_s(f)
        if surf is None:
            return None
        proj = GeomAPI_ProjectPointOnSurf(p, surf)
        if proj.NbPoints() < 1:
            return None
        u, v = proj.LowerDistanceParameters()
        props = GeomLProp_SLProps(surf, u, v, 1, 1e-6)
        if not props.IsNormalDefined():
            return None
        n = props.Normal()
        return n.Reversed() if f.Orientation() == TopAbs_REVERSED else n
    except Exception:  # noqa: BLE001 – degenerate geometry
        return None


# ---------------------------------------------------------------- two entities
def _line_line_distance(p1, d1, p2, d2):
    """Distance of two (infinite) lines; for parallel lines the perpendicular distance."""
    v1, v2 = gp_Vec(d1), gp_Vec(d2)
    w = gp_Vec(p1, p2)
    n = v1.Crossed(v2)
    if n.Magnitude() < 1e-9:                      # parallel
        return w.Crossed(v1).Magnitude() / v1.Magnitude(), True
    return abs(w.Dot(n)) / n.Magnitude(), False


def relation(a, b):
    """Rows + drawing data for two entities: min distance (+ global ΔX/ΔY/ΔZ), centre distance, angle."""
    rows, draw = [], {}
    dss = BRepExtrema_DistShapeShape(a.shape, b.shape)
    if dss.IsDone() and dss.NbSolution() > 0:
        p1, p2 = dss.PointOnShape1(1), dss.PointOnShape2(1)
        d = gp_Vec(p1, p2)
        rows += [("<b>Min. vzdálenost</b>", f"<b>{_p(dss.Value())} mm</b>"),
                 ("ΔX / ΔY / ΔZ", f"{_p(d.X())} / {_p(d.Y())} / {_p(d.Z())} mm"),
                 ("Bod 1", _pt(p1)), ("Bod 2", _pt(p2))]
        draw["min"] = (p1, p2)
    ra, rb = a.ref_point(), b.ref_point()
    if ra is not None and rb is not None:
        d = gp_Vec(ra, rb)
        rows += [("Vzdálenost středů", f"{_p(ra.Distance(rb))} mm"),
                 ("Středy ΔX / ΔY / ΔZ", f"{_p(d.X())} / {_p(d.Y())} / {_p(d.Z())} mm")]
        draw["center"] = (ra, rb)
    if a.axis is not None and b.axis is not None:
        ang = math.degrees(gp_Vec(a.axis).Angle(gp_Vec(b.axis)))
        if a.is_line == b.is_line:                 # line-line or normal-normal
            if a.is_line:
                ang = min(ang, 180 - ang)
            rows.append(("Úhel", f"{_p(ang, 3)}°"))
        else:                                      # line vs plane normal -> line to plane angle
            rows.append(("Úhel (přímka – rovina)", f"{_p(abs(90 - min(ang, 180 - ang)), 3)}°"))
        # parallel planes (B-rep planes or picked points with their surface normal) -> perpendicular
        # distance = wall thickness / plane spacing; mesh normals are noisy, so 2° tolerance there
        pa, pb = getattr(a, "plane_point", None), getattr(b, "plane_point", None)
        tol = 1e-4 if a.type == "rovina" and b.type == "rovina" else 2.0
        if pa is not None and pb is not None and min(ang, 180 - ang) < tol:
            dist = abs(gp_Vec(pa, pb).Dot(gp_Vec(a.axis)))
            label = "Vzdálenost rovin" if a.type == "rovina" and b.type == "rovina" else "Kolmá vzdálenost (tloušťka)"
            rows.append((f"<b>{label}</b>", f"<b>{_p(dist)} mm</b>"))
        if a.is_line and b.is_line and a.center is not None and b.center is not None:
            dist, par = _line_line_distance(a.center, a.axis, b.center, b.axis)
            rows.append(("Vzdálenost os" + (" (rovnoběžné)" if par else " (mimoběžné)"), f"{_p(dist)} mm"))
    return rows, draw


def summary(ents):
    """3+ entities: totals."""
    rows = [("Vybráno", f"{len(ents)} entit")]
    edges = [e for e in ents if e.kind == "edge"]
    faces = [e for e in ents if e.kind == "face"]
    if edges:
        rows.append(("Součet délek hran", f"{_p(sum(e.length for e in edges))} mm ({len(edges)} hran)"))
    if faces:
        a = sum(e.area for e in faces)
        rows.append(("Součet ploch", f"{_p(a, 2)} mm²  ({_p(a / 100.0, 3)} cm², {len(faces)} ploch)"))
    patches = {id(e.patch[0]): e.patch_area for e in ents if getattr(e, "patch_area", None) is not None}
    if patches:
        a = sum(patches.values())
        rows.append(("Součet rovinných ploch (síť)", f"{_p(a, 2)} mm²  ({_p(a / 100.0, 3)} cm², {len(patches)} ploch)"))
    return rows


# ---------------------------------------------------------------- mass properties
def proto_props(shape, mesh=False):
    """(volume mm³, area mm², centroid gp_Pnt in the shape's coords, solids, faces).
    Volume / centroid are exact (B-rep); the surface area comes from the display mesh – exact surface
    integration was 2/3 of the time (3.9 s of 5.9 s on an 871-part assembly) for a value that only gets shown."""
    g = GProp_GProps()
    BRepGProp.VolumeProperties_s(shape, g, False, False, mesh)
    vol = g.Mass()
    cog = g.CentreOfMass() if abs(vol) > 1e-12 else gp_Pnt()
    g2 = GProp_GProps()
    BRepGProp.SurfaceProperties_s(shape, g2, False, True)
    ns = nf = 0
    e = TopExp_Explorer(shape, TopAbs_SOLID)
    while e.More():
        ns += 1
        e.Next()
    e = TopExp_Explorer(shape, TopAbs_FACE)
    while e.More():
        nf += 1
        e.Next()
    return abs(vol), g2.Mass(), cog, ns, nf


# ---------------------------------------------------------------- interference
def solids_of(shape):
    """Solids of a part (the part itself when it has none, e.g. a sheet body)."""
    out = []
    e = TopExp_Explorer(shape, TopAbs_SOLID)
    while e.More():
        out.append(e.Current())
        e.Next()
    return out or [shape]


def compound(shapes):
    from OCP.BRep import BRep_Builder
    from OCP.TopoDS import TopoDS_Compound
    b = BRep_Builder()
    c = TopoDS_Compound()
    b.MakeCompound(c)
    for s in shapes:
        b.Add(c, s)
    return c


def shape_box(shape, gap=0.0):
    b = Bnd_Box()
    BRepBndLib.AddOptimal_s(shape, b, True, False)
    if gap:
        b.Enlarge(gap)
    return b


def candidate_pairs(boxes):
    """Sweep-and-prune on X: index pairs whose boxes overlap. boxes: list of Bnd_Box (may be void)."""
    items = []
    for i, b in enumerate(boxes):
        if b is None or b.IsVoid():
            continue
        lo, hi = b.CornerMin(), b.CornerMax()
        items.append((lo.X(), hi.X(), i, lo, hi))
    items.sort()
    out = []
    active = []
    for x0, x1, i, lo, hi in items:
        active = [a for a in active if a[1] >= x0]
        for a in active:
            alo, ahi = a[3], a[4]
            if alo.Y() <= hi.Y() and lo.Y() <= ahi.Y() and alo.Z() <= hi.Z() and lo.Z() <= ahi.Z():
                out.append((a[2], i))
        active.append((x0, x1, i, lo, hi))
    return out


def interference(s1, s2, touch=False, tol=1e-4):
    """Returns (kind, volume, common shape): kind None / "kolize" / "dotyk"."""
    args, tools = TopTools_ListOfShape(), TopTools_ListOfShape()
    args.Append(s1)
    tools.Append(s2)
    op = BRepAlgoAPI_Common()
    op.SetArguments(args)
    op.SetTools(tools)
    op.SetRunParallel(True)
    op.SetFuzzyValue(tol)
    try:
        op.Build()
    except Exception:  # noqa: BLE001 – boolean failure on bad geometry
        return None, 0.0, None
    if not op.IsDone():
        return None, 0.0, None
    res = op.Shape()
    vol = 0.0
    if not res.IsNull():
        g = GProp_GProps()
        BRepGProp.VolumeProperties_s(res, g)
        vol = abs(g.Mass())
    if vol > 1e-3:
        return "kolize", vol, res
    if touch:
        dss = BRepExtrema_DistShapeShape(s1, s2)
        if dss.IsDone() and dss.Value() <= 1e-3:
            return "dotyk", 0.0, res
    return None, vol, None
