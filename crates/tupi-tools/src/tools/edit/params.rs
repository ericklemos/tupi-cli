use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct EditParams {
    pub path: String,
    pub symbol: String,
    pub expected_hash: String,
    pub new_body: String,
    pub signature: Option<String>,
    pub budget_tokens: Option<usize>,
}
