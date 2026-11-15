use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchDetail {
    #[default]
    #[serde(alias = "Auto")]
    Auto,
    #[serde(alias = "Full")]
    Full,
    #[serde(alias = "Signatures")]
    Signatures,
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_results: Option<usize>,
    #[serde(default)]
    pub detail: Option<SearchDetail>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    #[serde(default)]
    pub repeat: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(detail: &str) -> SearchParams {
        serde_json::from_value(serde_json::json!({ "query": "x", "detail": detail })).unwrap()
    }

    #[test]
    fn detail_accepts_the_schema_lowercase_and_pascal_case() {
        assert_eq!(params("signatures").detail, Some(SearchDetail::Signatures));
        assert_eq!(params("Signatures").detail, Some(SearchDetail::Signatures));
        assert_eq!(params("auto").detail, Some(SearchDetail::Auto));
    }

    #[test]
    fn detail_rejects_an_unknown_variant() {
        assert!(
            serde_json::from_value::<SearchParams>(
                serde_json::json!({ "query": "x", "detail": "signature" })
            )
            .is_err()
        );
    }
}
