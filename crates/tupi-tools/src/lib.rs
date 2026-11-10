mod ast;
mod budget;
mod ledger;
mod peek;
mod read;
mod read_many;
mod store;

pub use budget::{
    estimate_tokens, render, select, split_section, Section, Selection, CHARS_PER_TOKEN,
    DEFAULT_BUDGET_TOKENS,
};
pub use ast::{AstChunker, AstSnippet};
pub use peek::PeekTool;
pub use read::{ReadParams, ReadTool};
pub use read_many::{ReadItem, ReadManyParams, ReadManyTool};
pub use store::ResponseStore;

use std::sync::Arc;
use std::time::Duration;

use tupi_kernel::ToolRegistry;

use crate::ledger::DeliveryLedger;

/// Registra `read`, `read_many` e `peek`, compartilhando store e ledger.
pub fn register(root_dir: std::path::PathBuf, registry: &mut ToolRegistry) {
    let store = Arc::new(ResponseStore::new(
        Duration::from_secs(600),
        8 * 1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(
        crate::ledger::DEFAULT_ENTRIES,
        crate::ledger::DEFAULT_MAX_BYTES,
    ));
    let read = Arc::new(ReadTool::new(
        root_dir,
        store.clone(),
        ledger,
        DEFAULT_BUDGET_TOKENS,
    ));

    registry.register(read.clone());
    registry.register(Arc::new(ReadManyTool::new(read)));
    registry.register(Arc::new(PeekTool::new(store, DEFAULT_BUDGET_TOKENS)));
}
