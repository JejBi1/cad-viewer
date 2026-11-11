"""OCCT V3d view embedded in a Qt widget."""
import ctypes
import math
import time

from PySide6.QtCore import QPoint, Qt, QTimer, Signal
from PySide6.QtWidgets import QWidget

from OCP.AIS import (AIS_ConnectedInteractive, AIS_InteractiveContext, AIS_SelectionScheme_Replace,
                     AIS_SelectionScheme_XOR, AIS_Shape, AIS_TextLabel, AIS_ViewCube, AIS_ViewCubeOwner)
from OCP.XCAFPrs import XCAFPrs_AISObject
from OCP.Aspect import (Aspect_DisplayConnection, Aspect_GradientFillMethod_Vertical,
                        Aspect_TOTP_LEFT_LOWER, Aspect_TOTP_RIGHT_UPPER, Aspect_TOL_SOLID)
from OCP.BRep import BRep_Builder
from OCP.BRepBuilderAPI import BRepBuilderAPI_MakeEdge, BRepBuilderAPI_MakeVertex
from OCP.Graphic3d import (Graphic3d_HTA_CENTER, Graphic3d_TMF_2d, Graphic3d_VTA_BOTTOM,
                           Graphic3d_ZLayerId_TopOSD)
from OCP.TopAbs import TopAbs_EDGE, TopAbs_FACE, TopAbs_VERTEX
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS_Compound
from OCP.Graphic3d import Graphic3d_ZLayerId_Top, Graphic3d_ZLayerId_Topmost
from OCP.Aspect import Aspect_TOL_DASH
from OCP.Prs3d import Prs3d_PointAspect
from OCP.Aspect import Aspect_TOM_BALL
from OCP.GeomAbs import GeomAbs_C0
from OCP.gp import gp_Ax1, gp_Dir, gp_Pln, gp_Pnt, gp_Trsf, gp_Vec
from OCP.Graphic3d import (Graphic3d_Camera, Graphic3d_ClipPlane, Graphic3d_SequenceOfHClipPlane,
                           Graphic3d_TMF_TriedronPers, Graphic3d_TransformPers,
                           Graphic3d_NameOfMaterial_Plastified)
from OCP.collections import Vec2_int as Graphic3d_Vec2i
from OCP.OpenGl import OpenGl_GraphicDriver
from OCP.Prs3d import Prs3d_LineAspect
from OCP.Quantity import Quantity_Color, Quantity_TOC_sRGB
from OCP.TCollection import TCollection_AsciiString, TCollection_ExtendedString
from OCP.V3d import (V3d_TypeOfOrientation_Zup_AxoRight, V3d_TypeOfOrientation_Zup_Back,
                     V3d_TypeOfOrientation_Zup_Bottom, V3d_TypeOfOrientation_Zup_Front,
                     V3d_TypeOfOrientation_Zup_Left, V3d_TypeOfOrientation_Zup_Right,
                     V3d_TypeOfOrientation_Zup_Top, V3d_Xneg, V3d_Xpos, V3d_Yneg, V3d_Ypos,
                     V3d_Zneg, V3d_Zpos, V3d_Viewer)
import sys

if sys.platform == "win32":
    from OCP.WNT import WNT_Window
else:
    from OCP.Xw import Xw_Window

ORIENT = {
    "iso": V3d_TypeOfOrientation_Zup_AxoRight,
    "front": V3d_TypeOfOrientation_Zup_Front,
    "back": V3d_TypeOfOrientation_Zup_Back,
    "top": V3d_TypeOfOrientation_Zup_Top,
    "bottom": V3d_TypeOfOrientation_Zup_Bottom,
    "left": V3d_TypeOfOrientation_Zup_Left,
    "right": V3d_TypeOfOrientation_Zup_Right,
}


DEFAULT_MSAA = 2


def msaa_samples():
    """MSAA samples of the 3D view (0 = off, 2, 4, 8): environment STEPVIEW_MSAA, else the saved option "msaa"
    (QSettings CadProhlizec/StepViewer), else DEFAULT_MSAA. The multisampled frame buffer is the biggest
    single user of graphics memory (a few hundred MB at full HD on some drivers)."""
    import os
    v = os.environ.get("STEPVIEW_MSAA")
    if v is None:
        try:
            from PySide6.QtCore import QSettings
            v = QSettings("CadProhlizec", "StepViewer").value("msaa")
        except Exception:  # noqa: BLE001
            v = None
    try:
        n = int(v)
    except (TypeError, ValueError):
        return DEFAULT_MSAA
    return n if n in (0, 2, 4, 8) else DEFAULT_MSAA


def rgb(r, g, b):
    return Quantity_Color(r / 255.0, g / 255.0, b / 255.0, Quantity_TOC_sRGB)


def nice_step(raw):
    """1 / 2 / 5 × 10^n not smaller than raw."""
    if raw <= 0:
        return 1.0
    e = 10.0 ** math.floor(math.log10(raw))
    for m in (1.0, 2.0, 5.0, 10.0):
        if m * e >= raw - 1e-12:
            return m * e
    return 10.0 * e


def fmt_val(v):
    if abs(v) >= 1000:
        s = f"{v:,.0f}".replace(",", " ")
    else:
        s = f"{v:.6g}"
    return s.replace(".", ",")


VERTEX_MODE = AIS_Shape.SelectionMode_s(TopAbs_VERTEX)
ENTITY_MODES = (VERTEX_MODE, AIS_Shape.SelectionMode_s(TopAbs_EDGE), AIS_Shape.SelectionMode_s(TopAbs_FACE))
PICK_COLOR = (255, 140, 0)
HOVER_COLOR = (0, 170, 255)
AXIS_COLORS = ((220, 40, 40), (30, 160, 50), (40, 90, 230))     # X, Y, Z components


class PickItem:
    """One picked thing of the measure tool. kind: "shape" (B-rep vertex / edge / face in world coords),
    "point" (exact point, optional surface normal; mesh points may be snapped to a mesh vertex),
    "loop" (sharp-edge polyline of a mesh, maybe with a fitted circle). A mesh point on a flat face of the
    model carries patch = (facets, area, is_face, centroid) of that face."""
    __slots__ = ("kind", "shape", "point", "normal", "mesh", "snapped", "loop", "patch", "_wire")

    def __init__(self, kind, shape=None, point=None, normal=None, mesh=None, snapped=False, loop=None, patch=None):
        self.kind, self.shape, self.point, self.normal = kind, shape, point, normal
        self.mesh, self.snapped, self.loop, self.patch, self._wire = mesh, snapped, loop, patch, None

    def wire(self):
        if self._wire is None and self.loop is not None:
            self._wire = polyline_wire(self.loop.pts, self.loop.closed)
        return self._wire

    def patch_face(self):
        """Triangulated face of the flat mesh patch (cached per mesh) - for highlighting it."""
        if self.patch is None:
            return None
        cache = self.mesh.display_cache
        key = id(self.patch[0])
        if key not in cache:
            while len(cache) >= 32:                      # hovering many faces must not pile up triangulations
                cache.pop(next(iter(cache)))
            cache[key] = mesh_patch_face(self.mesh, self.patch[0])
        return cache[key]

    def same(self, other):
        if self.kind != other.kind:
            return False
        if self.kind == "shape":
            return self.shape.IsSame(other.shape)
        if self.kind == "loop":
            return self.loop is other.loop
        return self.point.Distance(other.point) < 1e-9


def polyline_wire(pts, closed):
    from OCP.BRepBuilderAPI import BRepBuilderAPI_MakePolygon
    mp = BRepBuilderAPI_MakePolygon()
    for x, y, z in (pts.tolist() if hasattr(pts, "tolist") else pts):     # plain floats: no numpy scalar per call
        mp.Add(gp_Pnt(x, y, z))
    if closed and len(pts) > 2:
        mp.Close()
    return mp.Wire()


def mesh_patch_face(mesh, facets):
    """A face carrying only a triangulation (no surface) made of some facets of a mesh. OCCT's STL reader builds
    it from a private temporary file: filling a Poly_Triangulation node by node from Python took 1.7 s for
    170 000 facets (a hover freeze), this takes ~0.05 s."""
    import os
    import struct
    import tempfile
    import numpy as np
    from OCP.RWStl import RWStl
    from OCP.TopoDS import TopoDS_Face
    rec = np.zeros(len(facets), dtype=np.dtype([("n", "<f4", 3), ("v", "<f4", (3, 3)), ("a", "<u2")]))
    rec["v"] = mesh.nodes[mesh.tris[facets]]
    fd, path = tempfile.mkstemp(prefix="cadview_", suffix=".stl")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(bytes(80) + struct.pack("<I", len(rec)) + rec.tobytes())
        tri = RWStl.ReadFile_s(path)
    finally:
        try:
            os.remove(path)
        except OSError:
            pass
    if tri is None or tri.NbTriangles() == 0:
        return None
    tri.ComputeNormals()                              # without normals the overlay shades washed out
    f = TopoDS_Face()
    BRep_Builder().MakeFace(f, tri)
    return f


def polylines_compound(loops):
    b = BRep_Builder()
    comp = TopoDS_Compound()
    b.MakeCompound(comp)
    for lp in loops:
        if len(lp.pts) >= 2:
            try:
                b.Add(comp, polyline_wire(lp.pts, lp.closed))
            except Exception:  # noqa: BLE001 – degenerate (duplicate) points
                pass
    return comp


def _shape_mid(s):
    """A point in the middle of an edge (or the shape's first vertex)."""
    from OCP.BRepAdaptor import BRepAdaptor_Curve
    from OCP.TopoDS import TopoDS
    try:
        c = BRepAdaptor_Curve(TopoDS.Edge(s))
        return c.Value((c.FirstParameter() + c.LastParameter()) / 2)
    except Exception:  # noqa: BLE001
        return None


def standalone_object(n):
    """Leaf node -> its own XCAF presentation (instance colours included)."""
    o = XCAFPrs_AISObject(n.label)
    if not n.parent_loc.IsIdentity():
        o.SetLocalTransformation(n.parent_loc.Transformation())
    return o


def leaf_objects(leaves, proto_count, style):
    """One AIS object per leaf node; style(obj) sets colour / edges of every presentation.

    Parts used more than once share one presentation: AIS_ConnectedInteractive instances of a single
    (never displayed) XCAFPrs prototype. A 2 700-instance assembly then displays ~8x faster with a third
    of the RAM. Instances with their own colour keep a standalone object. Returns (objects, protos)."""
    protos, out = {}, []
    for n in leaves:
        if proto_count.get(n.key, 1) > 1 and not n.own_style:
            p = protos.get(n.key)
            if p is None:
                p = protos[n.key] = XCAFPrs_AISObject(n.proto)
                style(p)
            o = AIS_ConnectedInteractive()
            o.Connect(p, n.loc.Transformation())
        else:
            o = standalone_object(n)
            style(o)
        out.append(o)
    return out, protos


def _capsule(ptr):
    f = ctypes.pythonapi.PyCapsule_New
    f.restype = ctypes.py_object
    f.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_void_p]
    return f(ptr, None, None)


class OccView(QWidget):
    selectionChanged = Signal()           # selection changed by clicking in 3D
    contextMenuAt = Signal(QPoint)        # right click (no drag) at global pos
    measured = Signal(object, object)     # p1, p2 (gp_Pnt)
    angleMeasured = Signal(float)         # degrees
    entitiesPicked = Signal(list)         # measure tool: picked vertices / edges / faces (world TopoDS shapes)
    hoverText = Signal(str)
    zoomSelected = Signal()               # double click on a part

    def __init__(self, parent=None):
        super().__init__(parent)
        for a in (Qt.WA_NativeWindow, Qt.WA_PaintOnScreen, Qt.WA_NoSystemBackground):
            self.setAttribute(a)
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.StrongFocus)
        self.setMinimumSize(200, 200)

        self._disp = Aspect_DisplayConnection()
        self.driver = OpenGl_GraphicDriver(self._disp, True)
        self.viewer = V3d_Viewer(self.driver)
        self.viewer.SetDefaultLights()
        self.viewer.SetLightOn()
        self.ctx = AIS_InteractiveContext(self.viewer)
        self.view = self.viewer.CreateView()
        self._window = None
        self._press_pos = None
        self._press_btn = None
        self._dragging = False
        self._last = None
        self.measure_mode = None          # None / "dist" / "angle"
        self._measure_pts = []
        self._measure_objs = []
        self._snap_objs = []
        self._snap_pool = set()
        self._snap_modes = ()
        self._picked = []                 # measure tool: PickItems
        self._pinned = []                 # kept measurements (Enter)
        self.meshes = {}                  # AIS object of a mesh part (STL) -> meshmeasure.Mesh
        self.feature_objs = {}            # AIS of the sharp-edge lines of a mesh -> its Mesh
        self.face_normal = None           # callable(face, gp_Pnt) -> gp_Dir (set by the window)
        self.describe = None              # callable(PickItem) -> one-line text for the hover preview
        self._hover_key = None
        self._hover_obj = self._hover_obj_key = None   # drawn highlight of a mesh loop / flat face
        self._quiet_hilight = None        # invisible hover style of meshes while measuring
        self._sm_last = 0.0               # 3D mouse: time of the last motion, its rotation pivot
        self._sm_pivot = None
        self._pick_objs = []              # overlays of the picked entities
        self._overlay = []                # other analysis overlays (centre of gravity, collisions)
        self.scale_bar_on = True
        self._bar_objs = []
        self._bar_key = None
        self._dark = False
        self.clip = None
        self._anim = QTimer(self)
        self._anim.setInterval(15)
        self._anim.timeout.connect(self._animate)
        self._edge_aspect = Prs3d_LineAspect(rgb(25, 25, 30), Aspect_TOL_SOLID, 1.0)
        dr = self.ctx.DefaultDrawer()
        dr.SetAutoTriangulation(False)
        hs = self.ctx.HighlightStyle()
        hs.SetColor(rgb(80, 200, 255))
        hs.SetDisplayMode(1)
        ss = self.ctx.SelectionStyle()
        ss.SetColor(rgb(255, 140, 0))
        ss.SetDisplayMode(1)
        self.ctx.SetPixelTolerance(3) if hasattr(self.ctx, "SetPixelTolerance") else None

    # ------------------------------------------------------------ Qt plumbing
    def paintEngine(self):
        return None

    def dpr(self):
        return self.devicePixelRatioF()

    def _px(self, pos):
        r = self.dpr()
        return int(pos.x() * r), int(pos.y() * r)

    def showEvent(self, e):
        super().showEvent(e)
        if self._window is None:
            if sys.platform == "win32":
                self._window = WNT_Window(_capsule(int(self.winId())))
            else:                                    # X11 (Wayland sessions run Qt via XWayland, see main)
                self._window = Xw_Window(self._disp, int(self.winId()))
            self.view.SetWindow(self._window)
            if not self._window.IsMapped():
                self._window.Map()
            self._setup_view()
            QTimer.singleShot(0, self._force_resize)
            QTimer.singleShot(300, self._force_resize)

    def _force_resize(self):
        self._size = None
        self.redraw()

    def _setup_view(self):
        v = self.view
        self.set_dark_background(False, redraw=False)
        rp = v.ChangeRenderingParams()
        rp.NbMsaaSamples = msaa_samples()
        rp.IsAntialiasingEnabled = True
        v.SetProj(ORIENT["iso"])
        cube = AIS_ViewCube()
        s = self.dpr()
        cube.SetSize(55 * s)
        cube.SetFontHeight(11 * s)
        cube.SetBoxColor(rgb(225, 228, 235))
        cube.SetTransformPersistence(Graphic3d_TransformPers(
            Graphic3d_TMF_TriedronPers, Aspect_TOTP_RIGHT_UPPER, Graphic3d_Vec2i(int(90 * s), int(90 * s))))
        for side, txt in ((V3d_Yneg, "Přední"), (V3d_Ypos, "Zadní"), (V3d_Zpos, "Horní"),
                          (V3d_Zneg, "Spodní"), (V3d_Xneg, "Levý"), (V3d_Xpos, "Pravý")):
            cube.SetBoxSideLabel(side, TCollection_AsciiString(txt))
        cube.SetDuration(0.35)
        cube.SetFitSelected(False)
        noclip = Graphic3d_SequenceOfHClipPlane()
        noclip.SetOverrideGlobal(True)
        cube.SetClipPlanes(noclip)
        self.cube = cube
        self.ctx.Display(cube, False)
        v.MustBeResized()

    def set_dark_background(self, dark, redraw=True):
        top, bottom = ((50, 54, 62), (20, 22, 26)) if dark else ((230, 235, 242), (150, 162, 180))
        self.view.SetBgGradientColors(rgb(*top), rgb(*bottom), Aspect_GradientFillMethod_Vertical, False)
        self._dark = dark
        self._bar_key = None
        if redraw:
            self.redraw()

    def resizeEvent(self, e):
        super().resizeEvent(e)
        if self._window is not None:
            self.view.MustBeResized()
            QTimer.singleShot(0, self.redraw)

    def paintEvent(self, e):
        self.redraw()

    def redraw(self):
        if self._window is None:
            return
        cur = (int(self.width() * self.dpr()), int(self.height() * self.dpr()))
        if getattr(self, "_size", None) != cur:
            self._size = cur
            self._window.DoResize()
            self.view.MustBeResized()
        self._update_scale_bar()
        self.view.Redraw()

    # ------------------------------------------------------------ scale bar ("pravítko")
    def set_scale_bar(self, on):
        self.scale_bar_on = on
        self._bar_key = None
        self.redraw()

    def _remove_bar(self):
        for o in self._bar_objs:
            self.ctx.Remove(o, False)
        self._bar_objs = []

    def _update_scale_bar(self):
        cam = self.view.Camera()
        persp = cam.ProjectionType() != Graphic3d_Camera.Projection_e.Projection_Orthographic
        # ortho Scale() is the view height for wide windows but the view WIDTH for tall ones
        h = min(self._size) if getattr(self, "_size", None) else 0
        if not self.scale_bar_on or persp or h <= 0:
            if self._bar_objs:
                self._remove_bar()
                self._bar_key = None
            return
        upp = cam.Scale() / h                      # model units per pixel (ortho)
        key = (round(math.log(upp), 6), self._dark, h)
        if key == self._bar_key:
            return
        self._bar_key = key
        self._remove_bar()
        s = self.dpr()
        length = nice_step(110 * s * upp)
        L = length / upp
        tk = 7 * s
        b = BRep_Builder()
        comp = TopoDS_Compound()
        b.MakeCompound(comp)
        segs = [((0, 0), (L, 0)), ((0, 0), (0, tk)), ((L, 0), (L, tk)), ((L / 2, 0), (L / 2, tk * 0.6))]
        for i in range(1, 10):
            if i != 5:
                segs.append(((L * i / 10, 0), (L * i / 10, tk * 0.35)))
        for (x0, y0), (x1, y1) in segs:
            b.Add(comp, BRepBuilderAPI_MakeEdge(gp_Pnt(x0, y0, 0), gp_Pnt(x1, y1, 0)).Edge())
        col = rgb(235, 235, 235) if self._dark else rgb(30, 32, 38)
        pers = Graphic3d_TransformPers(Graphic3d_TMF_2d, Aspect_TOTP_LEFT_LOWER,
                                       Graphic3d_Vec2i(int(24 * s), int(22 * s)))
        bar = AIS_Shape(comp)
        bar.SetColor(col)
        bar.SetWidth(1.6)
        lbl = AIS_TextLabel()
        lbl.SetText(TCollection_ExtendedString(f"{fmt_val(length)} mm", True))
        lbl.SetPosition(gp_Pnt(L / 2, tk + 3 * s, 0))
        lbl.SetHJustification(Graphic3d_HTA_CENTER)
        lbl.SetVJustification(Graphic3d_VTA_BOTTOM)
        lbl.SetColor(col)
        lbl.SetHeight(13 * s)
        noclip = Graphic3d_SequenceOfHClipPlane()
        noclip.SetOverrideGlobal(True)
        for o in (bar, lbl):
            o.SetTransformPersistence(pers)
            o.SetZLayer(Graphic3d_ZLayerId_TopOSD)
            o.SetClipPlanes(noclip)
            self.ctx.Display(o, False)
            self.ctx.Deactivate(o)
            self._bar_objs.append(o)

    def apply_edge_style(self, obj, on):
        dr = obj.Attributes()
        dr.SetFaceBoundaryDraw(on)
        if on:
            dr.SetFaceBoundaryAspect(self._edge_aspect)

    # ------------------------------------------------------------ camera
    def fit_all(self, redraw=True):
        self.view.FitAll(0.04, False)
        self.view.ZFitAll()
        if redraw:
            self.redraw()

    def fit_box(self, box):
        if box is None or box.IsVoid():
            return
        self.view.FitAll(box, 0.15, False)
        self.view.ZFitAll()
        self.redraw()

    def set_orientation(self, key):
        self.view.SetProj(ORIENT[key])
        self.fit_all()

    def set_perspective(self, on):
        cam = self.view.Camera()
        cam.SetProjectionType(Graphic3d_Camera.Projection_e.Projection_Perspective if on
                              else Graphic3d_Camera.Projection_e.Projection_Orthographic)
        self.redraw()

    def _animate(self):
        if not self.cube.UpdateAnimation(True):
            self._anim.stop()
        self.redraw()                              # cube animation re-fits -> scale bar follows

    def _zoom_at(self, x, y, factor):
        cam = self.view.Camera()
        w, h = self.width() * self.dpr(), self.height() * self.dpr()
        c = cam.Center()
        zc = cam.Project(c).Z()
        p = cam.UnProject(gp_Pnt(2.0 * x / w - 1.0, 1.0 - 2.0 * y / h, zc))
        e = cam.Eye()
        k = 1.0 / factor
        nc = gp_Pnt(p.X() + (c.X() - p.X()) * k, p.Y() + (c.Y() - p.Y()) * k, p.Z() + (c.Z() - p.Z()) * k)
        if cam.ProjectionType() == Graphic3d_Camera.Projection_e.Projection_Orthographic:
            d = gp_Vec(c, nc)
            cam.SetScale(cam.Scale() * k)
            cam.SetCenter(nc)
            cam.SetEye(e.Translated(d))
        else:                                      # perspective: move the eye towards the cursor point
            cam.SetCenter(nc)
            cam.SetEye(gp_Pnt(p.X() + (e.X() - p.X()) * k, p.Y() + (e.Y() - p.Y()) * k,
                              p.Z() + (e.Z() - p.Z()) * k))
        self.view.Invalidate()
        self.redraw()

    # ------------------------------------------------------------ mouse
    def mousePressEvent(self, e):
        self.setFocus()
        self._press_pos = e.position().toPoint()
        self._press_btn = e.button()
        self._dragging = False
        x, y = self._px(self._press_pos)
        self._last = (x, y)
        ctrl = bool(e.modifiers() & Qt.ControlModifier)
        # Onshape: RMB drag = rotate about the point under the cursor, MMB / Ctrl+RMB = pan, LMB = select
        if e.button() == Qt.RightButton and not ctrl:
            self._mode = "rotate"
            self._pivot = self._pick_pivot(x, y)
        elif e.button() in (Qt.MiddleButton, Qt.RightButton):
            self._mode = "pan"
        else:
            self._mode = None

    def _pick_pivot(self, x, y):
        """Model point under the cursor, else the centre of the view."""
        self.ctx.MoveTo(x, y, self.view, False)
        p = None
        if self.ctx.HasDetected() and not isinstance(self.ctx.DetectedOwner(), AIS_ViewCubeOwner):
            p = self._picked_point()
        self.ctx.ClearDetected(False) if hasattr(self.ctx, "ClearDetected") else None
        return p if p is not None else self.view.Camera().Center()

    # ------------------------------------------------------------ 3D mouse (3Dconnexion)
    SM_ROT = math.radians(150.0)           # rad/s at full deflection, speed 1
    SM_PAN = 1100.0                        # px/s
    SM_ZOOM = 2.2                          # e-folds/s

    def space_motion(self, v, dt):
        """Object mode like the 3Dconnexion default: the model follows the cap. v = (tx, ty, tz, rx, ry, rz) in the
        screen frame (X right, Y up, Z towards the user), rotations about the pivot picked when a motion starts."""
        if self._dragging or self._window is None:
            return
        now = time.monotonic()
        if now - self._sm_last > 0.3 or self._sm_pivot is None:
            s = self.dpr()
            self._sm_pivot = self._pick_pivot(int(self.width() * s / 2), int(self.height() * s / 2))
        self._sm_last = now
        tx, ty, tz, rx, ry, rz = v
        cam = self.view.Camera()
        d, up = cam.Direction(), cam.Up()
        right = gp_Vec(d.Crossed(up))
        if rx or ry or rz:                         # model rotates by w -> camera by -w about the pivot
            w = right.Multiplied(rx).Added(gp_Vec(up).Multiplied(ry)).Added(gp_Vec(d).Multiplied(-rz))
            mag = w.Magnitude()
            if mag > 1e-12:
                t = gp_Trsf()
                t.SetRotation(gp_Ax1(self._sm_pivot, gp_Dir(w)), -mag * self.SM_ROT * dt)
                cam.Transform(t)
                d, up = cam.Direction(), cam.Up()
                right = gp_Vec(d.Crossed(up))
        if tx or ty:                               # model moves right / up -> camera the other way
            k = self.SM_PAN * self.dpr() * dt * self.world_per_pixel()
            t = gp_Trsf()
            t.SetTranslation(right.Multiplied(-tx * k).Added(gp_Vec(up).Multiplied(-ty * k)))
            cam.Transform(t)
        if tz:                                     # pulled towards the user = closer
            s = self.dpr()
            self._zoom_at(self.width() * s / 2, self.height() * s / 2, math.exp(tz * self.SM_ZOOM * dt))
            return
        self.view.Invalidate()
        self.redraw()

    def roll(self, ang):
        """Rotate the view about the viewing direction (3D mouse Roll keys)."""
        cam = self.view.Camera()
        t = gp_Trsf()
        t.SetRotation(gp_Ax1(cam.Center(), cam.Direction()), ang)
        cam.Transform(t)
        self.view.Invalidate()
        self.redraw()

    def orbit(self, ang_h, ang_v, pivot=None):
        """Rotate the camera about pivot: ang_h around the screen vertical, ang_v around the screen horizontal."""
        cam = self.view.Camera()
        pivot = pivot or cam.Center()
        d = cam.Direction()
        up = cam.Up()
        side = gp_Dir(d.Crossed(up))
        t1 = gp_Trsf()
        t1.SetRotation(gp_Ax1(pivot, up), ang_h)
        t2 = gp_Trsf()
        t2.SetRotation(gp_Ax1(pivot, side), ang_v)
        cam.Transform(t2.Multiplied(t1))
        self.view.Invalidate()
        self.redraw()

    def mouseMoveEvent(self, e):
        pos = e.position().toPoint()
        x, y = self._px(pos)
        if e.buttons() == Qt.NoButton:
            if self.measure_mode:
                self._hover_activate(x, y)
            self.ctx.MoveTo(x, y, self.view, True)
            if self.measure_mode == "pick" and self.describe is not None:
                self._hover_preview(bool(e.modifiers() & Qt.ShiftModifier))
            return
        if self._press_pos is None:
            return
        if not self._dragging:
            if (pos - self._press_pos).manhattanLength() < 4:
                return
            self._dragging = True
        lx, ly = self._last
        if self._mode == "rotate":
            k = 0.006 / self.dpr()                       # rad per physical pixel (~180° per 520 px)
            self.orbit(-(x - lx) * k, -(y - ly) * k, self._pivot)
        elif self._mode == "pan":
            self.view.Pan(x - lx, ly - y, 1.0, True)
        self._last = (x, y)

    def _hover_preview(self, shift):
        """Measure tool: one line about the entity under the cursor (computed once per entity)."""
        owner = self.ctx.DetectedOwner() if self.ctx.HasDetected() else None
        mesh_obj = owner is not None and self.ctx.DetectedInteractive() in self.meshes
        key = (id(owner), shift) if owner is not None and not mesh_obj else None
        if key is not None and key == self._hover_key:
            return
        self._hover_key = key
        it = self._detected_item(shift) if owner is not None else None
        try:
            self._show_hover(it)
            self.hoverText.emit(self.describe(it) if it is not None else "")
        except Exception:  # noqa: BLE001 – preview only
            self.hoverText.emit("")

    def _show_hover(self, it):
        """Mesh loops and flat mesh faces are no OCCT sub-shapes, so OCCT can't highlight them - draw them."""
        key = None
        if it is not None and it.kind == "loop":
            key = ("loop", id(it.loop))
        elif it is not None and it.kind == "point" and it.patch is not None:
            key = ("patch", id(it.patch[0]))
        if key == self._hover_obj_key:
            return
        self._remove_hover()
        self._hover_obj_key = key
        if key is None:
            return
        if key[0] == "loop":
            o = AIS_Shape(it.wire())
            o.SetColor(rgb(*HOVER_COLOR))
            o.SetWidth(3.0)
            o.SetZLayer(Graphic3d_ZLayerId_TopOSD)
            self.ctx.Display(o, 0, -1, False)
            self.ctx.Deactivate(o)
        else:
            o = self._patch_obj(it, HOVER_COLOR, 0.55)
        self._hover_obj = o
        self.ctx.UpdateCurrentViewer()

    def _remove_hover(self):
        if self._hover_obj is not None:
            self.ctx.Remove(self._hover_obj, False)
            self.ctx.UpdateCurrentViewer()
        self._hover_obj, self._hover_obj_key = None, None

    def _patch_obj(self, it, color, transparency):
        face = it.patch_face()
        if face is None:
            return None
        o = AIS_Shape(face)
        o.Attributes().SetAutoTriangulation(False)       # the face has no surface to re-mesh
        o.SetColor(rgb(*color))
        o.SetTransparency(transparency)
        o.SetZLayer(Graphic3d_ZLayerId_Top)
        self.ctx.Display(o, 1, -1, False)
        self.ctx.Deactivate(o)
        return o

    def keyPressEvent(self, e):
        if self.measure_mode == "pick" and e.key() in (Qt.Key_Return, Qt.Key_Enter):
            self.pin_measure()
            return
        # Onshape: arrows rotate 15°, Shift+arrows 90°
        mods = e.modifiers()
        shift = bool(mods & Qt.ShiftModifier)
        other = bool(mods & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier))
        step = math.radians(90 if shift else 15)
        keys = {Qt.Key_Left: (step, 0), Qt.Key_Right: (-step, 0), Qt.Key_Up: (0, step), Qt.Key_Down: (0, -step)}
        if e.key() in keys and not other:
            self.orbit(*keys[e.key()])
        else:
            super().keyPressEvent(e)

    def mouseReleaseEvent(self, e):
        pos = e.position().toPoint()
        x, y = self._px(pos)
        was_drag = self._dragging
        self._dragging = False
        self._press_pos = None
        if was_drag:
            return
        if e.button() == Qt.LeftButton:
            if self.measure_mode:
                self._hover_activate(x, y)
            self.ctx.MoveTo(x, y, self.view, False)
            owner = self.ctx.DetectedOwner() if self.ctx.HasDetected() else None
            if owner is not None and isinstance(owner, AIS_ViewCubeOwner):
                self.cube.HandleClick(owner)
                self._anim.start()
                return
            if self.measure_mode == "pick":       # Onshape-like: clicks accumulate, empty space clears
                self._pick_entity(add=True, shift=bool(e.modifiers() & Qt.ShiftModifier))
                return
            if self.measure_mode:
                self._measure_click()
                return
            if owner is None:
                self.ctx.ClearSelected(False)
            else:
                scheme = AIS_SelectionScheme_XOR if e.modifiers() & Qt.ControlModifier else AIS_SelectionScheme_Replace
                self.ctx.SelectDetected(scheme)
            self.ctx.UpdateCurrentViewer()
            self.selectionChanged.emit()
        elif e.button() == Qt.RightButton:
            self.contextMenuAt.emit(self.mapToGlobal(pos))

    def mouseDoubleClickEvent(self, e):
        if e.button() == Qt.LeftButton and not self.measure_mode:
            self.selectionChanged.emit()
            self.zoomSelected.emit()           # the window knows the located shapes (box incl. assembly placement)

    def wheelEvent(self, e):
        x, y = self._px(e.position().toPoint())
        d = e.angleDelta().y()
        if d == 0 or time.monotonic() - self._sm_last < 0.4:   # 3DxWare may emulate the wheel while moving
            return
        self._zoom_at(x, y, 1.2 ** (d / 120.0))

    def leaveEvent(self, e):
        if hasattr(self.ctx, "ClearDetected"):
            self.ctx.ClearDetected(True)

    # ------------------------------------------------------------ selection helpers
    def selected_objects(self):
        out = []
        self.ctx.InitSelected()
        while self.ctx.MoreSelected():
            out.append(self.ctx.SelectedInteractive())
            self.ctx.NextSelected()
        return out

    # ------------------------------------------------------------ section plane
    def set_section(self, axis, pos, flip=False, redraw=True):
        """axis: None (off) / 0,1,2. pos = coordinate along axis."""
        if axis is None:
            if self.clip is not None:
                self.view.RemoveClipPlane(self.clip)
                self.clip = None
            if redraw:
                self.redraw()
            return
        n = [0.0, 0.0, 0.0]
        n[axis] = -1.0 if not flip else 1.0
        p = [0.0, 0.0, 0.0]
        p[axis] = pos
        pln = gp_Pln(gp_Pnt(*p), gp_Dir(*n))
        if self.clip is None:
            self.clip = Graphic3d_ClipPlane(pln)
            self.clip.SetCapping(True)
            self.clip.SetUseObjectMaterial(True)
            self.clip.SetCappingHatchOff()
            self.view.AddClipPlane(self.clip)
        else:
            self.clip.SetEquation(pln)
        if redraw:
            self.redraw()

    # ------------------------------------------------------------ measuring
    def set_measure_mode(self, kind, snap_objs=()):
        """kind: None / "pick" (vertices, edges, faces) / "angle" (3 points, snaps to vertices)."""
        for o in self.feature_objs:                  # mesh sharp-edge lines are pickable only while measuring
            self.ctx.Deactivate(o)
        for o in self.meshes:
            self._mesh_hilight(o, bool(kind))
        if kind == "pick":
            for o in self.feature_objs:
                if self.ctx.IsDisplayed(o):
                    self.ctx.Activate(o, AIS_Shape.SelectionMode_s(TopAbs_EDGE))
        if hasattr(self.ctx, "SetPixelTolerance"):     # edges / vertices are easier to hit while measuring
            self.ctx.SetPixelTolerance(int(7 * self.dpr()) if kind else 3)
        self._hover_key = None
        for o in self._snap_objs:
            try:
                for m in self._snap_modes:
                    self.ctx.Deactivate(o, m)
                # activating a sub-shape mode switched the part mode 0 off (GlobalOrLocal) - without this,
                # nothing could be selected by clicking after a measurement
                if self.ctx.IsDisplayed(o):
                    self.ctx.Activate(o, 0)
            except Exception:  # noqa: BLE001
                pass
        self._snap_objs = []
        self._snap_pool = set()
        self.measure_mode = kind
        self._measure_pts = []
        self._picked = []
        self.clear_measure()
        if not kind:
            return
        self.ctx.ClearSelected(False)
        self._snap_modes = ENTITY_MODES if kind == "pick" else (VERTEX_MODE,)
        # parts get their sub-shape pick modes only when the cursor first hovers them (see _hover_activate):
        # computing vertex / edge / face sensitives for a whole big assembly up front costs seconds and RAM
        self._snap_pool = set(snap_objs)

    def _mesh_hilight(self, o, quiet):
        """A mesh is one face for OCCT: its hover highlight would flood the whole part and hide the loop /
        flat face highlight drawn by _show_hover - make it invisible while measuring."""
        try:
            if quiet:
                if self._quiet_hilight is None:
                    from OCP.Prs3d import Prs3d_Drawer
                    d = Prs3d_Drawer()
                    d.SetLink(self.ctx.HighlightStyle())
                    d.SetColor(rgb(80, 200, 255))
                    d.SetDisplayMode(1)
                    d.SetTransparency(1.0)
                    self._quiet_hilight = d
                o.SetDynamicHilightAttributes(self._quiet_hilight)
            else:
                o.UnsetHilightAttributes()
        except Exception:  # noqa: BLE001 – cosmetic only
            pass

    def add_mesh(self, obj, mesh, feat=None):
        """Register an STL mesh (+ the AIS of its sharp-edge lines); it may arrive while measuring."""
        self.meshes[obj] = mesh
        if feat is not None:
            self.feature_objs[feat] = mesh
        if self.measure_mode:
            self._mesh_hilight(obj, True)
            if feat is not None and self.measure_mode == "pick" and self.ctx.IsDisplayed(feat):
                self.ctx.Activate(feat, AIS_Shape.SelectionMode_s(TopAbs_EDGE))

    def show_features(self, on):
        """Show / hide the sharp-edge lines of meshes (pickable while measuring)."""
        for o in self.feature_objs:
            if on:
                self.ctx.Display(o, 0, -1, False)
                self.ctx.Deactivate(o)
                if self.measure_mode == "pick":
                    self.ctx.Activate(o, AIS_Shape.SelectionMode_s(TopAbs_EDGE))
            else:
                self.ctx.Erase(o, False)

    def activate_snap(self, o):
        """A presentation replaced while measuring keeps the measuring pick modes."""
        if self.measure_mode and self.ctx.IsDisplayed(o):
            for m in self._snap_modes:
                self.ctx.Activate(o, m)
            if o not in self._snap_objs:
                self._snap_objs.append(o)

    def _hover_activate(self, x, y):
        """Measuring: give the part under the cursor its vertex / edge / face picking, then re-detect."""
        self.ctx.MoveTo(x, y, self.view, False)
        if not self.ctx.HasDetected():
            return
        o = self.ctx.DetectedInteractive()
        if o is not None and o in self._snap_pool and o not in self._snap_objs:
            self.activate_snap(o)                  # the caller's next MoveTo detects the sub-shape

    def clear_measure(self):
        self._remove_hover()
        for o in self._measure_objs + self._pick_objs + self._pinned:
            self.ctx.Remove(o, False)
        self._measure_objs = []
        self._pick_objs = []
        self._pinned = []
        self._measure_pts = []
        self.ctx.UpdateCurrentViewer()

    def pin_measure(self):
        """Enter: keep the current measurement on screen and start a new one."""
        if not self._picked and not self._measure_objs:
            return
        self._pinned += self._pick_objs + self._measure_objs
        self._pick_objs, self._measure_objs = [], []
        self._picked = []
        self.entitiesPicked.emit([])
        self.ctx.UpdateCurrentViewer()

    def world_per_pixel(self):
        try:
            return self.view.Convert(int(100 * self.dpr())) / (100 * self.dpr())
        except Exception:  # noqa: BLE001
            return 0.0

    def _detected_item(self, shift=False):
        """What the cursor is on, as a PickItem: B-rep vertex / edge / face, a point on a mesh (snapped to
        the nearest mesh vertex), a sharp-edge loop of a mesh, or (Shift) the exact point on a B-rep face."""
        if not self.ctx.HasDetected():
            return None
        obj = self.ctx.DetectedInteractive()
        p = self._picked_point()
        if obj in self.feature_objs:
            mesh = self.feature_objs[obj]
            s = self._world_detected()
            mid = _shape_mid(s) if s is not None else (p if p is not None else None)
            if mid is None:
                return None
            lp = mesh.loop_near((mid.X(), mid.Y(), mid.Z()))
            return PickItem("loop", loop=lp, mesh=mesh) if lp is not None else None
        mesh = self.meshes.get(obj)
        if mesh is not None and p is not None:
            xyz = (p.X(), p.Y(), p.Z())
            tol = 7 * self.world_per_pixel()
            if not shift:                  # OCCT prefers the mesh face to the feature line drawn on it
                lp = mesh.loop_at(xyz, tol)
                if lp is not None:
                    return PickItem("loop", loop=lp, mesh=mesh)
            t = mesh.nearest_triangle(xyz)
            n = [float(c) for c in mesh.normals[t]]
            v, dist = mesh.nearest_vertex(xyz, tol)
            snapped = dist <= tol
            if snapped:
                p = gp_Pnt(*map(float, v))
            patch = mesh.flat_patch(t)
            return PickItem("point", point=p, normal=gp_Dir(*n) if any(n) else None, mesh=mesh, snapped=snapped,
                            patch=patch if patch[2] else None)
        s = self._world_detected()
        if s is None:
            return None
        if shift and p is not None:
            normal = None
            if s.ShapeType() == TopAbs_FACE and self.face_normal is not None:
                normal = self.face_normal(s, p)
            return PickItem("point", point=p, normal=normal)
        return PickItem("shape", shape=s)

    # ---- measure tool: picked entities
    def _world_detected(self):
        """Detected sub-shape in world coordinates (the owner shape is in the presentation's own frame)."""
        owner = self.ctx.DetectedOwner()
        if owner is None or not hasattr(owner, "Shape") or not owner.HasShape():
            return None
        s = owner.Shape()
        obj = self.ctx.DetectedInteractive()
        if obj is not None and obj.HasTransformation():
            s = s.Moved(TopLoc_Location(obj.Transformation()))
        return s

    def _pick_entity(self, add=True, shift=False):
        it = self._detected_item(shift)
        if it is None:
            self._picked = []
        else:
            same = [p for p in self._picked if p.same(it)]
            if same:
                self._picked = [p for p in self._picked if not p.same(it)]
            elif add:
                self._picked.append(it)
            else:
                self._picked = [it]
        self.show_picked()
        self.entitiesPicked.emit(list(self._picked))

    def clear_picked(self):
        self._picked = []
        self.show_picked()
        self.entitiesPicked.emit([])

    def show_picked(self):
        for o in self._pick_objs + self._measure_objs:
            self.ctx.Remove(o, False)
        self._pick_objs, self._measure_objs = [], []
        for it in self._picked:
            if it.kind == "point":
                self._marker(it.point, PICK_COLOR if not it.snapped else (0, 150, 255), into=self._pick_objs)
                o = self._patch_obj(it, PICK_COLOR, 0.45) if it.patch is not None else None
                if o is not None:
                    self._pick_objs.append(o)
                continue
            s = it.shape if it.kind == "shape" else it.wire()
            o = AIS_Shape(s)
            st = s.ShapeType()
            o.SetColor(rgb(*PICK_COLOR))
            if st == TopAbs_FACE:
                o.SetTransparency(0.35)
                o.SetZLayer(Graphic3d_ZLayerId_Top)
                self.ctx.Display(o, 1, -1, False)
            else:
                o.SetWidth(4.0)
                if st == TopAbs_VERTEX:
                    o.Attributes().SetPointAspect(Prs3d_PointAspect(Aspect_TOM_BALL, rgb(*PICK_COLOR), 3.0))
                o.SetZLayer(Graphic3d_ZLayerId_TopOSD)
                self.ctx.Display(o, 0, -1, False)
            self.ctx.Deactivate(o)
            self._pick_objs.append(o)
        self.ctx.UpdateCurrentViewer()

    def draw_relation(self, draw, label=None):
        """Min distance line (+ its X / Y / Z components) and centre-to-centre line."""
        if "min" in draw:
            p1, p2 = draw["min"]
            self._mline(p1, p2)
            if label:
                self._mlabel(gp_Pnt((p1.X() + p2.X()) / 2, (p1.Y() + p2.Y()) / 2, (p1.Z() + p2.Z()) / 2), label)
            a = p1
            for i, col in enumerate(AXIS_COLORS):
                c = [a.X(), a.Y(), a.Z()]
                c[i] = (p2.X(), p2.Y(), p2.Z())[i]
                b = gp_Pnt(*c)
                self._mline(a, b, col, 1.5, dashed=True)
                a = b
        if "center" in draw:
            c1, c2 = draw["center"]
            self._mline(c1, c2, (120, 120, 130), 1.5, dashed=True)
        self.ctx.UpdateCurrentViewer()

    # ---- other overlays (centre of gravity, interference volumes)
    def clear_overlay(self):
        for o in self._overlay:
            self.ctx.Remove(o, False)
        self._overlay = []
        self.ctx.UpdateCurrentViewer()

    def show_point(self, p, text, color=(0, 120, 255)):
        v = AIS_Shape(BRepBuilderAPI_MakeVertex(p).Vertex())
        v.Attributes().SetPointAspect(Prs3d_PointAspect(Aspect_TOM_BALL, rgb(*color), 4.0))
        v.SetZLayer(Graphic3d_ZLayerId_TopOSD)
        self.ctx.Display(v, 0, -1, False)
        self.ctx.Deactivate(v)
        lbl = AIS_TextLabel()
        lbl.SetText(TCollection_ExtendedString(text, True))
        lbl.SetPosition(p)
        lbl.SetColor(rgb(*color))
        lbl.SetHeight(15 * self.dpr())
        lbl.SetZLayer(Graphic3d_ZLayerId_TopOSD)
        self.ctx.Display(lbl, False)
        self.ctx.Deactivate(lbl)
        self._overlay += [v, lbl]
        self.ctx.UpdateCurrentViewer()

    def show_shapes(self, shapes, color=(230, 20, 20), transparency=0.0):
        """Analysis results (e.g. interference volumes): meshed here (auto-triangulation is off), drawn in the
        topmost layer so they stay visible inside closed housings."""
        from OCP.BRepMesh import BRepMesh_IncrementalMesh
        for s in shapes:
            BRepMesh_IncrementalMesh(s, 0.05, False, 0.3, True)
            o = AIS_Shape(s)
            o.SetColor(rgb(*color))
            if transparency:
                o.SetTransparency(transparency)
            o.SetZLayer(Graphic3d_ZLayerId_Topmost)
            self.ctx.Display(o, 1, -1, False)
            self.ctx.Deactivate(o)
            self._overlay.append(o)
        self.ctx.UpdateCurrentViewer()

    def _mline(self, a, b, color=(220, 0, 0), width=2.5, dashed=False):
        if a.Distance(b) < 1e-9:
            return
        ln = AIS_Shape(BRepBuilderAPI_MakeEdge(a, b).Edge())
        ln.SetColor(rgb(*color))
        ln.SetWidth(width)
        if dashed:
            ln.Attributes().SetWireAspect(Prs3d_LineAspect(rgb(*color), Aspect_TOL_DASH, width))
        ln.SetZLayer(Graphic3d_ZLayerId_TopOSD)
        self.ctx.Display(ln, False)
        self.ctx.Deactivate(ln)
        self._measure_objs.append(ln)

    def _mlabel(self, p, text):
        lbl = AIS_TextLabel()
        lbl.SetText(TCollection_ExtendedString(text, True))
        lbl.SetPosition(p)
        lbl.SetColor(rgb(170, 0, 0))
        lbl.SetHeight(16 * self.dpr())
        lbl.SetZLayer(Graphic3d_ZLayerId_TopOSD)
        self.ctx.Display(lbl, False)
        self.ctx.Deactivate(lbl)
        self._measure_objs.append(lbl)

    def _picked_point(self):
        sel = self.ctx.MainSelector()
        if sel.NbPicked() < 1:
            return None
        return sel.PickedPoint(1)

    def _marker(self, p, color=(220, 0, 0), into=None):
        v = AIS_Shape(BRepBuilderAPI_MakeVertex(p).Vertex())
        v.Attributes().SetPointAspect(Prs3d_PointAspect(Aspect_TOM_BALL, rgb(*color), 3.0))
        v.SetZLayer(Graphic3d_ZLayerId_TopOSD)
        self.ctx.Display(v, 0, -1, False)
        self.ctx.Deactivate(v)
        (self._measure_objs if into is None else into).append(v)

    def label_at(self, p, text):
        self._mlabel(p, text)
        self.ctx.UpdateCurrentViewer()

    def _measure_click(self):
        p = self._picked_point()
        if p is None:
            return
        mesh = self.meshes.get(self.ctx.DetectedInteractive()) if self.ctx.HasDetected() else None
        if mesh is not None:                          # snap to a mesh vertex nearby
            tol = 7 * self.world_per_pixel()
            v, dist = mesh.nearest_vertex((p.X(), p.Y(), p.Z()), tol)
            if dist <= tol:
                p = gp_Pnt(*v)
        self._measure_pts.append(p)
        self._marker(p)
        pts = self._measure_pts
        if self.measure_mode == "dist" and len(pts) == 2:
            p1, p2 = pts
            self._mline(p1, p2)
            mid = gp_Pnt((p1.X() + p2.X()) / 2, (p1.Y() + p2.Y()) / 2, (p1.Z() + p2.Z()) / 2)
            self._mlabel(mid, f"{p1.Distance(p2):.2f} mm".replace(".", ","))
            self._measure_pts = []
            self.measured.emit(p1, p2)
        elif self.measure_mode == "angle" and len(pts) == 3:
            a, v, c = pts                          # arm end, vertex, arm end
            self._mline(v, a)
            self._mline(v, c)
            va, vc = gp_Vec(v, a), gp_Vec(v, c)
            if va.Magnitude() > 1e-9 and vc.Magnitude() > 1e-9:
                ang = math.degrees(va.Angle(vc))
                self._mlabel(v, f"{ang:.2f}°".replace(".", ","))
                self.angleMeasured.emit(ang)
            self._measure_pts = []
        self.ctx.UpdateCurrentViewer()

    def screenshot(self, path):
        return self.view.Dump(path)
