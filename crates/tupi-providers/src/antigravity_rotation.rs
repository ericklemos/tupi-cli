//! Rotacao e cooldown das contas Antigravity do tupi.
//!
//! O Hub oficial mantem N contas com quota propria por conta, cooldown e
//! rotacao. O tupi importa as contas por copia (`antigravity_accounts`) e aqui
//! escolhe qual conta atende cada chamada: parte da conta ativa, roda em
//! circulo e pula quem esta em cooldown ou sem entitlement. Quando nenhuma
//! conta pode atender, o erro e agregado e visivel — nunca um retry silencioso
//! em conta bloqueada.

use tupi_kernel::ProviderError;

use crate::antigravity::AntigravityCredentials;
use crate::antigravity_accounts::AntigravityAccountStore;

/// Cooldown por conta, em segundos de relogio.
///
/// Fica fora do store serializado de proposito: cooldown e estado de execucao,
/// nao configuracao. A quota do Hub e por conta, entao o bloqueio tambem e.
#[derive(Debug, Clone, Default)]
pub struct AccountCooldowns {
    until: std::collections::HashMap<String, u64>,
}

impl AccountCooldowns {
    pub fn new() -> Self {
        Self::default()
    }

    /// Segundos que faltam ate a conta poder ser usada de novo.
    pub fn remaining_secs(
        &self,
        account: &AntigravityCredentials,
        now_secs: u64,
    ) -> Option<u64> {
        let until = *self.until.get(&account_key(account))?;
        (until > now_secs).then(|| until.saturating_sub(now_secs))
    }

    pub fn is_cooling(&self, account: &AntigravityCredentials, now_secs: u64) -> bool {
        self.remaining_secs(account, now_secs).is_some()
    }

    /// Marca a conta por `cooldown_secs` a partir de `now_secs`. Cooldown novo
    /// nunca encurta o que ja estava marcado.
    pub fn mark(&mut self, account: &AntigravityCredentials, now_secs: u64, cooldown_secs: u64) {
        let until = now_secs.saturating_add(cooldown_secs.max(1));
        let entry = self.until.entry(account_key(account)).or_insert(until);
        if *entry < until {
            *entry = until;
        }
    }

    pub fn clear(&mut self, account: &AntigravityCredentials) {
        self.until.remove(&account_key(account));
    }

    /// Menor espera pendente entre as contas informadas; vira o `reset` do erro
    /// agregado, para o usuario saber quando vale tentar de novo.
    pub fn soonest_reset_secs(
        &self,
        accounts: &[AntigravityCredentials],
        now_secs: u64,
    ) -> Option<u64> {
        accounts
            .iter()
            .filter_map(|account| self.remaining_secs(account, now_secs))
            .min()
    }
}

/// Identidade estavel da conta: e-mail quando existe, senao o digest do refresh
/// token (o token em si nunca vira chave nem texto de erro).
pub fn account_key(account: &AntigravityCredentials) -> String {
    if let Some(email) = account
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
    {
        return format!("email:{email}");
    }
    let digest = blake3::hash(account.refresh_token.as_bytes());
    let hex = digest.to_hex().to_string();
    format!("token:{}", &hex[..16])
}

/// Rotulo curto da conta na mensagem de erro (sem segredo).
fn account_label(account: &AntigravityCredentials) -> String {
    let key = account_key(account);
    match key.strip_prefix("email:") {
        Some(email) => email.to_string(),
        None => key,
    }
}

/// Decide se a conta pode atender a chamada (entitlement real do catalogo).
pub type AccountEntitlement = dyn Fn(&AntigravityCredentials) -> bool + Send + Sync;

/// Escolhe a proxima conta utilizavel: parte da conta ativa e roda em circulo,
/// pulando quem esta em cooldown ou sem entitlement.
///
/// Sem nenhuma conta utilizavel o erro e agregado e visivel, com o motivo de
/// cada conta e a menor espera pendente no `reset`.
pub fn select_account(
    accounts: &[AntigravityCredentials],
    active_index: usize,
    now_secs: u64,
    cooldowns: &AccountCooldowns,
    entitled: Option<&AccountEntitlement>,
) -> Result<usize, ProviderError> {
    if accounts.is_empty() {
        return Err(ProviderError::Protocol(
            "nenhuma conta Antigravity configurada; rode o login do provider".to_string(),
        ));
    }

    let len = accounts.len();
    let start = active_index % len;
    let mut reasons = Vec::new();

    for offset in 0..len {
        let index = (start + offset) % len;
        let account = &accounts[index];
        if let Some(wait) = cooldowns.remaining_secs(account, now_secs) {
            reasons.push(format!(
                "{} em cooldown por {wait}s",
                account_label(account)
            ));
            continue;
        }
        if let Some(check) = entitled
            && !check(account)
        {
            reasons.push(format!(
                "{} sem entitlement para o modelo",
                account_label(account)
            ));
            continue;
        }
        return Ok(index);
    }

    Err(ProviderError::QuotaExhausted {
        status: 429,
        reset: cooldowns.soonest_reset_secs(accounts, now_secs),
        message: format!(
            "nenhuma das {len} contas Antigravity pode atender agora: {}",
            reasons.join("; ")
        ),
        body: String::new(),
    })
}

impl AntigravityAccountStore {
    /// Avanca a rotacao para a proxima conta utilizavel e a devolve; a conta
    /// escolhida passa a ser a ativa do store (o chamador persiste).
    pub fn select(
        &mut self,
        now_secs: u64,
        cooldowns: &AccountCooldowns,
    ) -> Result<&AntigravityCredentials, ProviderError> {
        let index = select_account(&self.accounts, self.active_index, now_secs, cooldowns, None)?;
        self.active_index = index;
        self.accounts
            .get(index)
            .ok_or_else(|| ProviderError::Protocol("indice de conta Antigravity invalido".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(email: &str, refresh_token: &str) -> AntigravityCredentials {
        AntigravityCredentials {
            access_token: String::new(),
            refresh_token: refresh_token.to_string(),
            expires_at: 0,
            project_id: "proj".to_string(),
            email: Some(email.to_string()),
        }
    }

    #[test]
    fn rotation_skips_cooling_account_and_picks_the_next() {
        let accounts = vec![
            account("a@example.com", "r1"),
            account("b@example.com", "r2"),
        ];
        let mut cooldowns = AccountCooldowns::new();
        cooldowns.mark(&accounts[0], 1_000, 120);

        let index = select_account(&accounts, 0, 1_000, &cooldowns, None).expect("conta b");
        assert_eq!(index, 1, "primeira conta em cooldown cede a vez");

        let mut store = AntigravityAccountStore {
            accounts: accounts.clone(),
            active_index: 0,
        };
        let chosen = store.select(1_000, &cooldowns).expect("conta b");
        assert_eq!(chosen.email.as_deref(), Some("b@example.com"));
        assert_eq!(store.active_index, 1, "rotacao move a conta ativa");
    }

    #[test]
    fn rotation_skips_account_without_entitlement() {
        let accounts = vec![
            account("a@example.com", "r1"),
            account("b@example.com", "r2"),
        ];
        let cooldowns = AccountCooldowns::new();
        let entitled =
            |creds: &AntigravityCredentials| creds.email.as_deref() != Some("a@example.com");

        let index = select_account(&accounts, 0, 1_000, &cooldowns, Some(&entitled))
            .expect("conta b com entitlement");
        assert_eq!(index, 1);
    }

    #[test]
    fn all_accounts_cooling_returns_aggregated_quota_error() {
        let accounts = vec![
            account("a@example.com", "r1"),
            account("b@example.com", "r2"),
        ];
        let mut cooldowns = AccountCooldowns::new();
        cooldowns.mark(&accounts[0], 1_000, 300);
        cooldowns.mark(&accounts[1], 1_000, 120);

        let error =
            select_account(&accounts, 0, 1_000, &cooldowns, None).expect_err("nenhuma conta livre");
        match error {
            ProviderError::QuotaExhausted {
                status,
                reset,
                message,
                ..
            } => {
                assert_eq!(status, 429);
                assert_eq!(reset, Some(120), "reset e a menor espera pendente");
                assert!(message.contains("a@example.com"), "{message}");
                assert!(message.contains("b@example.com"), "{message}");
            }
            other => panic!("esperava quota agregada, veio {other:?}"),
        }
    }
}
