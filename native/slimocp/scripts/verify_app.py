#!/usr/bin/env python3
"""Functional check of the slim OCP against the STEP viewer sources.
usage: python verify_app.py <stepview dir (a COPY of the project's stepview)> <in.step> <workdir>
Exit code 0 = everything passed. Prints which OCP is used + timings."""
import importlib, math, os, struct, sys, time, traceback

SV, STEP, WD = [os.path.abspath(a) for a in sys.argv[1:4]]
os.makedirs(WD, exist_ok=True)
sys.path.insert(0, SV)
os.chdir(SV)
results = []

def step(name):
    def deco(fn):
        t = time.time()
        try:
            info = fn()
            results.append((name, 'ok', time.time() - t, info))
        except BaseException:
            results.append((name, 'FAIL', time.time() - t, traceback.format_exc(limit=6)))
        return fn
    return deco

t0 = time.time()
import OCP
print('OCP from', OCP.__file__, 'import %.2fs' % (time.time() - t0), 'slim=%s' % getattr(OCP, '__slim__', False))

@step('import all app modules')
def _():
    os.environ.setdefault('QT_QPA_PLATFORM', 'offscreen')
    mods = ['core', 'analysis', 'materials', 'meshmeasure', 'thumbs', 'thumbsettings', 'viewer3d', 'view2d', 'spacemouse', 'collisions']
    for m in mods:
        importlib.import_module(m)
    return '%d modules' % len(mods)

import core, analysis, meshmeasure
from OCP.XCAFDoc import XCAFDoc_DocumentTool, XCAFDoc_ShapeTool
from OCP.collections import Sequence_TDF_Label
from OCP.TopExp import TopExp_Explorer
from OCP.TopAbs import TopAbs_SOLID, TopAbs_FACE, TopAbs_EDGE, TopAbs_VERTEX
from OCP.TopoDS import TopoDS
from OCP.BRep import BRep_Tool
from OCP.TopLoc import TopLoc_Location
doc = None

@step('core.read_cad (STEP + mesh)')
def _():
    global doc
    t = time.time(); doc = core.read_cad(STEP); dt = time.time() - t
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main())
    seq = Sequence_TDF_Label(); st.GetShapes(seq)
    ntri = nfaces = 0
    for i in range(1, seq.Length() + 1):
        lab = seq.Value(i)
        if XCAFDoc_ShapeTool.IsAssembly_s(lab): continue
        sh = XCAFDoc_ShapeTool.GetShape_s(lab)
        e = TopExp_Explorer(sh, TopAbs_FACE)
        while e.More():
            f = TopoDS.Face(e.Current()); e.Next(); nfaces += 1
            tri = BRep_Tool.Triangulation_s(f, TopLoc_Location())
            if tri is not None: ntri += tri.NbTriangles()
    assert ntri > 0
    return 'labels=%d faces=%d triangles=%d (%.1fs) unit=%s' % (seq.Length(), nfaces, ntri, dt, core.length_unit(doc))

@step('core.build_tree')
def _():
    root = core.build_tree(doc)
    def count(n): return 1 + sum(count(c) for c in getattr(n, 'children', []))
    return 'nodes=%s' % (count(root) if root is not None and hasattr(root, 'children') else type(root))

@step('save_xbf / load_xbf round trip')
def _():
    p = os.path.join(WD, 'rt.xbf'); core.save_xbf(doc, p); d2 = core.load_xbf(p)
    st = XCAFDoc_DocumentTool.ShapeTool_s(d2.Main()); seq = Sequence_TDF_Label(); st.GetShapes(seq)
    return 'xbf %d bytes, %d labels' % (os.path.getsize(p), seq.Length())

parts = []
@step('analysis: mass properties')
def _():
    st = XCAFDoc_DocumentTool.ShapeTool_s(doc.Main()); seq = Sequence_TDF_Label(); st.GetShapes(seq)
    tot = 0.0
    for i in range(1, seq.Length() + 1):
        lab = seq.Value(i)
        if XCAFDoc_ShapeTool.IsAssembly_s(lab): continue
        sh = XCAFDoc_ShapeTool.GetShape_s(lab)
        if sh.IsNull(): continue
        vol, area, cog, ns, nf = analysis.proto_props(sh, mesh=True)
        parts.append(sh); tot += vol
    return 'parts=%d total volume %.3f mm3' % (len(parts), tot)

@step('analysis: entities / relation / distance')
def _():
    sh = parts[0]
    ents = []
    for kind in (TopAbs_FACE, TopAbs_EDGE, TopAbs_VERTEX):
        e = TopExp_Explorer(sh, kind)
        k = 0
        while e.More() and k < 3:
            ents.append(analysis.Entity(e.Current())); e.Next(); k += 1
    rows = [r for en in ents for r in en.rows()]
    rel = analysis.relation(ents[0], ents[1]); s = analysis.summary(ents)
    from OCP.BRepExtrema import BRepExtrema_DistShapeShape
    d = BRepExtrema_DistShapeShape(parts[0], parts[min(1, len(parts) - 1)])
    d.Perform()
    return 'entities=%d rows=%d relation=%s dist=%.4f' % (len(ents), len(rows), bool(rel), d.Value())

@step('collisions: candidate_pairs + interference (BRepAlgoAPI_Common)')
def _():
    boxes = [analysis.shape_box(p) for p in parts]
    pairs = analysis.candidate_pairs(boxes)
    n = 0; kinds = {}
    for i, j in pairs[:12]:
        kind, vol, res = analysis.interference(analysis.compound(analysis.solids_of(parts[i])), analysis.compound(analysis.solids_of(parts[j])), touch=True)
        kinds[kind] = kinds.get(kind, 0) + 1; n += 1
    return 'pairs=%d tested=%d kinds=%s' % (len(pairs), n, kinds)

@step('STL: write, core.read_stl, meshmeasure')
def _():
    p = os.path.join(WD, 't.stl')
    tris = []
    for sh in parts[:5]:
        e = TopExp_Explorer(sh, TopAbs_FACE)
        while e.More():
            f = TopoDS.Face(e.Current()); e.Next()
            loc = TopLoc_Location(); tri = BRep_Tool.Triangulation_s(f, loc)
            if tri is None: continue
            tr = loc.Transformation()
            for i in range(1, tri.NbTriangles() + 1):
                a, b, c = tri.Triangle(i).Get()
                pts = []
                for k in (a, b, c):
                    q = tri.Node(k).Transformed(tr); pts.append((q.X(), q.Y(), q.Z()))
                tris.append(pts)
    with open(p, 'wb') as f:
        f.write(b'\0' * 80); f.write(struct.pack('<I', len(tris)))
        for pts in tris:
            f.write(struct.pack('<3f', 0, 0, 0))
            for q in pts: f.write(struct.pack('<3f', *q))
            f.write(b'\0\0')
    d2 = core.new_doc(); core.read_stl(p, d2)
    m = meshmeasure.mesh_for_file(p)
    m.prepare()
    import numpy as np
    q = np.array(tris[0][0], float)
    nt = m.nearest_triangle(q)
    lp = m.loops()
    rows = m.info_rows()
    return 'stl %d triangles; mesh nodes=%s tris=%s nearest=%s loops=%s' % (len(tris), len(m.nodes), len(m.tris), nt is not None, None if lp is None else len(lp))

print()
ok = True
for name, st, dt, info in results:
    print('%-52s %-4s %6.2fs  %s' % (name, st, dt, info if st == 'ok' else ''))
    if st != 'ok': ok = False; print(info)
print('TOTAL %.1fs' % (time.time() - t0))
sys.exit(0 if ok else 1)
