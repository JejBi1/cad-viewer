#!/bin/bash
# usage: import_time.sh <python> [PYTHONPATH]   -> median of 7 runs of `import OCP` + the 44 module imports of the app (seconds)
PY=$1; export PYTHONPATH=$2
cat > /tmp/imp_t.py <<'PYX'
import time, sys
t = time.perf_counter()
import OCP
t1 = time.perf_counter() - t
import importlib, json
mods = "AIS Aspect BRep BRepAdaptor BRepAlgoAPI BRepBndLib BRepBuilderAPI BRepExtrema BRepGProp BRepMesh BRepTools BinXCAFDrivers Bnd GCPnts GProp GeomAPI GeomAbs GeomLProp Graphic3d IFSelect IGESCAFControl IMeshTools Image OpenGl Prs3d Quantity RWStl STEPCAFControl TCollection TDF TDataStd TDocStd TopAbs TopExp TopLoc TopoDS V3d XCAFApp XCAFDoc XCAFPrs collections gp".split()
mods.append('WNT' if sys.platform == 'win32' else 'Xw')
for m in mods: importlib.import_module('OCP.' + m)
print('%.3f %.3f' % (t1, time.perf_counter() - t))
PYX
for i in 1 2 3 4 5 6 7; do $PY /tmp/imp_t.py; done | sort -n | sed -n 4p
