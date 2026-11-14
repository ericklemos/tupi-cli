use std::collections::HashSet;

/// Stopwords conservadoras de PT/EN. Curtas de propósito: remover uma palavra
/// que era termo custa mais que manter uma que não era.
const STOPWORDS: &[&str] = &[
    "a", "as", "o", "os", "um", "uma", "uns", "umas", "de", "da", "das", "do", "dos", "em", "na",
    "nas", "no", "nos", "por", "para", "pra", "com", "sem", "e", "ou", "que", "como", "onde",
    "quando", "qual", "quais", "ser", "sou", "sao", "são", "esta", "está", "este", "isto", "isso",
    "aquilo", "mais", "menos", "muito", "tem", "ter", "foi", "era", "the", "an", "and", "or", "of",
    "to", "in", "on", "at", "by", "for", "with", "from", "is", "are", "was", "were", "be", "been",
    "this", "that", "these", "those", "it", "its", "as", "how", "where", "when", "what", "which",
    "who", "does", "do", "did", "a", "an", "can", "could", "should", "would",
];

/// Termos que a busca usa: keywords explícitas primeiro, depois as palavras da
/// frase sem stopwords. Tudo minúsculo e deduplicado, preservando a ordem.
pub fn extract_terms(query: &str, keywords: &[String]) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for keyword in keywords {
        let term = keyword.trim().to_lowercase();
        if !term.is_empty() && seen.insert(term.clone()) {
            terms.push(term);
        }
    }

    for word in query.split(|character: char| !character.is_alphanumeric() && character != '_') {
        let term = word.trim().to_lowercase();
        if term.chars().count() < 2 || STOPWORDS.contains(&term.as_str()) {
            continue;
        }
        if seen.insert(term.clone()) {
            terms.push(term);
        }
    }

    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwords_are_dropped_from_a_natural_phrase() {
        assert_eq!(
            extract_terms("onde o token é validado", &[]),
            vec!["token".to_string(), "validado".to_string()]
        );
    }

    #[test]
    fn explicit_keywords_come_first_and_are_deduplicated() {
        assert_eq!(
            extract_terms("valida token", &["Verify_Token".to_string()]),
            vec![
                "verify_token".to_string(),
                "valida".to_string(),
                "token".to_string()
            ]
        );
    }

    #[test]
    fn an_explicit_stopword_keyword_is_kept() {
        assert_eq!(
            extract_terms("", &["if".to_string()]),
            vec!["if".to_string()]
        );
    }

    #[test]
    fn empty_or_stopword_only_phrases_produce_no_terms() {
        assert!(extract_terms("", &[]).is_empty());
        assert!(extract_terms("onde o é a", &[]).is_empty());
    }
}
