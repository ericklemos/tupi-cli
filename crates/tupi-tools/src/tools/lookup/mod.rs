pub(crate) mod exec;
pub(crate) mod graph;
pub(crate) mod params;
pub(crate) mod render;

use std::path::PathBuf;
use std::sync::Arc;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::read::ReadTool;
use crate::search::SearchTool;
use crate::support::ledger::DeliveryLedger;
use crate::support::store::ResponseStore;

use params::{LookupParams, NAME};

/// A tool única de leitura/busca: um lote heterogêneo de ops, um orçamento e
/// um ledger. `read`, `read_many` e `search` viram `kind` daqui.
pub(crate) struct LookupTool {
    read: Arc<ReadTool>,
    search: Arc<SearchTool>,
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    ledger: Arc<DeliveryLedger>,
    default_budget_tokens: usize,
}

impl LookupTool {
    pub(crate) fn new(
        read: Arc<ReadTool>,
        search: Arc<SearchTool>,
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        ledger: Arc<DeliveryLedger>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            read,
            search,
            root_dir,
            store,
            ledger,
            default_budget_tokens,
        }
    }
}

impl Tool for LookupTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: NAME.to_string(),
            description: params::DESCRIPTION.to_string(),
            schema: params::schema(),
        }
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: LookupParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let engines = exec::Engines::new(&self.read, &self.search, &self.root_dir);
        Ok(engines.run(
            &params,
            self.default_budget_tokens,
            &self.ledger,
            &self.store,
        ))
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let kinds: Vec<&str> = arguments
            .get("ops")
            .and_then(|ops| ops.as_array())
            .map(|ops| {
                ops.iter()
                    .map(|op| {
                        op.get("kind")
                            .and_then(|kind| kind.as_str())
                            .unwrap_or("?")
                    })
                    .collect()
            })
            .unwrap_or_default();
        if kinds.is_empty() {
            let mode = arguments
                .get("mode")
                .and_then(|mode| mode.as_str())
                .unwrap_or("lote vazio");
            return format!("lookup mode={mode}");
        }
        format!("lookup {} op(s): {}", kinds.len(), kinds.join(","))
    }
}
