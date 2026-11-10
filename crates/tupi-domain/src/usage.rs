//! Consumo de tokens de um turno, como o provedor reporta.

/// Consumo somado sobre todos os passos de tool call de um turno.
///
/// `prompt_tokens` é tudo o que entrou no modelo: system prompt, specs das
/// tools, histórico e resultados de tool. `cached_tokens` é a fatia que o
/// provedor cobrou mais barato; `cached_ratio` diz se o contexto está sendo
/// reaproveitado ou reconstruído a cada passo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub completion_tokens: u64,
}

impl Usage {
    pub fn new(prompt_tokens: u64, cached_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            cached_tokens,
            completion_tokens,
        }
    }

    /// Soma outro passo do mesmo turno: cada passo é uma requisição cobrada.
    pub fn absorb(&mut self, other: Usage) {
        self.prompt_tokens += other.prompt_tokens;
        self.cached_tokens += other.cached_tokens;
        self.completion_tokens += other.completion_tokens;
    }

    pub fn is_empty(&self) -> bool {
        self.prompt_tokens == 0 && self.cached_tokens == 0 && self.completion_tokens == 0
    }

    /// Fração do prompt que veio do cache do provedor, de 0.0 a 1.0.
    pub fn cached_ratio(&self) -> f64 {
        if self.prompt_tokens == 0 {
            0.0
        } else {
            self.cached_tokens as f64 / self.prompt_tokens as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absorb_sums_every_field() {
        let mut total = Usage::new(100, 40, 10);
        total.absorb(Usage::new(200, 160, 20));

        assert_eq!(total, Usage::new(300, 200, 30));
    }

    #[test]
    fn cached_ratio_is_zero_without_prompt_tokens() {
        assert_eq!(Usage::default().cached_ratio(), 0.0);
    }

    #[test]
    fn cached_ratio_reports_the_cached_share() {
        assert!((Usage::new(1_000, 900, 5).cached_ratio() - 0.9).abs() < 1e-9);
    }

    #[test]
    fn a_default_usage_is_empty() {
        assert!(Usage::default().is_empty());
        assert!(!Usage::new(1, 0, 0).is_empty());
    }
}
