//! Normalização de termos para a busca lexical da documentação.

use std::collections::{BTreeMap, HashSet};

/// Minúsculas e sem acentos, preservando o restante do texto.
pub fn normalize(text: &str) -> String {
    text.to_lowercase().chars().map(fold).collect()
}

fn fold(character: char) -> char {
    match character {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

/// Frequência dos termos normalizados, reusando a lista de stopwords do search.
pub fn terms(text: &str) -> BTreeMap<String, u16> {
    let normalized = normalize(text);
    let allowed: HashSet<String> =
        crate::support::terms::extract_terms(&normalized, &[]).into_iter().collect();
    let mut counts = BTreeMap::new();
    for token in normalized.split(|character: char| !character.is_alphanumeric() && character != '_') {
        if token.is_empty() || !allowed.contains(token) {
            continue;
        }
        *counts.entry(token.to_string()).or_insert(0) += 1;
    }
    counts
}

/// Termos únicos da consulta, na ordem em que aparecem.
#[allow(dead_code)]
pub fn query_terms(text: &str) -> Vec<String> {
    crate::support::terms::extract_terms(&normalize(text), &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_and_case_fold_to_the_same_term() {
        assert_eq!(normalize("Cobrança"), "cobranca");
        let counts = terms("Cobrança, cobranca!");
        assert_eq!(counts.get("cobranca"), Some(&2));
    }

    #[test]
    fn stopwords_and_single_letters_are_dropped() {
        let counts = terms("o a de cobrança");
        assert!(counts.get("de").is_none());
        assert!(counts.get("a").is_none());
        assert_eq!(counts.get("cobranca"), Some(&1));
    }
}
