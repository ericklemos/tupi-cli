use tupi_kernel::embed::{EmbedError, Embedder};

/// Embedder determinístico para testes: 4 dimensões, sem modelo real.
pub(crate) struct FakeEmbedder {
    model: String,
}

impl FakeEmbedder {
    pub fn named(model: &str) -> Self {
        Self {
            model: model.to_string(),
        }
    }

    pub fn truncate_query(&self, text: &str) -> String {
        text.split_whitespace()
            .take(self.max_query_tokens())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Default for FakeEmbedder {
    fn default() -> Self {
        Self::named("fake")
    }
}

impl Embedder for FakeEmbedder {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn dim(&self) -> usize {
        4
    }

    fn max_doc_tokens(&self) -> usize {
        512
    }

    fn max_query_tokens(&self) -> usize {
        32
    }

    fn token_len(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|text| fake_vec(text)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(fake_vec(&self.truncate_query(text)))
    }
}

fn fake_vec(text: &str) -> Vec<f32> {
    let mut buckets = [0f32; 4];
    for word in crate::tools::docs::terms::normalize(text)
        .split(|character: char| !character.is_alphanumeric() && character != '_')
    {
        if word.is_empty() {
            continue;
        }
        let word = match word {
            "carro" | "automovel" => "veiculo",
            other => other,
        };
        let index = word.bytes().fold(0usize, |acc, byte| acc + byte as usize) % 4;
        buckets[index] += 1.0;
    }
    buckets.to_vec()
}
