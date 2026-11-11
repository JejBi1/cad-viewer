s = open('build_orig.py', encoding='utf8').read()
# ---- the actual integration diff -------------------------------------------------------------------------
s = s.replace('SP = os.path.join(sys.prefix, "Lib", "site-packages")', 'SP = os.environ.get("SLIM_OCP_DIR", os.path.join(sys.prefix, "Lib", "site-packages"))   # folder holding OCP and OCP.libs')
s = s.replace('LIB_DIRS = ["cadquery_ocp.libs", "vtk.libs"]   # OCP/__init__ adds both as DLL directories', 'LIB_DIRS = ["OCP.libs"]   # slim OCP: OCP/__init__ adds OCP.libs as DLL directory (no vtk.libs any more)')
# ---- test harness only: separate work dir so the real release build is untouched ---------------------------
s = s.replace('BUILD = os.path.join(os.path.expanduser("~"), ".venvs", "stepviewer-build")', 'BUILD = r"C:\slimocp_w\integ\build"')
open('build_slim_test.py', 'w', encoding='utf8').write(s)
