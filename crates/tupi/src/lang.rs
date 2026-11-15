//! Língua da documentação do projeto: o campo `lang` de `ai.project.json`.
//!
//! O campo pode faltar em arquivos anteriores a esta versão. Ausente não é
//! erro: o locale do sistema decide (`LC_ALL` > `LANG` > `LANGUAGE`) e o valor
//! detectado é gravado de volta preservando os demais campos. Presente e
//! inválido é erro que derruba a configuração inteira, como `docs_folders`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::project_config::{ProjectConfig, ProjectConfigError};

/// Língua da documentação do projeto.
///
/// Só existem templates para estes dois idiomas; locale do sistema sem
/// equivalente resolve para `En`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Português do Brasil.
    PtBr,
    /// Inglês.
    En,
}

impl Lang {
    /// Identificador gravado em `ai.project.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Lang::PtBr => "pt-BR",
            Lang::En => "en",
        }
    }

    /// Normaliza o valor do campo: aceita `pt-BR`, `pt_br` e `PT-br` para
    /// `PtBr`, e `en`, `EN` e `en_US` para `En`.
    ///
    /// `None` para qualquer outro idioma: a recusa é decisão explícita, não
    /// palpite sobre o texto do repositório.
    pub fn parse(raw: &str) -> Option<Lang> {
        let normalized = raw.trim().replace('_', "-").to_ascii_lowercase();
        let tag = normalized
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .split('-')
            .next()
            .unwrap_or_default();
        match tag {
            "pt" => Some(Lang::PtBr),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    /// Língua de um locale do sistema (`pt_BR.UTF-8` vira `pt-BR`).
    ///
    /// Locale sem template equivalente (`fr_FR.UTF-8`) ou vazio resolve para
    /// inglês.
    pub fn from_locale(locale: &str) -> Lang {
        Lang::parse(locale).unwrap_or(Lang::En)
    }

    /// Língua do ambiente, com precedência `LC_ALL` > `LANG` > `LANGUAGE`.
    pub fn from_system() -> Lang {
        Lang::from_env(|key| std::env::var(key).ok())
    }

    /// Mesma precedência de [`Lang::from_system`], com a fonte injetável para
    /// o teste não tocar no ambiente global.
    pub fn from_env<F>(lookup: F) -> Lang
    where
        F: Fn(&str) -> Option<String>,
    {
        for key in ["LC_ALL", "LANG", "LANGUAGE"] {
            let Some(value) = lookup(key) else { continue };
            // `LANGUAGE` é uma lista de locales separada por `:`.
            for entry in value.split(':') {
                if entry.trim().is_empty() {
                    continue;
                }
                return Lang::from_locale(entry);
            }
        }
        Lang::En
    }
}

impl ProjectConfig {
    /// Língua resolvida: a do arquivo ou, quando o campo está ausente, a do
    /// locale do sistema.
    pub fn lang(&self) -> Lang {
        self.lang.unwrap_or_else(Lang::from_system)
    }
}

/// Grava de volta o `lang` detectado quando o arquivo ainda não tem o campo.
///
/// Preserva os demais campos e as chaves desconhecidas, e é idempotente: na
/// segunda execução o campo já está lá e nada é escrito, então o índice de
/// docs não é invalidado por `mtime`.
pub fn record(path: &Path, config: ProjectConfig) -> ProjectConfig {
    if config.lang.is_some() {
        return config;
    }
    let lang = Lang::from_system();
    let config = ProjectConfig {
        lang: Some(lang),
        ..config
    };
    if let Err(error) = insert(path, lang) {
        tracing::warn!(
            %error,
            path = %path.display(),
            "não foi possível gravar lang na configuração do projeto"
        );
    }
    config
}

/// Insere `lang` no JSON cru: reserializar `ProjectConfig` apagaria os campos
/// que a config ignora.
fn insert(path: &Path, lang: Lang) -> Result<(), ProjectConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ProjectConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&text).map_err(|source| {
        ProjectConfigError::Parse {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let serde_json::Value::Object(mut object) = value else {
        return Err(ProjectConfigError::NotAnObject {
            path: path.to_path_buf(),
        });
    };
    // Valor presente (inclusive escrito à mão) é intenção preservada.
    if object.get("lang").is_some_and(|value| !value.is_null()) {
        return Ok(());
    }
    object.insert(
        "lang".to_string(),
        serde_json::to_value(lang).map_err(|source| ProjectConfigError::Serialize {
            path: path.to_path_buf(),
            source,
        })?,
    );

    let mut text =
        serde_json::to_string_pretty(&serde_json::Value::Object(object)).map_err(|source| {
            ProjectConfigError::Serialize {
                path: path.to_path_buf(),
                source,
            }
        })?;
    text.push('\n');
    std::fs::write(path, text).map_err(|source| ProjectConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

impl Serialize for Lang {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D>(deserializer: D) -> Result<Lang, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Lang::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "lang inválido: {raw:?} precisa ser `pt-BR` ou `en`"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn parse(text: &str) -> Result<ProjectConfig, ProjectConfigError> {
        crate::project_config::parse_project(text, Path::new("ai.project.json"))
    }

    fn config(text: &str) -> ProjectConfig {
        parse(text).unwrap()
    }

    fn lang_from(pairs: &[(&str, &str)]) -> Lang {
        Lang::from_env(|key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        })
    }

    #[test]
    fn an_absent_lang_is_not_an_error_and_an_explicit_lang_is_read() {
        // Ausente vira `None` na leitura; quem preenche é `record`.
        assert_eq!(config(r#"{"name":"x"}"#).lang, None);
        assert_eq!(config(r#"{"name":"x","lang":"en"}"#).lang, Some(Lang::En));
        assert_eq!(
            config(r#"{"name":"x","lang":"pt-BR"}"#).lang,
            Some(Lang::PtBr)
        );
    }

    #[test]
    fn unknown_fields_still_parse_next_to_lang() {
        let parsed = config(r#"{"name":"x","future":true,"lang":"en"}"#);
        assert_eq!(parsed.name, "x");
        assert_eq!(parsed.lang, Some(Lang::En));
    }

    #[test]
    fn the_supported_spellings_are_normalized() {
        for text in ["pt-BR", "pt_br", "PT-br"] {
            assert_eq!(Lang::parse(text), Some(Lang::PtBr), "valor: {text}");
        }
        for text in ["en", "EN", "en_US"] {
            assert_eq!(Lang::parse(text), Some(Lang::En), "valor: {text}");
        }
        assert_eq!(Lang::parse("fr"), None);
    }

    #[test]
    fn an_invalid_lang_names_the_file_and_the_value() {
        let error = parse(r#"{"name":"x","lang":"fr"}"#).unwrap_err();
        assert!(
            matches!(error, ProjectConfigError::InvalidLang { .. }),
            "erro: {error:?}"
        );
        let message = error.to_string();
        assert!(message.contains("ai.project.json"), "mensagem: {message}");
        assert!(message.contains("fr"), "mensagem: {message}");
    }

    #[test]
    fn the_system_locale_resolves_to_a_supported_template() {
        assert_eq!(Lang::from_locale("pt_BR.UTF-8"), Lang::PtBr);
        assert_eq!(Lang::from_locale("pt-BR"), Lang::PtBr);
        assert_eq!(Lang::from_locale("en_US.UTF-8"), Lang::En);
        // Locale sem template equivalente cai em inglês.
        assert_eq!(Lang::from_locale("fr_FR.UTF-8"), Lang::En);
        assert_eq!(Lang::from_locale("C.UTF-8"), Lang::En);
        assert_eq!(Lang::from_locale(""), Lang::En);
    }

    #[test]
    fn lc_all_wins_over_lang_and_lang_wins_over_language() {
        assert_eq!(
            lang_from(&[("LC_ALL", "pt_BR.UTF-8"), ("LANG", "en_US.UTF-8")]),
            Lang::PtBr
        );
        assert_eq!(lang_from(&[("LANG", "pt_BR.UTF-8")]), Lang::PtBr);
        assert_eq!(lang_from(&[("LANGUAGE", "pt_BR:en")]), Lang::PtBr);
        // Valor vazio do LC_ALL não esconde o LANG.
        assert_eq!(
            lang_from(&[("LC_ALL", ""), ("LANG", "en_US.UTF-8")]),
            Lang::En
        );
        assert_eq!(lang_from(&[]), Lang::En);
    }

    #[test]
    fn an_absent_lang_is_written_back_preserving_the_other_fields() {
        let dir = tempdir().unwrap();
        let path = crate::project_config::project_path(dir.path());
        std::fs::write(
            &path,
            r#"{
  "name": "projeto",
  "future": 1,
  "docs_folders": ["wiki"],
  "models": {"openai/gpt": {"context_window": 8192}}
}"#,
        )
        .unwrap();

        let config = crate::project_config::load_or_create(dir.path());
        assert_eq!(config.lang, Some(Lang::from_system()));
        assert_eq!(config.name, "projeto");

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["lang"],
            serde_json::json!(Lang::from_system().as_str())
        );
        assert_eq!(value["name"], serde_json::json!("projeto"));
        assert_eq!(value["future"], serde_json::json!(1));
        assert_eq!(value["docs_folders"], serde_json::json!(["wiki"]));
        assert_eq!(
            value["models"]["openai/gpt"]["context_window"],
            serde_json::json!(8192)
        );
    }

    #[test]
    fn a_written_lang_is_not_written_again() {
        let dir = tempdir().unwrap();
        let path = crate::project_config::project_path(dir.path());
        std::fs::write(&path, r#"{"name":"projeto"}"#).unwrap();

        crate::project_config::load_or_create(dir.path());
        let text = std::fs::read_to_string(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        crate::project_config::load_or_create(dir.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn an_invalid_lang_falls_back_and_is_not_overwritten() {
        let dir = tempdir().unwrap();
        let path = crate::project_config::project_path(dir.path());
        let original = r#"{"name":"projeto","lang":"fr"}"#;
        std::fs::write(&path, original).unwrap();

        let config = crate::project_config::load_or_create(dir.path());
        assert_eq!(
            config.name,
            dir.path().file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn docs_folders_defaults_to_docs_and_an_explicit_list_replaces_it() {
        assert_eq!(config(r#"{"name":"x"}"#).docs_folders, vec!["docs"]);
        assert_eq!(
            config(r#"{"name":"x","docs_folders":[]}"#).docs_folders,
            vec!["docs"]
        );
        // A lista explícita substitui o default, nunca soma.
        assert_eq!(
            config(r#"{"name":"x","docs_folders":["wiki"]}"#).docs_folders,
            vec!["wiki"]
        );
    }
}
