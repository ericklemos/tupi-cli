//! Plano de investigação: pergunta → entidades, variantes e janela de tempo.

use serde::{Deserialize, Serialize};
use tupi_kernel::generate::Generator;

use super::error::ArchiveError;
use super::normalize::fold;

/// Uma entidade (pessoa ou assunto) e as formas que a representam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityPlan {
    pub label: String,
    pub forms: Vec<String>,
}

/// O que a investigação procura, e onde.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationPlan {
    pub question: String,
    pub entities: Vec<EntityPlan>,
    pub exclude: Vec<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub language: String,
}

impl InvestigationPlan {
    /// Hash estável: muda o plano, muda a chave de cache da adjudicação.
    pub fn hash(&self) -> String {
        let mut body = format!("{}|", self.question);
        for entity in &self.entities {
            body.push_str(&entity.label);
            for form in &entity.forms {
                body.push('|');
                body.push_str(form);
            }
        }
        for excluded in &self.exclude {
            body.push('-');
            body.push_str(excluded);
            body.push('|');
        }
        body.push_str(&format!("{:?}|{:?}|{}", self.from, self.to, self.language));
        blake3::hash(body.as_bytes()).to_hex().to_string()
    }

    /// Todas as formas, normalizadas e sem repetição.
    pub fn forms(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for entity in &self.entities {
            for form in &entity.forms {
                let form = fold(form);
                if !form.is_empty() && !out.contains(&form) {
                    out.push(form);
                }
            }
        }
        out
    }
}

#[derive(Deserialize)]
struct RawPlan {
    #[serde(default)]
    entities: Vec<RawEntity>,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Deserialize)]
struct RawEntity {
    label: String,
    #[serde(default)]
    forms: Vec<String>,
}

/// Expande a pergunta em variantes usando o gerador configurado.
pub fn build(
    question: &str,
    from: Option<i64>,
    to: Option<i64>,
    language: &str,
    generator: &dyn Generator,
) -> Result<InvestigationPlan, ArchiveError> {
    let prompt = format!(
        "Liste as formas pelas quais esta busca pode aparecer no acervo: grafias erradas provaveis, maiusculas, apelidos e sinonimos. Responda SOMENTE um JSON no formato {{\"entities\":[{{\"label\":\"nome\",\"forms\":[\"forma\"]}}],\"exclude\":[]}}. Idioma: {language}. Pergunta: {question}"
    );
    let raw = generator
        .generate(&prompt, 512)
        .map_err(|error| ArchiveError::Contract(error.to_string()))?;
    let parsed = parse_json::<RawPlan>(&raw)?;
    if parsed.entities.is_empty() {
        return Err(ArchiveError::Contract(
            "o plano veio sem entidades".to_string(),
        ));
    }
    let entities = parsed
        .entities
        .into_iter()
        .map(|entity| EntityPlan {
            label: entity.label,
            forms: entity.forms,
        })
        .collect();
    Ok(InvestigationPlan {
        question: question.to_string(),
        entities,
        exclude: parsed.exclude,
        from,
        to,
        language: language.to_string(),
    })
}

/// Extrai o JSON da resposta, ignorando cercas de código e prosa em volta.
pub(crate) fn parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, ArchiveError> {
    let trimmed = raw.trim();
    let start = trimmed
        .find(['{', '['])
        .ok_or_else(|| ArchiveError::Contract("saída sem JSON".to_string()))?;
    let end = trimmed
        .rfind(['}', ']'])
        .ok_or_else(|| ArchiveError::Contract("saída sem JSON".to_string()))?;
    if end < start {
        return Err(ArchiveError::Contract("JSON inválido".to_string()));
    }
    serde_json::from_str(&trimmed[start..=end])
        .map_err(|error| ArchiveError::Contract(format!("saída fora do contrato: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_kernel::generate::{GenerateError, Generator};

    struct Fake {
        text: String,
    }

    impl Generator for Fake {
        fn model_id(&self) -> &str {
            "fake"
        }
        fn generate(&self, _: &str, _: usize) -> Result<String, GenerateError> {
            Ok(self.text.clone())
        }
    }

    fn fake(text: &str) -> Fake {
        Fake {
            text: text.to_string(),
        }
    }

    #[test]
    fn build_parses_the_json_contract_and_hashes_the_plan() {
        let generator = fake(
            r#"{"entities":[{"label":"Joao","forms":["joao","joão","J. Silva"]}],"exclude":[]}"#,
        );
        let plan = build("quem falou do joao?", None, None, "pt-BR", &generator).unwrap();
        assert_eq!(plan.entities[0].forms.len(), 3);
        assert_eq!(plan.forms(), vec!["joao", "j. silva"]);
        assert_eq!(
            plan.hash(),
            build("quem falou do joao?", None, None, "pt-BR", &generator)
                .unwrap()
                .hash()
        );
    }

    #[test]
    fn prose_instead_of_json_is_a_contract_error() {
        let generator = fake("claro! vou procurar");
        assert!(matches!(
            build("x", None, None, "pt-BR", &generator),
            Err(ArchiveError::Contract(_))
        ));
    }
}
