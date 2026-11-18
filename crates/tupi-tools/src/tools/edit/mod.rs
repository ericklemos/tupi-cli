//! Tool `edit`: toda escrita cirúrgica entra por um endereço e sai por um
//! resolvedor escolhido por dado (`EditAddress`), nunca por extensão. As rotas
//! textuais (`old`+`new`, `start`+`end`+`expect`+`new`) valem para qualquer
//! arquivo de texto; `symbol` é otimização de quem tem parser (`resolve`).

mod literal;

#[cfg(test)]
mod literal_tests;

mod params;

#[cfg(test)]
mod range_tests;

mod range;
mod resolve;
mod splice;
mod symbol;

#[cfg(test)]
mod tests;

mod tool;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tree_sitter::Parser;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::span::{SpanResolver, SymbolSpan, render_candidate};
use crate::store::ResponseStore;

pub use params::{EditAddress, EditParams};

pub struct EditTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl EditTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }

    /// Despacho por endereço: o dado decide o resolvedor e nenhuma rota
    /// ramifica por linguagem — `symbol` consulta a tabela de parsers em
    /// `resolve` e, sem parser, a recusa aponta as rotas textuais.
    pub fn edit(&self, params: EditParams) -> Result<String, ToolError> {
        match params.address()? {
            EditAddress::Symbol {
                name,
                new_body,
                signature,
            } => symbol::edit_symbol(
                &self.root_dir,
                &self.store,
                self.default_budget_tokens,
                &params,
                &name,
                &new_body,
                signature.as_deref(),
            ),
            EditAddress::Lines {
                start,
                end,
                expect,
                new,
            } => range::edit_range(
                &self.root_dir,
                &self.store,
                self.default_budget_tokens,
                &params,
                start,
                end,
                &expect,
                &new,
            ),
            EditAddress::Literal { old, new } => literal::edit_literal(
                &self.root_dir,
                &self.store,
                self.default_budget_tokens,
                &params,
                &old,
                &new,
            ),
        }
    }
}
