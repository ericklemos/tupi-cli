//! Store local das contas Antigravity do tupi.
//!
//! O client do Hub (plugin opencode-antigravity-auth) guarda N contas em
//! `antigravity-accounts.json` dentro do diretorio de configuracao dele. O tupi
//! nao escreve nesse arquivo: importa por copia as contas habilitadas para o
//! proprio store e trabalha so com a copia. Escrever na origem seria dividir
//! estado — e refresh token vivo — com outro cliente.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::antigravity::AntigravityCredentials;

/// Registro local das contas Antigravity, com a conta ativa marcada.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AntigravityAccountStore {
    #[serde(default)]
    pub accounts: Vec<AntigravityCredentials>,
    /// Indice da conta em uso; a rotacao de contas parte daqui.
    #[serde(default)]
    pub active_index: usize,
}

impl AntigravityAccountStore {
    /// Nome do arquivo do store local, dentro do diretorio de estado do tupi.
    /// E o mesmo nome do arquivo do plugin, nunca o mesmo caminho.
    pub const FILE_NAME: &'static str = "antigravity-accounts.json";

    pub fn path(tupi_dir: &Path) -> PathBuf {
        tupi_dir.join(Self::FILE_NAME)
    }

    /// Leitura tolerante: arquivo ausente, truncado ou de outro formato vira
    /// store vazio — o boot nao pode falhar por causa de credencial.
    pub fn load(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&content).unwrap_or_default()
    }

    pub fn load_from_dir(tupi_dir: &Path) -> Self {
        Self::load(&Self::path(tupi_dir))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, content)
    }

    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    pub fn active(&self) -> Option<&AntigravityCredentials> {
        self.accounts
            .get(self.active_index)
            .or_else(|| self.accounts.first())
    }

    /// Acrescenta contas ja normalizadas e devolve quantos registros novos
    /// entraram. Refresh token repetido nao duplica registro: o registro local
    /// e preservado, porque pode carregar token mais novo.
    pub fn merge(&mut self, accounts: impl IntoIterator<Item = AntigravityCredentials>) -> usize {
        let mut added = 0;
        for creds in accounts {
            if self
                .accounts
                .iter()
                .any(|known| known.refresh_token == creds.refresh_token)
            {
                continue;
            }
            self.accounts.push(creds);
            added += 1;
        }
        added
    }

    /// Copia as contas habilitadas do arquivo do plugin para este store.
    ///
    /// A origem e aberta so para leitura (`read_to_string`); devolve quantos
    /// registros novos entraram.
    pub fn import_from_opencode_file(
        &mut self,
        source: &Path,
        default_project_id: &str,
    ) -> std::io::Result<usize> {
        let content = std::fs::read_to_string(source)?;
        let (accounts, active_index) = parse_opencode_accounts(&content, default_project_id);
        let was_empty = self.is_empty();
        let added = self.merge(accounts);
        if was_empty && !self.is_empty() {
            self.active_index = active_index.min(self.accounts.len().saturating_sub(1));
        }
        Ok(added)
    }
}

/// Contas habilitadas do arquivo do plugin, na ordem do arquivo, com o indice
/// da conta ativa dentro da lista devolvida.
///
/// A importacao e copia: so `refreshToken`, projeto e e-mail atravessam.
/// `accessToken`/`expiresAt` do outro cliente nunca sao reaproveitados, porque
/// token vivo nao se compartilha entre clientes — a conta se reautentica pelo
/// nosso caminho.
pub fn parse_opencode_accounts(
    text: &str,
    default_project_id: &str,
) -> (Vec<AntigravityCredentials>, usize) {
    let Ok(val) = serde_json::from_str::<Value>(text) else {
        return (Vec::new(), 0);
    };
    let Some(raw_accounts) = val.get("accounts").and_then(Value::as_array) else {
        return (Vec::new(), 0);
    };
    let active_raw = val
        .get("activeIndex")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;

    let mut accounts: Vec<AntigravityCredentials> = Vec::new();
    let mut active_index = 0;
    for (raw_index, raw) in raw_accounts.iter().enumerate() {
        let Some(creds) = account_from_json(raw, default_project_id) else {
            continue;
        };
        if accounts
            .iter()
            .any(|known| known.refresh_token == creds.refresh_token)
        {
            continue; // o mesmo refresh token nao vira dois registros
        }
        if raw_index == active_raw {
            active_index = accounts.len();
        }
        accounts.push(creds);
    }

    (accounts, active_index)
}

/// Uma conta do arquivo do plugin; `None` quando esta desabilitada ou nao tem
/// refresh token utilizavel.
fn account_from_json(account: &Value, default_project_id: &str) -> Option<AntigravityCredentials> {
    let disabled = account
        .get("disabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let enabled = account
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if disabled || !enabled {
        return None;
    }

    let refresh_token = account.get("refreshToken")?.as_str()?.trim().to_string();
    if refresh_token.is_empty() {
        return None;
    }

    let project_id = account
        .get("managedProjectId")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            account
                .get("projectId")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or(default_project_id)
        .to_string();
    let email = account
        .get("email")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    Some(AntigravityCredentials {
        access_token: String::new(),
        refresh_token,
        expires_at: 0,
        project_id,
        email,
    })
}
