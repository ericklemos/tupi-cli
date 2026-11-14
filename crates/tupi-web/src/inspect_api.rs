use axum::extract::{Query as QueryParams, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use tupi_domain::memory::{Fact, Pending};

use crate::routes::AppState;

const DEFAULT_TELEMETRY_LIMIT: usize = 100;
const MAX_TELEMETRY_LIMIT: usize = 500;

#[derive(Serialize)]
pub(crate) struct MemoryDto {
    pub(crate) active_facts: Vec<Fact>,
    pub(crate) forgotten_facts: Vec<Fact>,
    pub(crate) open_pendings: Vec<Pending>,
    pub(crate) closed_pendings: Vec<Pending>,
}

pub(crate) async fn memory(State(state): State<AppState>) -> Json<MemoryDto> {
    let root = state.ctx.root.clone();
    let (active_facts, forgotten_facts) = fold_facts(tupi_domain::memory::records(&root))
        .into_iter()
        .partition(|fact| fact.live());
    let (open_pendings, closed_pendings) = tupi_domain::memory::pendings(&root)
        .into_iter()
        .partition(|pending| pending.open());

    Json(MemoryDto {
        active_facts,
        forgotten_facts,
        open_pendings,
        closed_pendings,
    })
}

#[derive(Serialize)]
pub(crate) struct StateDto {
    pub(crate) entries: Vec<tupi_tools::StateEntry>,
}

pub(crate) async fn state(State(state): State<AppState>) -> Json<StateDto> {
    let entries = tupi_tools::read_state_entries(&state.ctx.root, &state.ctx.state_dir);
    Json(StateDto { entries })
}

#[derive(Deserialize)]
pub(crate) struct TelemetryParams {
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct TelemetryDto {
    pub(crate) enabled: bool,
    pub(crate) path: Option<String>,
    pub(crate) stats: crate::routes::StatsDto,
    pub(crate) entries: Vec<serde_json::Value>,
}

pub(crate) async fn telemetry_recent(
    State(state): State<AppState>,
    QueryParams(params): QueryParams<TelemetryParams>,
) -> Json<TelemetryDto> {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_TELEMETRY_LIMIT)
        .clamp(1, MAX_TELEMETRY_LIMIT);
    let path = state.ctx.telemetry.path().map(|path| path.to_path_buf());

    let entries = match path.as_deref() {
        Some(path) => read_tail(path, limit).await,
        None => Vec::new(),
    };

    Json(TelemetryDto {
        enabled: state.ctx.telemetry.is_enabled(),
        path: path.map(|path| path.display().to_string()),
        stats: state.ctx.telemetry.stats().into(),
        entries,
    })
}

async fn read_tail(path: &std::path::Path, limit: usize) -> Vec<serde_json::Value> {
    let Ok(body) = tokio::fs::read_to_string(path).await else {
        return Vec::new();
    };
    let mut entries: Vec<serde_json::Value> = body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    entries
}

/// Mantém só o último registro de cada fato, como `memory::facts` faz:
/// o arquivo guarda o fato original e o tombstone do mesmo id.
fn fold_facts(records: Vec<Fact>) -> Vec<Fact> {
    let mut folded: Vec<Fact> = Vec::new();
    let mut index = std::collections::HashMap::<String, usize>::new();
    for fact in records {
        match index.get(&fact.id) {
            Some(&position) => folded[position] = fact,
            None => {
                index.insert(fact.id.clone(), folded.len());
                folded.push(fact);
            }
        }
    }
    folded
}
