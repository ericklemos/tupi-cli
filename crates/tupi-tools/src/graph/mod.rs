mod builder;
mod legacy;
mod model;
mod queries;
mod scope;

pub use builder::GraphBuilder;
pub use model::{ProjectCodeGraph, RelationEdge, RelationKind, SymbolNode};
pub use scope::{FileScope, ImportTarget, Resolution, ScopeResolver};
// Compatibilidade temporária: as tools antigas continuam compilando.
pub use legacy::{CallGraph, Def, GraphCache};
pub(crate) use legacy::InvalidatingTool;
