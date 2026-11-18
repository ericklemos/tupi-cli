//! T8.1: importar todas as contas habilitadas do arquivo do plugin por copia.

use tupi_providers::antigravity::AntigravityCredentials;
use tupi_providers::antigravity_accounts::AntigravityAccountStore;

const PLUGIN_JSON: &str = r#"{
    "version": 4,
    "accounts": [
        { "refreshToken": "1//alpha", "projectId": "proj-a", "email": "a@gmail.com" },
        { "refreshToken": "1//bravo", "projectId": "proj-b", "email": "b@gmail.com" },
        { "refreshToken": "1//charlie", "managedProjectId": "proj-c", "email": "c@gmail.com" },
        { "refreshToken": "1//delta", "projectId": "proj-d", "email": "d@gmail.com", "disabled": true }
    ],
    "activeIndex": 2
}"#;

#[test]
fn imports_all_enabled_accounts_by_copy() {
    let tupi_dir = tempfile::tempdir().unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    let plugin_path = plugin_dir
        .path()
        .join(AntigravityAccountStore::FILE_NAME);
    std::fs::write(&plugin_path, PLUGIN_JSON).unwrap();
    let hash_before = blake3::hash(&std::fs::read(&plugin_path).unwrap());

    let mut store = AntigravityAccountStore::load_from_dir(tupi_dir.path());
    let added = store
        .import_from_opencode_file(&plugin_path, "rising-fact-p41fc")
        .unwrap();
    assert_eq!(added, 3, "3 contas habilitadas viram 3 registros locais");
    assert_eq!(store.accounts.len(), 3);
    assert_eq!(
        store
            .accounts
            .iter()
            .map(|a| a.refresh_token.as_str())
            .collect::<Vec<_>>(),
        vec!["1//alpha", "1//bravo", "1//charlie"]
    );
    assert_eq!(store.active_index, 2, "activeIndex do plugin preservado");
    assert_eq!(store.accounts[2].project_id, "proj-c");
    assert_eq!(store.accounts[2].email.as_deref(), Some("c@gmail.com"));
    assert!(
        store
            .accounts
            .iter()
            .all(|a| a.access_token.is_empty() && a.expires_at == 0),
        "token vivo do outro cliente nunca atravessa a copia"
    );

    // arquivo de origem intocado (hash antes/depois)
    let hash_after = blake3::hash(&std::fs::read(&plugin_path).unwrap());
    assert_eq!(hash_before, hash_after, "arquivo do plugin nao e escrito");

    // store local grava as 3 contas e a importacao e idempotente
    store.save(&AntigravityAccountStore::path(tupi_dir.path())).unwrap();
    let mut reloaded = AntigravityAccountStore::load_from_dir(tupi_dir.path());
    assert_eq!(reloaded.accounts.len(), 3);
    assert_eq!(
        reloaded
            .import_from_opencode_file(&plugin_path, "rising-fact-p41fc")
            .unwrap(),
        0,
        "segunda importacao nao duplica"
    );
    assert_eq!(reloaded.accounts.len(), 3);
}

#[test]
fn load_from_dirs_imports_accounts_and_never_writes_source() {
    let tupi_dir = tempfile::tempdir().unwrap();
    let plugin_dir = tempfile::tempdir().unwrap();
    let plugin_path = plugin_dir
        .path()
        .join(AntigravityAccountStore::FILE_NAME);
    std::fs::write(&plugin_path, PLUGIN_JSON).unwrap();
    let hash_before = blake3::hash(&std::fs::read(&plugin_path).unwrap());

    let active =
        AntigravityCredentials::load_from_dirs(tupi_dir.path(), Some(plugin_dir.path())).unwrap();
    assert_eq!(active.refresh_token, "1//charlie", "conta ativa do plugin");

    let hash_after = blake3::hash(&std::fs::read(&plugin_path).unwrap());
    assert_eq!(hash_before, hash_after, "arquivo do plugin nao e escrito");

    let store = AntigravityAccountStore::load_from_dir(tupi_dir.path());
    assert_eq!(store.accounts.len(), 3, "copia local com as 3 contas");

    // mesmo diretorio nos dois lados: nem leitura-escrita circular
    let same = tempfile::tempdir().unwrap();
    std::fs::write(same.path().join(AntigravityAccountStore::FILE_NAME), PLUGIN_JSON).unwrap();
    let same_path = same.path().join(AntigravityAccountStore::FILE_NAME);
    let hash_before = blake3::hash(&std::fs::read(&same_path).unwrap());
    let _ = AntigravityCredentials::load_from_dirs(same.path(), Some(same.path())).unwrap();
    assert_eq!(hash_before, blake3::hash(&std::fs::read(&same_path).unwrap()));
}
