//! Fusão de rankings lexicais e densos por Reciprocal Rank Fusion.

use std::collections::HashMap;

/// Funde dois rankings (índices 0-based) dando `dense_weight` ao denso.
///
/// `score[i] += peso / (k + rank + 1)`. Empates preservam a ordem de entrada.
pub(crate) fn rrf(
    lexical: &[usize],
    dense: &[usize],
    dense_weight: f32,
    k: f32,
    limit: usize,
) -> Vec<usize> {
    let mut scores: HashMap<usize, f32> = HashMap::new();
    for (rank, &index) in lexical.iter().enumerate() {
        *scores.entry(index).or_default() += 1.0 / (k + rank as f32 + 1.0);
    }
    for (rank, &index) in dense.iter().enumerate() {
        *scores.entry(index).or_default() += dense_weight / (k + rank as f32 + 1.0);
    }
    let mut ranked: Vec<(usize, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::rrf;

    #[test]
    fn dense_weight_breaks_a_first_place_tie() {
        assert_eq!(rrf(&[0], &[1], 3.0, 60.0, 2), vec![1, 0]);
    }

    #[test]
    fn an_item_in_both_lists_accumulates() {
        assert_eq!(rrf(&[7], &[7], 3.0, 60.0, 1), vec![7]);
    }

    #[test]
    fn empty_inputs_are_empty() {
        assert!(rrf(&[], &[], 3.0, 60.0, 5).is_empty());
    }

    #[test]
    fn limit_is_respected() {
        assert_eq!(rrf(&[0, 1, 2], &[], 3.0, 60.0, 2).len(), 2);
    }
}
