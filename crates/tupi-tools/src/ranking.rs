use crate::ast::AstSnippet;

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct ScoredSnippet {
    pub snippet: AstSnippet,
    pub score: f32,
}

#[derive(Default)]
pub struct LexicalRanker;

impl LexicalRanker {
    pub fn new() -> Self {
        Self
    }

    /// Ordena por relevância lexical, com empate preservando a ordem de entrada.
    #[allow(dead_code)]
    pub fn rank(&self, terms: &[String], snippets: Vec<AstSnippet>) -> Vec<ScoredSnippet> {
        let scores = score(terms, &snippets);
        let mut scored: Vec<ScoredSnippet> = snippets
            .into_iter()
            .zip(scores)
            .map(|(snippet, score)| ScoredSnippet { snippet, score })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored
    }

    /// Mesmo ranking, devolvendo índices sobre o slice original. Permite ao
    /// chamador carregar o caminho do candidato sem duplicar o snippet.
    pub fn rank_indices(&self, terms: &[String], snippets: &[AstSnippet]) -> Vec<(usize, f32)> {
        let scores = score(terms, snippets);
        let mut ranked: Vec<(usize, f32)> = scores.into_iter().enumerate().collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked
    }
}

fn score(terms: &[String], snippets: &[AstSnippet]) -> Vec<f32> {
    let mut scores: Vec<f32> = snippets
        .iter()
        .map(|snippet| {
            let symbol_lower = snippet.symbol_name.to_lowercase();
            let signature_lower = snippet.signature.to_lowercase();

            let mut raw = 0.0f32;
            for term in terms {
                if symbol_lower == *term {
                    raw += 10.0;
                } else if symbol_lower.contains(term) {
                    raw += 5.0;
                }

                if signature_lower.contains(term) {
                    raw += 3.0;
                }

                let occurrences = count_occurrences(&snippet.code, term);
                if occurrences > 0 {
                    raw += (occurrences as f32).min(5.0) * 0.8;
                }
            }
            raw
        })
        .collect();

    let max = scores.iter().copied().fold(0.0f32, f32::max);
    if max > 0.0 {
        for score in &mut scores {
            *score = (*score / max).clamp(0.0, 1.0);
        }
    } else {
        scores.fill(0.5);
    }
    scores
}

/// Contagem sem sobreposição e case-insensitive. Termos não-ASCII caem no
/// caminho que aloca, para o matching Unicode continuar correto.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    if !needle.is_ascii() {
        return haystack
            .to_lowercase()
            .matches(&needle.to_lowercase())
            .count();
    }

    let bytes = haystack.as_bytes();
    let pattern = needle.as_bytes();
    let mut count = 0;
    let mut at = 0;
    while at + pattern.len() <= bytes.len() {
        if bytes[at..at + pattern.len()].eq_ignore_ascii_case(pattern) {
            count += 1;
            at += pattern.len();
        } else {
            at += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snippet(name: &str, signature: &str, code: &str) -> AstSnippet {
        AstSnippet {
            symbol_name: name.to_string(),
            qualified_name: name.to_string(),
            signature: signature.to_string(),
            code: code.to_string(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn terms(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn a_name_hit_beats_a_body_hit() {
        let name_hit = snippet("verify_token", "fn verify_token()", "");
        let body_hit = snippet("other", "fn other()", "verify_token verify_token");

        let ranked = LexicalRanker::new().rank(&terms(&["verify_token"]), vec![body_hit, name_hit]);

        assert_eq!(ranked[0].snippet.symbol_name, "verify_token");
        assert_eq!(ranked[0].score, 1.0);
    }

    #[test]
    fn a_signature_hit_beats_a_single_body_occurrence() {
        let signature_hit = snippet("other", "fn verify_token()", "");
        let body_hit = snippet("another", "fn another()", "verify_token");

        let ranked =
            LexicalRanker::new().rank(&terms(&["verify_token"]), vec![body_hit, signature_hit]);

        assert_eq!(ranked[0].snippet.symbol_name, "other");
    }

    #[test]
    fn ties_keep_the_input_order() {
        let first = snippet("a", "fn a()", "token");
        let second = snippet("b", "fn b()", "token");

        let ranked = LexicalRanker::new().rank(&terms(&["token"]), vec![first, second]);

        assert_eq!(ranked[0].snippet.symbol_name, "a");
        assert_eq!(ranked[1].snippet.symbol_name, "b");
    }

    #[test]
    fn empty_inputs_are_empty() {
        let ranker = LexicalRanker::new();
        assert!(ranker.rank(&terms(&["x"]), Vec::new()).is_empty());
        let one = vec![snippet("a", "", "")];
        assert_eq!(ranker.rank(&[], one)[0].score, 0.5);
    }

    #[test]
    fn occurrence_count_is_case_insensitive_and_non_overlapping() {
        assert_eq!(count_occurrences("abab", "ab"), 2);
        assert_eq!(count_occurrences("ABab", "ab"), 2);
        assert_eq!(count_occurrences("çãoção", "ção"), 2);
        assert_eq!(count_occurrences("", "x"), 0);
    }
}
