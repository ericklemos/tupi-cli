//! Índice vetorial local do repositório (o "espaço").

pub mod build;
pub mod index;
pub mod store;

pub use build::{BuildReport, needs_rebuild, rebuild, spawn, symbol_text};
pub use index::{SpaceEntry, SpaceIndex, SpaceSnapshot};
pub use store::{SpaceFile, SpaceManifest, SpaceSymbol};
