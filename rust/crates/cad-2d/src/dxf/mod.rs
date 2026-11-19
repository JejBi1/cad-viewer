//! DXF file reader (ASCII and binary) producing a typed document.
pub mod entities;
pub mod loader;
pub mod model;
pub mod tags;

pub use loader::load_document;
pub use model::*;
