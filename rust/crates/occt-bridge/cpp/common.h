// Small helpers shared by the C++ translation units.
#pragma once

#include "bridge.h"

#include <Quantity_Color.hxx>
#include <TopoDS_Shape.hxx>

namespace stepview {

/// UTF-16 (OCCT extended string) to UTF-8; lone surrogates become U+FFFD.
std::string utf16_to_utf8(const char16_t* s);
inline rust::String to_rs(const std::string& s) { return rust::String::lossy(s); }

std::string label_name(const TDF_Label& lab);
/// XCAFDoc_ColorSurf first, then XCAFDoc_ColorGen (core.py: label_color).
bool label_color(const TDF_Label& lab, Quantity_Color& out);
std::string entry_of(const TDF_Label& lab);
/// True when the first face has no surface (STL-like, core.py: is_mesh_only).
bool shape_is_mesh_only(const TopoDS_Shape& shape);

/// Fills a MeshRec from any shape (faces / edges / vertices in TopExp::MapShapes order). `colors` is optional.
struct FaceColors {
  std::vector<unsigned char> has;
  std::vector<float> rgb;  // 3 per face
};
void extract_mesh(const TopoDS_Shape& shape, const ShapeMap& faces, const ShapeMap& edges, const ShapeMap& verts,
                  bool mesh_only, const FaceColors* colors, double edge_defl, MeshRec& out);

}  // namespace stepview
