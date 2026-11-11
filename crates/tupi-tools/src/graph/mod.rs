mod builder;
mod legacy;
mod model;
mod queries;

pub use builder::GraphBuilder;
pub use model::{ProjectCodeGraph, RelationEdge, RelationKind, SymbolNode};
// Compatibilidade temporária: as tools antigas continuam compilando.
pub use legacy::{CallGraph, Def, GraphCache};
pub(crate) use legacy::InvalidatingTool;
