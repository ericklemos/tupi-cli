//! Contrato do modelo local de decisão (`d1`).
//!
//! `d1` não é chat: a questão inteira cabe num único batch e o modelo devolve
//! uma etiqueta direto do forward pass, sem gerar tokens. Por isso ele não
//! encaixa em [`Generator`](crate::generate::Generator) nem em
//! [`Embedder`](crate::embed::Embedder) — daí os tipos próprios abaixo.

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Alternativa de uma [`Question::Choice`]: id estável + descrição legível.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceOption {
    /// Identificador devolvido em [`Answer::Choice`].
    pub id: String,
    /// Texto enviado ao modelo junto com o id.
    pub description: String,
}

impl ChoiceOption {
    /// Monta uma alternativa a partir de id e descrição.
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
        }
    }
}

/// Questão submetida ao modelo de decisão em uma única passada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Question {
    /// Sim/não sobre um texto.
    Noul {
        /// Texto da pergunta, enviado inteiro ao modelo.
        prompt: String,
    },
    /// Escolha de um entre vários critérios (id + descrição).
    Choice {
        /// Texto da pergunta, enviado inteiro ao modelo.
        prompt: String,
        /// Alternativas candidatas, na ordem em que chegam ao modelo.
        options: Vec<ChoiceOption>,
    },
    /// Nível ordinal numa escala declarada.
    Score {
        /// Texto da pergunta, enviado inteiro ao modelo.
        prompt: String,
        /// Rótulos da escala, do menor para o maior nível.
        levels: Vec<String>,
    },
}

/// Resposta do modelo de decisão, sempre acompanhada de confiança calibrada.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Resposta booleana para [`Question::Noul`].
    Noul {
        /// Valor respondido.
        value: bool,
        /// Confiança calibrada em `0.0..=1.0`.
        confidence: f32,
    },
    /// Alternativa escolhida para [`Question::Choice`].
    Choice {
        /// Id da alternativa vencedora.
        id: String,
        /// Confiança calibrada em `0.0..=1.0`.
        confidence: f32,
    },
    /// Nível escolhido para [`Question::Score`].
    Score {
        /// Índice do nível vencedor na escala da questão.
        level: usize,
        /// Confiança calibrada em `0.0..=1.0`.
        confidence: f32,
    },
}

impl Answer {
    /// Confiança calibrada da resposta, independente da variante.
    pub fn confidence(&self) -> f32 {
        match self {
            Self::Noul { confidence, .. }
            | Self::Choice { confidence, .. }
            | Self::Score { confidence, .. } => *confidence,
        }
    }
}

/// Modelo de decisão: responde todas as questões de uma vez, sem gerar texto.
///
/// Diferente de [`Generator`](crate::generate::Generator), o d1 consome o
/// estado e o lote de [`Question`] num único forward pass e devolve uma
/// [`Answer`] por questão, com confiança calibrada.
pub trait Decider: Send + Sync {
    /// Identificador estável do modelo.
    fn model_id(&self) -> &str;
    /// Responde `questions` contra `state` numa única passada.
    ///
    /// A ordem das respostas acompanha a ordem das questões.
    fn decide(&self, state: &str, questions: &[Question]) -> Result<Vec<Answer>, DecideError>;
}

/// Falhas do contrato de decisão.
#[derive(Debug, thiserror::Error)]
pub enum DecideError {
    #[error("modelo indisponível: {0}")]
    Unavailable(String),
    #[error("falha na inferência: {0}")]
    Inference(String),
    #[error("questão sem suporte no modelo: {0}")]
    Unsupported(String),
}

/// Slot compartilhado: o load sob demanda preenche, o consumidor lê.
#[derive(Default)]
pub struct DeciderSlot(RwLock<Option<Arc<dyn Decider>>>);

impl DeciderSlot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(decider: Arc<dyn Decider>) -> Self {
        Self(RwLock::new(Some(decider)))
    }

    pub fn get(&self) -> Option<Arc<dyn Decider>> {
        self.0
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn set(&self, decider: Arc<dyn Decider>) {
        *self.0.write().unwrap_or_else(|error| error.into_inner()) = Some(decider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(question: &Question, answer: &Answer) {
        let question_json = serde_json::to_string(question).expect("question serialize");
        let answer_json = serde_json::to_string(answer).expect("answer serialize");
        let question_back: Question = serde_json::from_str(&question_json).expect("question revive");
        let answer_back: Answer = serde_json::from_str(&answer_json).expect("answer revive");
        assert_eq!(&question_back, question);
        assert_eq!(&answer_back, answer);
    }

    #[test]
    fn noul_round_trips_through_json() {
        let question = Question::Noul {
            prompt: "o texto fala de Rust?".to_string(),
        };
        let answer = Answer::Noul {
            value: true,
            confidence: 0.92,
        };
        round_trip(&question, &answer);
        assert_eq!(answer.confidence(), 0.92);
    }

    #[test]
    fn choice_round_trips_through_json() {
        let question = Question::Choice {
            prompt: "qual critério descreve a mudança?".to_string(),
            options: vec![
                ChoiceOption::new("bug", "corrige comportamento errado"),
                ChoiceOption::new("feature", "adiciona capacidade nova"),
            ],
        };
        let answer = Answer::Choice {
            id: "feature".to_string(),
            confidence: 0.75,
        };
        round_trip(&question, &answer);
        assert_eq!(answer.confidence(), 0.75);
    }

    #[test]
    fn score_round_trips_through_json() {
        let question = Question::Score {
            prompt: "quão arriscada é a mudança?".to_string(),
            levels: vec!["baixo".to_string(), "médio".to_string(), "alto".to_string()],
        };
        let answer = Answer::Score {
            level: 2,
            confidence: 0.4,
        };
        round_trip(&question, &answer);
        assert_eq!(answer.confidence(), 0.4);
    }

    #[test]
    fn variants_serialize_with_stable_tags() {
        let noul = Question::Noul {
            prompt: "p".to_string(),
        };
        let choice = Question::Choice {
            prompt: "p".to_string(),
            options: Vec::new(),
        };
        let score = Question::Score {
            prompt: "p".to_string(),
            levels: Vec::new(),
        };
        assert!(serde_json::to_string(&noul).unwrap().contains("\"noul\""));
        assert!(serde_json::to_string(&choice).unwrap().contains("\"choice\""));
        assert!(serde_json::to_string(&score).unwrap().contains("\"score\""));

        let answer = Answer::Noul {
            value: false,
            confidence: 0.0,
        };
        assert!(serde_json::to_string(&answer).unwrap().contains("\"noul\""));
    }

    #[test]
    fn slot_starts_empty_and_accepts_one_decider() {
        struct Fake;
        impl Decider for Fake {
            fn model_id(&self) -> &str {
                "fake-decider"
            }

            fn decide(
                &self,
                _state: &str,
                questions: &[Question],
            ) -> Result<Vec<Answer>, DecideError> {
                Ok(questions
                    .iter()
                    .map(|_| Answer::Noul {
                        value: true,
                        confidence: 0.5,
                    })
                    .collect())
            }
        }

        let slot = DeciderSlot::empty();
        assert!(slot.get().is_none());
        slot.set(std::sync::Arc::new(Fake));
        let decider = slot.get().expect("decider presente");
        assert_eq!(decider.model_id(), "fake-decider");

        let questions = [Question::Noul {
            prompt: "existe?".to_string(),
        }];
        let answers = decider.decide("estado", &questions).expect("resposta");
        assert_eq!(answers.len(), questions.len());

        let preloaded = DeciderSlot::new(decider);
        assert_eq!(preloaded.get().expect("presente").model_id(), "fake-decider");
    }
}
