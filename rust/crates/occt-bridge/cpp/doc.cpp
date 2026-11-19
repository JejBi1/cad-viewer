// Document handling: application / XBF format, STEP / IGES / STL import, meshing, instance count, tree.
// Behaviour reference: stepview/core.py. Policy (constants, formulas, Czech texts) stays on the Rust side.
#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <setjmp.h>
#include <signal.h>
#endif

#include "bridge.h"
#include "common.h"
#include "occt-bridge/src/ffi.rs.h"

#include <algorithm>
#include <cmath>
#include <mutex>
#include <string>
#include <unordered_set>

#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <BRepBndLib.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepTools.hxx>
#include <BinXCAFDrivers_DocumentRetrievalDriver.hxx>
#include <BinXCAFDrivers_DocumentStorageDriver.hxx>
#include <Bnd_Box.hxx>
#include <IFSelect_ReturnStatus.hxx>
#include <IMeshTools_MeshAlgoType.hxx>
#include <IMeshTools_Parameters.hxx>
#include <NCollection_Sequence.hxx>
#include <PCDM_ReaderStatus.hxx>
#include <PCDM_StoreStatus.hxx>
#include <Poly_Triangulation.hxx>
#include <Quantity_Color.hxx>
#include <RWStl.hxx>
#include <Standard_Failure.hxx>
#include <TCollection_AsciiString.hxx>
#include <TCollection_ExtendedString.hxx>
#include <TDF_Tool.hxx>
#include <TDataStd_Name.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Face.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>
#include <XSAlgo_ShapeProcessor.hxx>
#include <gp_Trsf.hxx>

namespace stepview {

// ------------------------------------------------------------------ errors and small helpers

#ifdef _WIN32
namespace {
// Only crashes of the kind a corrupt input causes; C++ exceptions and everything else keep propagating.
int hardware_filter(unsigned long code) {
  switch (code) {
    case EXCEPTION_ACCESS_VIOLATION:
    case EXCEPTION_IN_PAGE_ERROR:
    case EXCEPTION_ILLEGAL_INSTRUCTION:
    case EXCEPTION_PRIV_INSTRUCTION:
    case EXCEPTION_INT_DIVIDE_BY_ZERO:
    case EXCEPTION_ARRAY_BOUNDS_EXCEEDED:
    case EXCEPTION_DATATYPE_MISALIGNMENT:
      return EXCEPTION_EXECUTE_HANDLER;
    default:
      return EXCEPTION_CONTINUE_SEARCH;
  }
}
}  // namespace

unsigned long run_guarded(void (*fn)(void*), void* arg) {
  __try {
    fn(arg);
    return 0;
  } __except (hardware_filter(GetExceptionCode())) {
    return GetExceptionCode();
  }
}
#else
namespace {
__thread sigjmp_buf* g_guard_jmp = nullptr;
std::once_flag g_sig_once;
constexpr int kGuardSignals[] = {SIGSEGV, SIGBUS, SIGFPE, SIGILL};
struct sigaction g_prev_act[4]{};

const struct sigaction* prev_action_for(int sig) {
  for (size_t i = 0; i < 4; ++i) {
    if (kGuardSignals[i] == sig) return &g_prev_act[i];
  }
  return nullptr;
}

void guard_signal_handler(int sig, siginfo_t* info, void* uctx) {
  sigjmp_buf* target = g_guard_jmp;
  if (target != nullptr) {
    g_guard_jmp = nullptr;
    siglongjmp(*target, sig);
  }
  if (const struct sigaction* prev = prev_action_for(sig)) {
    if ((prev->sa_flags & SA_SIGINFO) != 0 && prev->sa_sigaction != nullptr) {
      prev->sa_sigaction(sig, info, uctx);
      return;
    }
    if (prev->sa_handler == SIG_IGN) return;
    if (prev->sa_handler != SIG_DFL && prev->sa_handler != nullptr) {
      prev->sa_handler(sig);
      return;
    }
  }
  struct sigaction dfl{};
  dfl.sa_handler = SIG_DFL;
  sigemptyset(&dfl.sa_mask);
  sigaction(sig, &dfl, nullptr);
  if (info != nullptr && info->si_code <= 0) {
    raise(sig);
  }
}

void install_signal_handlers() {
  struct sigaction sa{};
  sa.sa_sigaction = guard_signal_handler;
  sigemptyset(&sa.sa_mask);
  sa.sa_flags = SA_SIGINFO | SA_ONSTACK;
  for (size_t i = 0; i < 4; ++i) {
    sigaction(kGuardSignals[i], &sa, &g_prev_act[i]);
  }
}

struct JmpGuard {
  sigjmp_buf* prev;
  sigjmp_buf buf;
  JmpGuard() : prev(g_guard_jmp) { g_guard_jmp = &buf; }
  ~JmpGuard() { g_guard_jmp = prev; }
};
}  // namespace

unsigned long run_guarded(void (*fn)(void*), void* arg) {
  std::call_once(g_sig_once, install_signal_handlers);
  JmpGuard jg;
  int sig = sigsetjmp(jg.buf, 1);
  if (sig != 0) {
    return static_cast<unsigned long>(sig);
  }
  fn(arg);
  return 0;
}
#endif

namespace {
// Per thread: a test that injects a fault must not hit the calls that other tests make at the same time.
thread_local uint32_t g_fault = 0;
}  // namespace

void test_inject_fault(uint32_t id) { g_fault = id; }

void fault_point(uint32_t id) {
  if (id == 0 || g_fault != id) return;
  g_fault = 0;  // once
#ifdef _WIN32
  RaiseException(EXCEPTION_ACCESS_VIOLATION, 0, 0, nullptr);
#else
  raise(SIGSEGV);
#endif
}

void rethrow_as_runtime() {
  try {
    throw;
  } catch (const Fail&) {
    throw;
  } catch (const Standard_Failure& e) {
    std::string msg = "Chyba geometrického jádra (OCCT, ";
    msg += e.ExceptionType();
    msg += ")";
    const char* what = e.what();
    if (what != nullptr && *what != '\0') {
      msg += ": ";
      msg += what;
    }
    throw std::runtime_error(msg);
  } catch (const std::bad_alloc&) {
    throw std::runtime_error("Nedostatek paměti.");
  } catch (const std::exception& e) {
    throw std::runtime_error(std::string("Chyba: ") + e.what());
  } catch (...) {
    throw std::runtime_error("Neznámá chyba geometrického jádra.");
  }
}

std::string utf16_to_utf8(const char16_t* s) {
  std::string out;
  if (s == nullptr) return out;
  for (; *s != 0; ++s) {
    uint32_t c = *s;
    if (c >= 0xD800 && c <= 0xDBFF && s[1] >= 0xDC00 && s[1] <= 0xDFFF) {
      c = 0x10000 + ((c - 0xD800) << 10) + (static_cast<uint32_t>(s[1]) - 0xDC00);
      ++s;
    } else if (c >= 0xD800 && c <= 0xDFFF) {
      c = 0xFFFD;
    }
    if (c < 0x80) {
      out += static_cast<char>(c);
    } else if (c < 0x800) {
      out += static_cast<char>(0xC0 | (c >> 6));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else if (c < 0x10000) {
      out += static_cast<char>(0xE0 | (c >> 12));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    } else {
      out += static_cast<char>(0xF0 | (c >> 18));
      out += static_cast<char>(0x80 | ((c >> 12) & 0x3F));
      out += static_cast<char>(0x80 | ((c >> 6) & 0x3F));
      out += static_cast<char>(0x80 | (c & 0x3F));
    }
  }
  return out;
}

std::string label_name(const TDF_Label& lab) {
  occ::handle<TDataStd_Name> attr;
  if (lab.FindAttribute(TDataStd_Name::GetID(), attr) && !attr.IsNull()) {
    return utf16_to_utf8(attr->Get().ToExtString());
  }
  return std::string();
}

bool label_color(const TDF_Label& lab, Quantity_Color& out) {
  const XCAFDoc_ColorType types[2] = {XCAFDoc_ColorSurf, XCAFDoc_ColorGen};
  for (XCAFDoc_ColorType t : types) {
    if (XCAFDoc_ColorTool::GetColor(lab, t, out)) return true;
  }
  return false;
}

std::string entry_of(const TDF_Label& lab) {
  TCollection_AsciiString s;
  TDF_Tool::Entry(lab, s);
  return std::string(s.ToCString());
}

bool shape_is_mesh_only(const TopoDS_Shape& shape) {
  if (shape.IsNull()) return false;
  TopExp_Explorer e(shape, TopAbs_FACE);
  if (!e.More()) return false;
  return BRep_Tool::Surface(TopoDS::Face(e.Current())).IsNull();
}

// ------------------------------------------------------------------ application (core.py: application)

namespace {

std::mutex g_io;  // serialises document creation, STEP / IGES reading and XBF storage / retrieval

struct AppState {
  occ::handle<XCAFApp_Application> app;
  occ::handle<BinXCAFDrivers_DocumentRetrievalDriver> retr;
};

// Defines format "BinXCAF" (extension xbf) with a storage driver that keeps the triangulation.
AppState& app_state() {
  static AppState st = [] {
    AppState s;
    s.app = XCAFApp_Application::GetApplication();
    occ::handle<BinXCAFDrivers_DocumentStorageDriver> store = new BinXCAFDrivers_DocumentStorageDriver();
    store->SetWithTriangles(s.app->MessageDriver(), true);
    s.retr = new BinXCAFDrivers_DocumentRetrievalDriver();
    s.app->DefineFormat(TCollection_AsciiString("BinXCAF"), TCollection_AsciiString("Binary XCAF"),
                        TCollection_AsciiString("xbf"), s.retr, store);
    return s;
  }();
  return st;
}

std::unique_ptr<Doc> make_doc_locked() {
  auto d = std::make_unique<Doc>();
  d->doc = new TDocStd_Document(TCollection_ExtendedString("BinXCAF"));
  app_state().app->InitDocument(d->doc);
  return d;
}

const char* store_status_name(PCDM_StoreStatus s) {
  switch (s) {
    case PCDM_SS_OK: return "PCDM_StoreStatus.PCDM_SS_OK";
    case PCDM_SS_DriverFailure: return "PCDM_StoreStatus.PCDM_SS_DriverFailure";
    case PCDM_SS_WriteFailure: return "PCDM_StoreStatus.PCDM_SS_WriteFailure";
    case PCDM_SS_Failure: return "PCDM_StoreStatus.PCDM_SS_Failure";
    case PCDM_SS_Doc_IsNull: return "PCDM_StoreStatus.PCDM_SS_Doc_IsNull";
    case PCDM_SS_No_Obj: return "PCDM_StoreStatus.PCDM_SS_No_Obj";
    case PCDM_SS_Info_Section_Error: return "PCDM_StoreStatus.PCDM_SS_Info_Section_Error";
    case PCDM_SS_UserBreak: return "PCDM_StoreStatus.PCDM_SS_UserBreak";
    case PCDM_SS_UnrecognizedFormat: return "PCDM_StoreStatus.PCDM_SS_UnrecognizedFormat";
  }
  return "PCDM_StoreStatus.?";
}

const char* reader_status_name(PCDM_ReaderStatus s) {
  switch (s) {
    case PCDM_RS_OK: return "PCDM_ReaderStatus.PCDM_RS_OK";
    case PCDM_RS_NoDriver: return "PCDM_ReaderStatus.PCDM_RS_NoDriver";
    case PCDM_RS_UnknownFileDriver: return "PCDM_ReaderStatus.PCDM_RS_UnknownFileDriver";
    case PCDM_RS_OpenError: return "PCDM_ReaderStatus.PCDM_RS_OpenError";
    case PCDM_RS_NoVersion: return "PCDM_ReaderStatus.PCDM_RS_NoVersion";
    case PCDM_RS_NoSchema: return "PCDM_ReaderStatus.PCDM_RS_NoSchema";
    case PCDM_RS_NoDocument: return "PCDM_ReaderStatus.PCDM_RS_NoDocument";
    case PCDM_RS_ExtensionFailure: return "PCDM_ReaderStatus.PCDM_RS_ExtensionFailure";
    case PCDM_RS_WrongStreamMode: return "PCDM_ReaderStatus.PCDM_RS_WrongStreamMode";
    case PCDM_RS_FormatFailure: return "PCDM_ReaderStatus.PCDM_RS_FormatFailure";
    case PCDM_RS_TypeFailure: return "PCDM_ReaderStatus.PCDM_RS_TypeFailure";
    case PCDM_RS_TypeNotFoundInSchema: return "PCDM_ReaderStatus.PCDM_RS_TypeNotFoundInSchema";
    case PCDM_RS_UnrecognizedFileFormat: return "PCDM_ReaderStatus.PCDM_RS_UnrecognizedFileFormat";
    case PCDM_RS_MakeFailure: return "PCDM_ReaderStatus.PCDM_RS_MakeFailure";
    case PCDM_RS_PermissionDenied: return "PCDM_ReaderStatus.PCDM_RS_PermissionDenied";
    case PCDM_RS_DriverFailure: return "PCDM_ReaderStatus.PCDM_RS_DriverFailure";
    case PCDM_RS_AlreadyRetrievedAndModified: return "PCDM_ReaderStatus.PCDM_RS_AlreadyRetrievedAndModified";
    case PCDM_RS_AlreadyRetrieved: return "PCDM_ReaderStatus.PCDM_RS_AlreadyRetrieved";
    case PCDM_RS_UnknownDocument: return "PCDM_ReaderStatus.PCDM_RS_UnknownDocument";
    case PCDM_RS_WrongResource: return "PCDM_ReaderStatus.PCDM_RS_WrongResource";
    case PCDM_RS_ReaderException: return "PCDM_ReaderStatus.PCDM_RS_ReaderException";
    case PCDM_RS_NoModel: return "PCDM_ReaderStatus.PCDM_RS_NoModel";
    case PCDM_RS_UserBreak: return "PCDM_ReaderStatus.PCDM_RS_UserBreak";
  }
  return "PCDM_ReaderStatus.?";
}

TDF_Label proto_of(const TDF_Label& lab) {
  if (XCAFDoc_ShapeTool::IsReference(lab)) {
    TDF_Label p;
    if (XCAFDoc_ShapeTool::GetReferredShape(lab, p)) return p;
  }
  return lab;
}

}  // namespace

// ------------------------------------------------------------------ Doc members

void Doc::ensure_protos() const {
  if (protos_ready) return;
  protos_ready = true;
  occ::handle<XCAFDoc_ShapeTool> st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  NCollection_Sequence<TDF_Label> seq;
  st->GetShapes(seq);
  for (int i = 1; i <= seq.Length(); ++i) {
    const TDF_Label& lab = seq.Value(i);
    if (XCAFDoc_ShapeTool::IsAssembly(lab)) continue;
    proto_for_label(lab);
  }
}

uint32_t Doc::proto_for_label(const TDF_Label& lab) const {
  std::string key = entry_of(lab);
  auto it = proto_by_key.find(key);
  if (it != proto_by_key.end()) return it->second;
  auto p = std::make_unique<Proto>();
  p->label = lab;
  p->key = key;
  protos.push_back(std::move(p));
  uint32_t id = static_cast<uint32_t>(protos.size() - 1);
  proto_by_key.emplace(std::move(key), id);
  return id;
}

Proto& Doc::proto(uint32_t id) const {
  ensure_protos();
  if (id >= protos.size()) throw Fail("Neplatný identifikátor dílu.");
  return *protos[id];
}

const TopoDS_Shape& Doc::proto_shape(Proto& p) const {
  if (!p.shape_loaded) {
    p.shape = XCAFDoc_ShapeTool::GetShape(p.label);
    p.mesh_only = !p.shape.IsNull() && shape_is_mesh_only(p.shape);
    p.shape_loaded = true;
  }
  return p.shape;
}

void Doc::ensure_maps(Proto& p) const {
  if (p.maps_ready) return;
  const TopoDS_Shape& s = proto_shape(p);
  if (!s.IsNull()) {
    TopExp::MapShapes(s, TopAbs_FACE, p.faces);
    TopExp::MapShapes(s, TopAbs_EDGE, p.edges);
    TopExp::MapShapes(s, TopAbs_VERTEX, p.verts);
  }
  p.maps_ready = true;
}

TopoDS_Shape Doc::located_shape(const TNode& n) const {
  // core.py/main.py: GetShape(instance label) moved by the parent location == prototype shape moved by loc.
  Proto& p = *protos[static_cast<size_t>(n.proto)];
  TopoDS_Shape s = proto_shape(p);
  if (s.IsNull()) return s;
  return n.loc.IsIdentity() ? s : s.Moved(n.loc);
}

void Doc::leaves_of(uint32_t node, std::vector<uint32_t>& out) const {
  if (node >= nodes.size()) return;
  std::vector<uint32_t> stack{node};
  while (!stack.empty()) {
    uint32_t i = stack.back();
    stack.pop_back();
    const TNode& n = nodes[i];
    if (!n.is_asm) {
      out.push_back(i);
    } else {
      for (auto it = n.children.rbegin(); it != n.children.rend(); ++it) stack.push_back(static_cast<uint32_t>(*it));
    }
  }
}

void Doc::build_tree() const {
  if (tree_built) return;
  ensure_protos();
  nodes.clear();
  occ::handle<XCAFDoc_ShapeTool> st = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  NCollection_Sequence<TDF_Label> free_shapes;
  st->GetFreeShapes(free_shapes);

  struct Frame {
    int node;
    std::vector<TDF_Label> comps;
    size_t next = 0;
  };
  std::vector<Frame> stack;

  auto make = [&](const TDF_Label& lab, int parent, const TopLoc_Location& parent_loc) -> int {
    TNode n;
    n.label = lab;
    n.parent = parent;
    const bool is_ref = XCAFDoc_ShapeTool::IsReference(lab);
    TDF_Label proto = lab;
    n.loc = parent_loc;
    if (is_ref) {
      TDF_Label referred;
      if (XCAFDoc_ShapeTool::GetReferredShape(lab, referred)) proto = referred;
      n.loc = parent_loc.Multiplied(XCAFDoc_ShapeTool::GetLocation(lab));
    }
    n.proto_label = proto;
    n.is_asm = XCAFDoc_ShapeTool::IsAssembly(proto);
    n.proto = n.is_asm ? -1 : static_cast<int>(proto_for_label(proto));
    nodes.push_back(std::move(n));
    return static_cast<int>(nodes.size() - 1);
  };
  auto push_frame = [&](int idx) {
    Frame f;
    f.node = idx;
    NCollection_Sequence<TDF_Label> comps;
    XCAFDoc_ShapeTool::GetComponents(nodes[static_cast<size_t>(idx)].proto_label, comps, false);
    for (int i = 1; i <= comps.Length(); ++i) f.comps.push_back(comps.Value(i));
    stack.push_back(std::move(f));
  };

  for (int i = 1; i <= free_shapes.Length(); ++i) {
    int root = make(free_shapes.Value(i), -1, TopLoc_Location());
    if (nodes[static_cast<size_t>(root)].is_asm) push_frame(root);
    while (!stack.empty()) {
      Frame& f = stack.back();
      if (f.next >= f.comps.size()) {
        stack.pop_back();
        continue;
      }
      TDF_Label c = f.comps[f.next++];
      int parent = f.node;
      int idx = make(c, parent, nodes[static_cast<size_t>(parent)].loc);
      nodes[static_cast<size_t>(parent)].children.push_back(idx);
      if (nodes[static_cast<size_t>(idx)].is_asm) push_frame(idx);
    }
  }
  tree_built = true;
}

// ------------------------------------------------------------------ creation / storage

std::unique_ptr<Doc> new_doc() {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(std::unique_ptr<Doc>)
  return make_doc_locked();
  BR_END
}

namespace {

// Reads the triangles of an STL file. Reading takes no lock (it touches no shared state of the application).
occ::handle<Poly_Triangulation> read_stl_triangulation(const std::string& p) {
  BR_BEGIN(occ::handle<Poly_Triangulation>)
  // RWStl: nodes are merged below 30 degrees so that sharp edges survive (core.py: read_stl).
  const double merge_angle = 30.0 * (3.14159265358979323846 / 180.0);
  return RWStl::ReadFile(p.c_str(), merge_angle);
  BR_END
}

}  // namespace

std::unique_ptr<Doc> read_stl(rust::Str path, rust::Str name) {
  std::string p(path);
  occ::handle<Poly_Triangulation> tri = read_stl_triangulation(p);
  if (tri.IsNull() || tri->NbTriangles() == 0) return nullptr;
  // the lock is taken outside the guarded region: a hardware fault caught by the guard does not unwind the C++ frames
  // inside it, so a lock held there would stay locked and block every later load
  std::lock_guard<std::mutex> lk(g_io);
  BR_BEGIN(std::unique_ptr<Doc>)
  std::unique_ptr<Doc> d = make_doc_locked();
  fault_point(FAULT_READ_STL);
  TopoDS_Face face;
  BRep_Builder().MakeFace(face, tri);
  occ::handle<XCAFDoc_ShapeTool> st = XCAFDoc_DocumentTool::ShapeTool(d->doc->Main());
  TDF_Label lab = st->AddShape(face, false);
  std::string n(name);
  TDataStd_Name::Set(lab, TCollection_ExtendedString(n.c_str(), true));
  return d;
  BR_END
}

std::unique_ptr<Doc> load_xbf(rust::Str path, rust::String& status) {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(std::unique_ptr<Doc>)
  AppState& st = app_state();
  auto d = std::make_unique<Doc>();
  d->doc = new TDocStd_Document(TCollection_ExtendedString("BinXCAF"));
  std::string p(path);
  st.retr->Read(TCollection_ExtendedString(p.c_str(), true), d->doc, st.app);
  status = rust::String(reader_status_name(st.retr->GetStatus()));
  return d;
  BR_END
}

rust::String doc_save_xbf(const Doc& d, rust::Str path) {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(rust::String)
  std::string p(path);
  TCollection_ExtendedString message;
  PCDM_StoreStatus s = app_state().app->SaveAs(d.doc, TCollection_ExtendedString(p.c_str(), true), message);
  return rust::String(store_status_name(s));
  BR_END
}

// ------------------------------------------------------------------ STEP / IGES reader

std::unique_ptr<CadReader> new_reader(bool iges) {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(std::unique_ptr<CadReader>)
  auto r = std::make_unique<CadReader>();
  if (iges) {
    r->iges = std::make_unique<IGESCAFControl_Reader>();
    r->iges->SetColorMode(true);
    r->iges->SetNameMode(true);
    r->iges->SetLayerMode(false);
  } else {
    r->step = std::make_unique<STEPCAFControl_Reader>();
    r->step->SetColorMode(true);
    r->step->SetNameMode(true);
    r->step->SetLayerMode(false);
    r->step->SetPropsMode(false);
    r->step->SetGDTMode(false);
    r->step->SetMatMode(false);
    r->step->SetViewMode(false);
    // OCCT 8 also reads STEP metadata by default; the viewer shows none of it
    r->step->SetMetaMode(false);
  }
  return r;
  BR_END
}

bool reader_read_file(const CadReader& r, rust::Str path) {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(bool)
  std::string p(path);
  IFSelect_ReturnStatus st = r.iges ? r.iges->ReadFile(p.c_str()) : r.step->ReadFile(p.c_str());
  return st == IFSelect_RetDone;
  BR_END
}

// core.py: relax_shapefix. Only for STEP, after ReadFile and before Transfer. FixFaceMode stays on.
void reader_relax_shapefix(const CadReader& r) {
  BR_BEGIN(void)
  if (!r.step) return;
  XSAlgo_ShapeProcessor::ParameterMap m;
  m.Bind(TCollection_AsciiString("FixShape.FixSelfIntersectionMode"), TCollection_AsciiString("0"));
  m.Bind(TCollection_AsciiString("FixShape.FixIntersectingWiresMode"), TCollection_AsciiString("0"));
  r.step->SetShapeFixParameters(m);
  BR_END
}

bool reader_transfer(const CadReader& r, const Doc& d) {
  std::lock_guard<std::mutex> lk(g_io);  // outside the guarded region: it must be released even after a crash
  BR_BEGIN(bool)
  return r.iges ? r.iges->Transfer(d.doc) : r.step->Transfer(d.doc);
  BR_END
}

// ------------------------------------------------------------------ instance count (core.py: count_instances)

namespace {

struct CountAbort {
  uint8_t status;  // 1 cyclic, 2 too deep
};

struct CountCtx {
  std::unordered_map<std::string, uint64_t> memo;  // prototype entry -> nodes in its subtree
  std::unordered_set<std::string> active;          // prototypes on the current path
  uint64_t edges = 0;                              // component references seen
  uint64_t limit = 0;
  int32_t max_depth = 200;
};

uint64_t subtree(CountCtx& c, const TDF_Label& proto, int depth) {
  if (!XCAFDoc_ShapeTool::IsAssembly(proto)) return 1;
  std::string key = entry_of(proto);
  auto it = c.memo.find(key);
  if (it != c.memo.end()) return it->second;
  if (c.active.count(key) != 0) throw CountAbort{1};
  if (depth > c.max_depth) throw CountAbort{2};
  uint64_t n = 1;
  c.active.insert(key);
  NCollection_Sequence<TDF_Label> comps;
  XCAFDoc_ShapeTool::GetComponents(proto, comps, false);
  c.edges += static_cast<uint64_t>(comps.Length());
  for (int i = 1; i <= comps.Length(); ++i) {
    n += subtree(c, proto_of(comps.Value(i)), depth + 1);
    if (n > c.limit) break;
  }
  c.active.erase(key);
  c.memo[key] = n;
  return n;
}

}  // namespace

CountRec doc_count_instances(const Doc& d, uint64_t limit, uint64_t amplify_min, uint64_t amplify_ratio, int32_t max_depth) {
  BR_BEGIN(CountRec)
  occ::handle<XCAFDoc_ShapeTool> st = XCAFDoc_DocumentTool::ShapeTool(d.doc->Main());
  NCollection_Sequence<TDF_Label> free_shapes;
  st->GetFreeShapes(free_shapes);
  CountCtx c;
  c.limit = limit;
  c.max_depth = max_depth;
  CountRec out{};
  uint64_t total = 0;
  try {
    for (int i = 1; i <= free_shapes.Length(); ++i) {
      total += subtree(c, proto_of(free_shapes.Value(i)), 0);
      if (total > limit) break;
    }
  } catch (const CountAbort& a) {
    out.status = a.status;
    return out;
  }
  out.total = total;
  if (total > limit) {
    out.status = 3;
    return out;
  }
  const uint64_t stored = static_cast<uint64_t>(c.memo.size()) + c.edges + static_cast<uint64_t>(free_shapes.Length());
  out.stored = stored;
  if (total > amplify_min && total > amplify_ratio * stored) out.status = 4;
  return out;
  BR_END
}

// ------------------------------------------------------------------ meshing (core.py: mesh_doc)

namespace {

IMeshTools_Parameters mesh_params(double defl, bool delabella) {
  IMeshTools_Parameters p;
  p.Deflection = defl;
  p.Angle = 0.35;
  p.Relative = false;
  p.InParallel = true;
  if (delabella) p.MeshAlgo = IMeshTools_MeshAlgoType_Delabella;
  return p;
}

}  // namespace

rust::Vec<double> doc_mesh_items(const Doc& d) {
  BR_BEGIN(rust::Vec<double>)
  d.ensure_protos();
  d.mesh_shapes.clear();
  rust::Vec<double> sizes;
  occ::handle<XCAFDoc_ShapeTool> st = XCAFDoc_DocumentTool::ShapeTool(d.doc->Main());
  NCollection_Sequence<TDF_Label> seq;
  st->GetShapes(seq);
  for (int i = 1; i <= seq.Length(); ++i) {
    const TDF_Label& lab = seq.Value(i);
    if (XCAFDoc_ShapeTool::IsAssembly(lab)) continue;
    TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(lab);
    if (shape.IsNull() || shape_is_mesh_only(shape)) continue;
    Bnd_Box box;
    BRepBndLib::Add(shape, box, false);
    if (box.IsVoid()) continue;
    const gp_Pnt a = box.CornerMin();
    const gp_Pnt b = box.CornerMax();
    const double size = std::max({b.X() - a.X(), b.Y() - a.Y(), b.Z() - a.Z()});
    d.mesh_shapes.push_back(shape);
    sizes.push_back(size);
  }
  return sizes;
  BR_END
}

void doc_mesh_run(const Doc& d, rust::Slice<const int32_t> keys, bool no_delabella, uint32_t delabella_min_faces) {
  BR_BEGIN(void)
  if (keys.size() != d.mesh_shapes.size()) throw Fail("Interní chyba: počet těles k síťování nesouhlasí.");
  // buckets in order of first appearance of their key (a Python dict keeps insertion order)
  std::vector<std::pair<int32_t, std::vector<TopoDS_Shape>>> buckets;
  for (size_t i = 0; i < d.mesh_shapes.size(); ++i) {
    const int32_t k = keys[i];
    size_t b = 0;
    for (; b < buckets.size(); ++b) {
      if (buckets[b].first == k) break;
    }
    if (b == buckets.size()) buckets.emplace_back(k, std::vector<TopoDS_Shape>());
    buckets[b].second.push_back(d.mesh_shapes[i]);
  }

  bool fast = false;
  if (!no_delabella) {
    ShapeMap all_faces;
    for (const auto& b : buckets) {
      for (const TopoDS_Shape& s : b.second) TopExp::MapShapes(s, TopAbs_FACE, all_faces);
    }
    fast = static_cast<uint32_t>(all_faces.Extent()) > delabella_min_faces;
  }

  BRep_Builder builder;
  for (const auto& b : buckets) {
    TopoDS_Compound comp;
    builder.MakeCompound(comp);
    for (const TopoDS_Shape& s : b.second) builder.Add(comp, s);
    const double defl = std::pow(2.0, static_cast<double>(b.first) / 2.0);
    if (!fast) {
      BRepMesh_IncrementalMesh(comp, defl, false, 0.35, true);
      continue;
    }
    BRepMesh_IncrementalMesh(comp, mesh_params(defl, true));
    // Delabella must not leave a face without triangulation: cheap check of the whole compound first, then
    // the faces it skipped get the default mesher.
    if (!BRepTools::Triangulation(comp, defl * 1.001, false)) {
      std::vector<TopoDS_Shape> missing;
      for (const TopoDS_Shape& s : b.second) {
        for (TopExp_Explorer e(s, TopAbs_FACE); e.More(); e.Next()) {
          TopLoc_Location loc;
          const occ::handle<Poly_Triangulation>& t = BRep_Tool::Triangulation(TopoDS::Face(e.Current()), loc);
          if (t.IsNull() || t->NbTriangles() == 0) missing.push_back(e.Current());
        }
      }
      if (!missing.empty()) {
        TopoDS_Compound redo;
        builder.MakeCompound(redo);
        for (const TopoDS_Shape& f : missing) builder.Add(redo, f);
        BRepMesh_IncrementalMesh(redo, mesh_params(defl, false));
      }
    }
  }
  d.mesh_shapes.clear();
  BR_END
}

// ------------------------------------------------------------------ tree (core.py: build_tree)

TreeRec doc_tree(const Doc& d) {
  BR_BEGIN(TreeRec)
  d.build_tree();
  TreeRec out;
  out.proto_instances.reserve(d.protos.size());
  std::vector<uint32_t> counts(d.protos.size(), 0);
  for (const TNode& n : d.nodes) {
    if (!n.is_asm && n.proto >= 0) counts[static_cast<size_t>(n.proto)] += 1;
  }
  for (uint32_t c : counts) out.proto_instances.push_back(c);

  out.nodes.reserve(d.nodes.size());
  for (const TNode& n : d.nodes) {
    NodeRec r{};
    r.parent = n.parent;
    const bool is_ref = XCAFDoc_ShapeTool::IsReference(n.label);
    const std::string proto_name = label_name(n.proto_label);
    const std::string inst = is_ref ? label_name(n.label) : std::string();
    const bool auto_name = inst.empty() || inst.rfind("=>[", 0) == 0;
    std::string name = auto_name ? proto_name : inst;
    if (name.empty()) name = "(bez názvu)";
    r.name = to_rs(name);
    r.proto_name = to_rs(proto_name);
    r.key = to_rs(entry_of(n.proto_label));
    r.is_assembly = n.is_asm;
    r.proto = n.is_asm ? -1 : n.proto;
    const gp_Trsf t = n.loc.Transformation();
    for (int col = 0; col < 4; ++col) {
      for (int row = 0; row < 3; ++row) r.location[static_cast<size_t>(col * 4 + row)] = t.Value(row + 1, col + 1);
      r.location[static_cast<size_t>(col * 4 + 3)] = col == 3 ? 1.0 : 0.0;
    }
    Quantity_Color c;
    bool have = false;
    if (is_ref && label_color(n.label, c)) {
      r.own_style = true;
      have = true;
    }
    if (!have) have = label_color(n.proto_label, c);
    if (have) {
      r.has_color = true;
      r.color = {static_cast<float>(c.Red()), static_cast<float>(c.Green()), static_cast<float>(c.Blue())};
    }
    out.nodes.push_back(std::move(r));
  }
  return out;
  BR_END
}

uint32_t doc_proto_count(const Doc& d) {
  BR_BEGIN(uint32_t)
  d.ensure_protos();
  return static_cast<uint32_t>(d.protos.size());
  BR_END
}

}  // namespace stepview
