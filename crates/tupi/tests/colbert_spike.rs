use std::path::PathBuf;
use std::time::Instant;

use tupi_infer::{HttpFetcher, LlamaColBert, ensure_model, maxsim};

const REPO: &str = "LiquidAI/LFM2.5-ColBERT-350M-GGUF";
const FILE: &str = "LFM2.5-ColBERT-350M-Q8_0.gguf";
const SHA256: &str = "5765e4987f1e63e6c52572198abd65fe9616416edf523c0fa95a077a3e501d47";
const BYTES: u64 = 378_097_248;

#[tokio::test]
async fn colbert_spike_ranks_the_panda_document() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let flag = root.join(".superpowers/bakeoff.flag");
    if !flag.exists() {
        return;
    }
    let home = std::env::var("HOME").expect("HOME");
    let base = PathBuf::from(home).join(".tupi");
    let path = ensure_model(&base, REPO, FILE, SHA256, BYTES, &HttpFetcher)
        .await
        .expect("modelo ColBERT");
    let model = LlamaColBert::load(&path).expect("carrega ColBERT");
    let query = model.embed_tokens("What is panda?", true).expect("query");
    let docs = [
        "hi",
        "it is a bear",
        "The giant panda (Ailuropoda melanoleuca), sometimes called a panda bear or simply panda, is a bear species endemic to China.",
    ];
    let started = Instant::now();
    let mut scores = Vec::new();
    for doc in docs {
        let vectors = model.embed_tokens(doc, false).expect("documento");
        scores.push((doc.to_string(), maxsim(&query, &vectors), vectors.len()));
    }
    let elapsed = started.elapsed();
    let report = format!(
        "# ColBERT spike\n\n- query vectors: {}\n- dim: {}\n- tempo (3 docs): {:?}\n\n{}
",
        query.len(),
        query.first().map(Vec::len).unwrap_or(0),
        elapsed,
        scores
            .iter()
            .map(|(doc, score, tokens)| format!("- score {score:.2} · {tokens} tokens · {doc}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    std::fs::write(root.join(".superpowers/bakeoff-colbert.md"), &report).expect("relatorio");
    assert_eq!(query.first().map(Vec::len), Some(128), "{report}");
    assert!(scores[2].1 > scores[1].1, "{report}");
    assert!(scores[1].1 > scores[0].1, "{report}");
    let _ = std::fs::remove_file(flag);
}
