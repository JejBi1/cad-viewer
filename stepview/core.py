"""Loading STEP/IGES into an XCAF document, meshing, XBF hand-over and tree extraction."""
import math
import os

from OCP.BinXCAFDrivers import (BinXCAFDrivers_DocumentRetrievalDriver,
                                BinXCAFDrivers_DocumentStorageDriver)
from OCP.Bnd import Bnd_Box
from OCP.BRep import BRep_Builder
from OCP.BRepBndLib import BRepBndLib
from OCP.BRepMesh import BRepMesh_IncrementalMesh
from OCP.BRepTools import BRepTools
from OCP.collections import (DataMap_TCollection_AsciiString_TCollection_AsciiString as _StrMap,
                             IndexedMap_TopoDS_Shape_TopTools_ShapeMapHasher as _ShapeMap, Sequence_TDF_Label)
from OCP.IFSelect import IFSelect_RetDone
from OCP.IGESCAFControl import IGESCAFControl_Reader
from OCP.IMeshTools import IMeshTools_MeshAlgoType, IMeshTools_Parameters
from OCP.Quantity import Quantity_Color
from OCP.STEPCAFControl import STEPCAFControl_Reader
from OCP.TCollection import TCollection_AsciiString, TCollection_ExtendedString
from OCP.TDataStd import TDataStd_Name
from OCP.TDF import TDF_Label, TDF_Tool
from OCP.TDocStd import TDocStd_Document
from OCP.BRep import BRep_Tool
from OCP.RWStl import RWStl
from OCP.TopAbs import TopAbs_FACE
from OCP.TopExp import TopExp, TopExp_Explorer
from OCP.TopLoc import TopLoc_Location
from OCP.TopoDS import TopoDS, TopoDS_Compound, TopoDS_Face
from OCP.XCAFApp import XCAFApp_Application
from OCP.XCAFDoc import (XCAFDoc_ColorGen, XCAFDoc_ColorSurf, XCAFDoc_ColorTool,
                         XCAFDoc_DocumentTool, XCAFDoc_ShapeTool)

FORMAT = "BinXCAF"
CAD_EXT = (".step", ".stp", ".iges", ".igs", ".stl")

_app = None
_retr = None


def application():
    global _app, _retr
    if _app is None:
        _app = XCAFApp_Application.GetApplication_s()
        store = BinXCAFDrivers_DocumentStorageDriver()
        store.SetWithTriangles(_app.MessageDriver(), True)
        _retr = BinXCAFDrivers_DocumentRetrievalDriver()
        _app.DefineFormat(TCollection_AsciiString(FORMAT), TCollection_AsciiString("Binary XCAF"),
                          TCollection_AsciiString("xbf"), _retr, store)
    return _app


def new_doc():
    doc = TDocStd_Document(TCollection_ExtendedString(FORMAT))
    application().InitDocument(doc)
    return doc


def box_limits(box):
    a, b = box.CornerMin(), box.CornerMax()
    return a.X(), a.Y(), a.Z(), b.X(), b.Y(), b.Z()


# ---------------------------------------------------------------- loading

# STEPControl_Reader.Transfer runs ShapeFix on every shape ("FixShape"). Two of its repairs are O(n^2) in the number
# of wires of a face (a plate with ~1300 holes spends 20 of its 28 s there) and only matter for output of broken
# exporters, so they are switched off for STEP. Everything else (orientation, seams, gaps, missing edges ...) stays on;
# FixFaceMode must NOT be switched off (it produces invalid geometry). STEPVIEW_FULL_FIX=1 restores the full repair.
FAST_FIX_PARAMS = {"FixShape.FixSelfIntersectionMode": "0", "FixShape.FixIntersectingWiresMode": "0"}


def relax_shapefix(reader):
    """Disable the O(n^2) ShapeFix repairs on a STEP reader. Only works AFTER ReadFile (the transfer actor, which
    owns the parameters, does not exist before it) and before Transfer."""
    m = _StrMap()
    for k, v in FAST_FIX_PARAMS.items():
        m.Bind(TCollection_AsciiString(k), TCollection_AsciiString(v))
    reader.SetShapeFixParameters(m)


def read_cad(path, report=lambda msg: None):
    """Read STEP/IGES into a new XCAF document and mesh it. Returns the document."""
    doc = new_doc()
    ext = os.path.splitext(path)[1].lower()
    if ext == ".stl":
        return read_stl(path, doc, report)
    if ext in (".iges", ".igs"):
        r = IGESCAFControl_Reader()
        r.SetColorMode(True)
        r.SetNameMode(True)
        r.SetLayerMode(False)
    else:
        r = STEPCAFControl_Reader()
        r.SetColorMode(True)
        r.SetNameMode(True)
        r.SetLayerMode(False)
        r.SetPropsMode(False)
        r.SetGDTMode(False)
        r.SetMatMode(False)
        r.SetViewMode(False)
    report("Čtu soubor…")
    if r.ReadFile(path) != IFSelect_RetDone:
        raise RuntimeError("Soubor se nepodařilo přečíst (neplatný nebo poškozený STEP/IGES).")
    if ext not in (".iges", ".igs") and os.environ.get("STEPVIEW_FULL_FIX") != "1":
        try:
            relax_shapefix(r)
        except Exception:  # noqa: BLE001 – just the slower full repair
            pass
    report("Převádím geometrii…")
    if not r.Transfer(doc):
        raise RuntimeError("Převod geometrie selhal.")
    report("Kontroluji strukturu sestavy…")
    count_instances(doc)               # instance bomb / cyclic assembly: fail before meshing (also in thumbnails)
    mesh_doc(doc, report)
    return doc


def read_stl(path, doc, report=lambda msg: None):
    """STL -> one mesh-only face in the XCAF doc (sharp edges kept: nodes merged below 30°)."""
    report("Čtu STL…")
    tri = RWStl.ReadFile_s(path, math.radians(30.0))
    if tri is None or tri.NbTriangles() == 0:
        raise RuntimeError("STL soubor se nepodařilo přečíst nebo je prázdný.")
    face = TopoDS_Face()
    BRep_Builder().MakeFace(face, tri)
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    lab = st.AddShape(face, False)
    TDataStd_Name.Set_s(lab, TCollection_ExtendedString(os.path.splitext(os.path.basename(path))[0], True))
    return doc


def is_mesh_only(shape):
    """True for STL-like shapes (faces without a surface, triangulation only)."""
    e = TopExp_Explorer(shape, TopAbs_FACE)
    return e.More() and BRep_Tool.Surface_s(TopoDS.Face(e.Current())) is None


# Models with more faces than this are meshed with the Delabella algorithm (plates with hundreds of holes: ~6x faster
# than the default Delaunay; same deflection / angle). Small models keep the default mesher - no gain there.
DELABELLA_MIN_FACES = 2000


def face_count(shapes):
    m = _ShapeMap()
    for s in shapes:
        TopExp.MapShapes_s(s, TopAbs_FACE, m)
    return m.Extent()


def _mesh_params(defl, delabella):
    p = IMeshTools_Parameters()
    p.Deflection = defl
    p.Angle = 0.35
    p.Relative = False
    p.InParallel = True
    if delabella:
        p.MeshAlgo = IMeshTools_MeshAlgoType.IMeshTools_MeshAlgoType_Delabella
    return p


def _unmeshed_faces(shape):
    """Faces of `shape` that have no triangulation."""
    out = []
    e = TopExp_Explorer(shape, TopAbs_FACE)
    while e.More():
        f = TopoDS.Face(e.Current())
        e.Next()
        t = BRep_Tool.Triangulation_s(f, TopLoc_Location())
        if t is None or t.NbTriangles() == 0:
            out.append(f)
    return out


def mesh_doc(doc, report=lambda msg: None):
    """Mesh every non-assembly shape once; deflection scales with part size."""
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    seq = Sequence_TDF_Label()
    st.GetShapes(seq)
    buckets = {}
    for i in range(1, seq.Length() + 1):
        lab = seq.Value(i)
        if XCAFDoc_ShapeTool.IsAssembly_s(lab):
            continue
        shape = XCAFDoc_ShapeTool.GetShape_s(lab)
        if shape.IsNull() or is_mesh_only(shape):
            continue
        box = Bnd_Box()
        BRepBndLib.Add_s(shape, box, False)
        if box.IsVoid():
            continue
        x0, y0, z0, x1, y1, z1 = box_limits(box)
        size = max(x1 - x0, y1 - y0, z1 - z0)
        defl = min(max(size * 0.001, 0.002), 0.5)
        key = round(math.log2(defl) * 2)  # half-octave buckets
        buckets.setdefault(key, []).append(shape)
    n = sum(len(v) for v in buckets.values())
    report(f"Síťuji {n} těles…")
    fast = (os.environ.get("STEPVIEW_NO_DELABELLA") != "1"
            and face_count([s for v in buckets.values() for s in v]) > DELABELLA_MIN_FACES)
    b = BRep_Builder()
    for key, shapes in buckets.items():
        comp = TopoDS_Compound()
        b.MakeCompound(comp)
        for s in shapes:
            b.Add(comp, s)
        defl = 2.0 ** (key / 2.0)
        if not fast:
            BRepMesh_IncrementalMesh(comp, defl, False, 0.35, True)
            continue
        BRepMesh_IncrementalMesh(comp, _mesh_params(defl, True))
        # Delabella must not leave a face without triangulation (it would be missing in the picture): cheap check
        # of the whole compound first, then the faces it skipped get the default mesher
        if not BRepTools.Triangulation_s(comp, defl * 1.001, False):
            missing = []
            for s in shapes:
                missing += _unmeshed_faces(s)
            if missing:
                redo = TopoDS_Compound()
                b.MakeCompound(redo)
                for f in missing:
                    b.Add(redo, f)
                BRepMesh_IncrementalMesh(redo, _mesh_params(defl, False))


def save_xbf(doc, path):
    st = application().SaveAs(doc, TCollection_ExtendedString(path))
    if "OK" not in str(st):
        raise RuntimeError(f"Uložení mezivýsledku selhalo: {st}")


def load_xbf(path):
    application()
    doc = TDocStd_Document(TCollection_ExtendedString(FORMAT))
    _retr.Read(TCollection_ExtendedString(path), doc, _app)
    if "OK" not in str(_retr.GetStatus()):
        raise RuntimeError(f"Načtení mezivýsledku selhalo: {_retr.GetStatus()}")
    return doc


def length_unit(doc):
    """Document length unit in mm (1.0 = mm), or None."""
    try:
        ok, val = XCAFDoc_DocumentTool.GetLengthUnit_s(doc, 0.0)
        return val * 1000.0 if ok else None
    except Exception:
        return None


# ---------------------------------------------------------------- tree

def label_name(lab):
    attr = TDataStd_Name()
    if lab.FindAttribute(TDataStd_Name.GetID_s(), attr):
        return attr.Get().ToExtString()
    return ""


def label_color(ctool, lab):
    c = Quantity_Color()
    for t in (XCAFDoc_ColorSurf, XCAFDoc_ColorGen):
        if XCAFDoc_ColorTool.GetColor_s(lab, t, c):
            return c
    return None


def entry(lab):
    s = TCollection_AsciiString()
    TDF_Tool.Entry_s(lab, s)
    return s.ToCString()


class Node:
    __slots__ = ("idx", "parent", "label", "proto", "key", "name", "proto_name", "is_asm",
                 "children", "loc", "parent_loc", "color", "own_style", "ais", "item")

    def __init__(self):
        self.children = []
        self.ais = None
        self.color = None
        self.own_style = False      # instance (reference label) carries its own colour
        self.item = None


def _is_auto_name(name):
    return not name or name.startswith("=>[")


# An assembly is a DAG of shared prototypes: a 50 KB file with 10 components per level and 6 levels expands to
# a million instances (build_tree needs minutes and GBs). The instance count is known from the DAG alone.
MAX_INSTANCES = 300_000
# The absolute limit alone lets a 50 KB file with 5 levels x 10 components (111 000 nodes, 15 s in build_tree, much
# more to display) through. Real models re-use parts, but every instance still has its own record in the file:
# many more instances than records (labels + component references) means nested re-use, i.e. a bomb.
AMPLIFY_MIN = 50_000
AMPLIFY_RATIO = 100


class ModelTooComplex(RuntimeError):
    """The assembly would expand to more instances than we are willing to build (or is cyclic)."""


def count_instances(doc, limit=MAX_INSTANCES):
    """Number of tree nodes build_tree would create (assemblies + parts, every instance counted), computed
    over the label DAG with one memoised visit per prototype - cost ~ number of distinct prototypes and
    component references, not of instances. Raises ModelTooComplex above `limit` or on a cyclic assembly."""
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    free = Sequence_TDF_Label()
    st.GetFreeShapes(free)
    memo = {}                      # prototype entry -> nodes in its subtree (itself included)
    active = set()                 # prototypes on the current path (cycle detection)
    edges = [0]                    # component references seen (each assembly is walked once)

    def proto_of(lab):
        if XCAFDoc_ShapeTool.IsReference_s(lab):
            p = TDF_Label()
            XCAFDoc_ShapeTool.GetReferredShape_s(lab, p)
            return p
        return lab

    def subtree(proto, depth):
        if not XCAFDoc_ShapeTool.IsAssembly_s(proto):
            return 1                   # a part: nothing below it, no key / memo needed (cycles run through assemblies)
        key = entry(proto)
        n = memo.get(key)
        if n is not None:
            return n
        if key in active:
            raise ModelTooComplex("Sestava obsahuje sama sebe (cyklický odkaz mezi díly) – nelze zobrazit.")
        if depth > 200:
            raise ModelTooComplex("Sestava je vnořená příliš hluboko (více než 200 úrovní) – nelze zobrazit.")
        n = 1
        active.add(key)
        try:
            comps = Sequence_TDF_Label()
            XCAFDoc_ShapeTool.GetComponents_s(proto, comps, False)
            edges[0] += comps.Length()
            for i in range(1, comps.Length() + 1):
                n += subtree(proto_of(comps.Value(i)), depth + 1)
                if n > limit:
                    break
        finally:
            active.discard(key)
        memo[key] = n
        return n

    total = 0
    for i in range(1, free.Length() + 1):
        total += subtree(proto_of(free.Value(i)), 0)
        if total > limit:
            break
    if total > limit:
        lim = f"{limit:,}".replace(",", " ")
        raise ModelTooComplex(
            f"Sestava by se rozvinula na více než {lim} instancí dílů a sestav – "
            "to je příliš na zobrazení (pravděpodobně vnořené sestavy, které se opakovaně odkazují na sebe). "
            "Soubor se neotevře.")
    stored = len(memo) + edges[0] + free.Length()     # assemblies + component references + top-level shapes
    if total > AMPLIFY_MIN and total > AMPLIFY_RATIO * stored:
        raise ModelTooComplex(
            f"Sestava má v souboru jen {stored} záznamů, ale vnořením sestav by se rozvinula na {total} "
            f"instancí ({total // max(1, stored)}× víc) – to vypadá jako záměrně vytvořená „bomba“, "
            "zobrazení by trvalo minuty. Soubor se neotevře.")
    return total


def build_tree(doc, check=True):
    """Return (roots, nodes, proto_count). Leaves carry parent_loc for display. check=False only for callers that
    have just run count_instances(doc) themselves."""
    if check:
        count_instances(doc)       # refuse instance bombs / cyclic assemblies BEFORE expanding the DAG
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    ctool = XCAFDoc_DocumentTool.ColorTool_s(doc.Main())
    free = Sequence_TDF_Label()
    st.GetFreeShapes(free)
    nodes = []
    proto_count = {}

    def build(lab, parent, parent_loc):
        n = Node()
        n.idx = len(nodes)
        nodes.append(n)
        n.parent = parent
        n.label = lab
        n.parent_loc = parent_loc
        is_ref = XCAFDoc_ShapeTool.IsReference_s(lab)
        if is_ref:
            proto = TDF_Label()
            XCAFDoc_ShapeTool.GetReferredShape_s(lab, proto)
            loc = parent_loc.Multiplied(XCAFDoc_ShapeTool.GetLocation_s(lab))
        else:
            proto = lab
            loc = parent_loc
        n.proto = proto
        n.loc = loc
        n.proto_name = label_name(proto)
        inst = label_name(lab) if is_ref else ""
        n.name = n.proto_name if _is_auto_name(inst) else inst
        if not n.name:
            n.name = "(bez názvu)"
        inst_color = label_color(ctool, lab) if is_ref else None
        n.own_style = inst_color is not None
        n.color = inst_color or label_color(ctool, proto)
        n.is_asm = XCAFDoc_ShapeTool.IsAssembly_s(proto)
        n.key = key = entry(proto)
        if n.is_asm:
            comps = Sequence_TDF_Label()
            XCAFDoc_ShapeTool.GetComponents_s(proto, comps, False)
            for i in range(1, comps.Length() + 1):
                n.children.append(build(comps.Value(i), n, loc))
        else:
            proto_count[key] = proto_count.get(key, 0) + 1
        return n

    roots = [build(free.Value(i), None, TopLoc_Location()) for i in range(1, free.Length() + 1)]
    return roots, nodes, proto_count
