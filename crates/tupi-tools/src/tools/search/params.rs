use serde::Deserialize;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum SearchDetail {
    #[default]
    Auto,
    Full,
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
