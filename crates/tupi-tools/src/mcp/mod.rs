//! Cliente MCP: servidores declarados em `ai.project.json` falam JSON-RPC 2.0
//! pelo stdin/stdout de um processo filho.
//!
//! [`ServerSpec`] é o vocabulário neutro que atravessa a fronteira
//! `tupi` -> `tupi-tools`: a conversão de `ProjectConfig` acontece em
//! `crates/tupi/src/main.rs`, porque `tupi-tools` não conhece a configuração do
//! repositório (a dependência só aponta para baixo).

pub mod client;

pub use client::{Connection, ToolInfo};

mod tool;

pub use tool::McpTool;

use std::collections::BTreeMap;

/// Servidor MCP já validado pela config do repositório.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ServerSpec {
    /// Nome declarado na chave `mcp` do `ai.project.json`.
    pub name: String,
    /// Programa e argumentos, em ordem. O primeiro item é o executável.
    pub command: Vec<String>,
    /// Variáveis extras do processo filho. Nunca vão para log nem para
    /// mensagem de erro: podem carregar credencial.
    pub env: BTreeMap<String, String>,
    /// `true` declara que as tools do servidor não escrevem.
    pub read_only: bool,
}

/// `Debug` redigido: `env` pode carregar credencial, e o `{:?}` de um spec acaba
/// em log, panic ou mensagem de erro. Nome, comando e `read_only` seguem legíveis.
impl std::fmt::Debug for ServerSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerSpec")
            .field("name", &self.name)
            .field("command", &self.command)
            .field(
                "env",
                &format_args!("<{} variável(is) oculta(s)>", self.env.len()),
            )
            .field("read_only", &self.read_only)
            .finish()
    }
}

impl ServerSpec {
    /// Spec de transporte stdio, sem env extra e sem promessa de leitura.
    pub fn local(name: impl Into<String>, command: Vec<String>) -> Self {
        Self {
            name: name.into(),
            command,
            env: BTreeMap::new(),
            read_only: false,
        }
    }
}
