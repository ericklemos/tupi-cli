//! Diarização: descobre quem fala em cada trecho do áudio.

use crate::merge::Span;
use std::path::Path;

/// Falhas da diarização.
#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    #[error("falha na diarização: {0}")]
    Sherpa(String),
}

/// Marca os trechos de fala de cada locutor, em centésimos de segundo.
///
/// `speakers` fixa o número de vozes; quando é `None`, o agrupamento usa o
/// threshold do sherpa (`num_clusters = -1`).
pub fn diarize_spans(
    samples: &[f32],
    segmentation: &Path,
    embedding: &Path,
    speakers: Option<i32>,
    cached: Option<&mut sherpa_rs::diarize::Diarize>,
) -> Result<Vec<Span>, DiarizeError> {
    let mut owned = None;
    let diarizer = match cached {
        Some(diarizer) => diarizer,
        None => {
            let diarizer = sherpa_rs::diarize::Diarize::new(
                segmentation,
                embedding,
                sherpa_rs::diarize::DiarizeConfig {
                    num_clusters: Some(speakers.unwrap_or(-1)),
                    ..Default::default()
                },
            )
            .map_err(|error| DiarizeError::Sherpa(error.to_string()))?;
            owned.insert(diarizer)
        }
    };

    let segments = diarizer
        .compute(samples.to_vec(), None)
        .map_err(|error| DiarizeError::Sherpa(error.to_string()))?;

    Ok(segments
        .into_iter()
        .map(|segment| Span {
            start_cs: (segment.start * 100.0) as i64,
            end_cs: (segment.end * 100.0) as i64,
            speaker: segment.speaker,
        })
        .collect())
}
