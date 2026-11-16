//! T2.3 — o contrato [`Decider`] exercitado por um decider falso.
//!
//! Estes testes rodam sem feature alguma: `decide` não depende de
//! `local-models` (nem de qualquer outra feature), então o slot vazio aceita
//! qualquer implementação e a borda (`crates/tupi`) é quem decide plugar o d1.
//! Fica fora de `src/decide.rs` de propósito: prova o contrato pela API pública,
//! do mesmo jeito que um consumidor futuro vai usá-lo.

use std::sync::Arc;

use tupi_kernel::decide::{Answer, ChoiceOption, DecideError, Decider, DeciderSlot, Question};

/// Decider falso: responde na ordem das questões, sem modelo nenhum.
struct Fake;

impl Decider for Fake {
    fn model_id(&self) -> &str {
        "fake-decider"
    }

    fn decide(&self, state: &str, questions: &[Question]) -> Result<Vec<Answer>, DecideError> {
        assert_eq!(state, "estado", "estado chega ao decider inteiro");
        Ok(questions
            .iter()
            .map(|question| match question {
                Question::Noul { prompt } => Answer::Noul {
                    value: prompt.contains("sim"),
                    confidence: 0.9,
                },
                Question::Choice { options, .. } => Answer::Choice {
                    id: options.first().map(|o| o.id.clone()).unwrap_or_default(),
                    confidence: 0.6,
                },
                Question::Score { levels, .. } => Answer::Score {
                    level: levels.len().saturating_sub(1),
                    confidence: 0.3,
                },
            })
            .collect())
    }
}

fn sample_questions() -> Vec<Question> {
    vec![
        Question::Noul {
            prompt: "o texto menciona cargo? sim ou não".to_string(),
        },
        Question::Choice {
            prompt: "qual critério descreve a mudança?".to_string(),
            options: vec![
                ChoiceOption::new("bug", "corrige comportamento errado"),
                ChoiceOption::new("feature", "adiciona capacidade nova"),
            ],
        },
        Question::Score {
            prompt: "quão arriscada é a mudança?".to_string(),
            levels: vec!["baixo".to_string(), "médio".to_string(), "alto".to_string()],
        },
    ]
}

#[test]
fn empty_slot_starts_without_decider() {
    let slot = DeciderSlot::empty();
    assert!(slot.get().is_none(), "slot vazio não tem decider");
}

#[test]
fn empty_slot_accepts_fake_decider() {
    let slot = DeciderSlot::empty();
    slot.set(Arc::new(Fake));

    let decider = slot.get().expect("decider registrado no slot");
    assert_eq!(decider.model_id(), "fake-decider");

    let preloaded = DeciderSlot::new(decider);
    assert_eq!(
        preloaded.get().expect("decider presente").model_id(),
        "fake-decider"
    );
}

#[test]
fn fake_decider_answers_in_question_order() {
    let slot = DeciderSlot::empty();
    slot.set(Arc::new(Fake));
    let decider = slot.get().expect("decider registrado no slot");

    let questions = sample_questions();
    let answers = decider.decide("estado", &questions).expect("respostas");

    assert_eq!(answers.len(), questions.len(), "uma resposta por questão");
    assert!(
        matches!(&answers[0], Answer::Noul { value: true, confidence } if (confidence - 0.9).abs() < f32::EPSILON),
        "primeira questão (noul) responde primeiro: {:?}",
        answers[0].confidence(),
    );
    assert!(
        matches!(&answers[1], Answer::Choice { id, confidence } if id == "bug" && (confidence - 0.6).abs() < f32::EPSILON),
        "segunda questão (choice) responde segundo: {:?}",
        answers[1].confidence(),
    );
    assert!(
        matches!(&answers[2], Answer::Score { level: 2, confidence } if (confidence - 0.3).abs() < f32::EPSILON),
        "terceira questão (score) responde terceiro: {:?}",
        answers[2].confidence(),
    );
}

#[test]
fn fake_decider_keeps_order_when_same_variant_repeats() {
    let slot = DeciderSlot::empty();
    slot.set(Arc::new(Fake));
    let decider = slot.get().expect("decider registrado no slot");

    let questions = [
        Question::Noul {
            prompt: "primeiro: sim?".to_string(),
        },
        Question::Noul {
            prompt: "segundo: não?".to_string(),
        },
    ];
    let answers = decider.decide("estado", &questions).expect("respostas");

    assert!(
        matches!(&answers[0], Answer::Noul { value: true, .. }),
        "a primeira pergunta recebe a primeira resposta"
    );
    assert!(
        matches!(&answers[1], Answer::Noul { value: false, .. }),
        "a segunda pergunta recebe a segunda resposta"
    );
}

#[test]
fn fake_decider_accepts_empty_question_list() {
    let slot = DeciderSlot::empty();
    slot.set(Arc::new(Fake));
    let decider = slot.get().expect("decider registrado no slot");

    let answers = decider.decide("estado", &[]).expect("respostas");
    assert!(answers.is_empty());
}

#[test]
fn decide_module_needs_no_local_models_feature() {
    let manifest = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    assert!(
        !manifest.contains("local-models"),
        "tupi-kernel não declara a feature local-models"
    );

    let module = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/decide.rs"));
    assert!(
        !module.contains("local-models") && !module.contains("local_models"),
        "decide.rs não referencia local-models"
    );
}
