//! Non-UI core of the viewer: model cache, load worker protocol, settings, material library, number formatting and
//! STL mesh analysis. See docs/rust-port/ARCHITECTURE.md.
pub mod cache;
pub mod fmt;
pub mod materials;
pub mod meshmeasure;
pub mod paths;
pub mod settings;
pub mod worker;
