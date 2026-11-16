//! Esteira por arquivo: decodifica, transcreve, diariza e funde.

use crate::asr::{Asr, AsrError};
use crate::decode::{DecodeError, decode_to_pcm};
use crate::diarize::diarize_spans;
use crate::merge::{Line, Span, lines_from_words};
use std::path::{Path, PathBuf};

/// Parâmetros de uma execução de `tupi transcribe`.
#[derive(Debug, Clone)]
pub struct Options {
    pub segmentation_model: PathBuf,
    pub embedding_model: PathBuf,
    pub language: Option<String>,
    pub speakers: Option<i32>,
    pub ffmpeg: PathBuf,
    pub threads: usize,
}

/// Falhas que impedem a transcrição de um arquivo.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Asr(#[from] AsrError),
}

/// Resultado da esteira para um arquivo.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub lines: Vec<Line>,
    /// Aviso não fatal (por exemplo, diarização que falhou e virou voz única).
    pub warning: Option<String>,
    /// Tempo gasto decodificando o áudio com o ffmpeg.
    pub decode: std::time::Duration,
    /// Tempo gasto no Whisper.
    pub asr: std::time::Duration,
    /// Tempo gasto na diarização.
    pub diarize: std::time::Duration,
}

/// Motor de transcrição: carrega o Whisper uma vez e reaproveita entre arquivos.
pub struct Engine {
    asr: Vec<Asr>,
    diarizers: Vec<sherpa_rs::diarize::Diarize>,
    diarization_warning: Option<String>,
    gpu: bool,
    gpu_note: Option<String>,
}

impl Engine {
    /// Carrega o modelo de transcrição e um diarizador por worker.
    ///
    /// `jobs` define quantas diarizações rodam ao mesmo tempo (o sherpa-onnx é
    /// single-thread) e `contexts` quantos contextos Whisper disputam a GPU.
    pub fn load(
        model: &Path,
        options: &Options,
        jobs: usize,
        contexts: usize,
    ) -> Result<Self, EngineError> {
        let (first, gpu_note) = Asr::load_detailed(model)?;
        let gpu = gpu_note.is_none();
        let mut asr = vec![first];
        if gpu {
            for _ in 1..contexts.max(1) {
                match Asr::load(model) {
                    Ok(extra) => asr.push(extra),
                    Err(_) => break,
                }
            }
        }

        let mut diarizers = Vec::new();
        let mut diarization_warning = None;
        for _ in 0..jobs.max(1) {
            match sherpa_rs::diarize::Diarize::new(
                &options.segmentation_model,
                &options.embedding_model,
                sherpa_rs::diarize::DiarizeConfig {
                    num_clusters: Some(options.speakers.unwrap_or(-1)),
                    ..Default::default()
                },
            ) {
                Ok(diarizer) => diarizers.push(diarizer),
                Err(error) => {
                    if diarization_warning.is_none() {
                        diarization_warning = Some(format!(
                            "diarização indisponível ({error}); tudo como Voz 1"
                        ));
                    }
                    break;
                }
            }
        }

        Ok(Self {
            asr,
            diarizers,
            diarization_warning,
            gpu,
            gpu_note,
        })
    }

    /// O whisper.cpp conseguiu inicializar o backend GPU?
    pub fn gpu(&self) -> bool {
        self.gpu
    }

    /// Por que o whisper.cpp caiu para CPU, quando caiu.
    pub fn gpu_note(&self) -> Option<&str> {
        self.gpu_note.as_deref()
    }

    /// Quantos contextos Whisper estão ativos (streams simultâneos na GPU).
    pub fn contexts(&self) -> usize {
        self.asr.len()
    }

    /// Transcreve um arquivo, separando os turnos por locutor.
    pub fn transcribe(&mut self, audio: &Path, options: &Options) -> Result<Outcome, EngineError> {
        Self::transcribe_one(
            &self.asr[0],
            self.diarizers.first_mut(),
            self.diarization_warning.as_deref(),
            audio,
            options,
        )
    }

    /// Processa um lote em dois estágios: os workers de Whisper (um por contexto
    /// de GPU) alimentam uma fila com palavras e tempos; os workers de
    /// diarização consomem a fila, decodificam o áudio e fecham cada arquivo.
    ///
    /// A fila carrega só palavras (poucos KB), então não acumula PCM na memória;
    /// em troca, o áudio é decodificado uma segunda vez no estágio de diarização.
    pub fn transcribe_batch(
        &mut self,
        files: &[PathBuf],
        options: &Options,
        report: &(dyn Fn(usize, Result<Outcome, EngineError>) + Sync),
    ) {
        if files.is_empty() {
            return;
        }
        if files.len() == 1 || self.diarizers.is_empty() {
            for (index, audio) in files.iter().enumerate() {
                let outcome = Self::transcribe_one(
                    &self.asr[0],
                    self.diarizers.first_mut(),
                    self.diarization_warning.as_deref(),
                    audio,
                    options,
                );
                report(index, outcome);
            }
            return;
        }

        struct Job {
            index: usize,
            path: PathBuf,
            words: Option<Vec<crate::merge::Word>>,
            decode: std::time::Duration,
            asr: std::time::Duration,
            error: Option<EngineError>,
        }

        let (input_sender, input_receiver) = std::sync::mpsc::channel::<(usize, &Path)>();
        for (index, audio) in files.iter().enumerate() {
            let _ = input_sender.send((index, audio.as_path()));
        }
        drop(input_sender);
        let input_receiver = std::sync::Mutex::new(input_receiver);

        let (job_sender, job_receiver) = std::sync::mpsc::channel::<Job>();
        let job_receiver = std::sync::Mutex::new(job_receiver);
        let asr_senders: Vec<_> = (0..self.asr.len()).map(|_| job_sender.clone()).collect();
        drop(job_sender);

        let warning = self.diarization_warning.as_deref();
        let asr_contexts = &self.asr;
        let diarizers = &mut self.diarizers;

        std::thread::scope(|scope| {
            for (asr, job_sender) in asr_contexts.iter().zip(asr_senders) {
                let input_receiver = &input_receiver;
                scope.spawn(move || {
                    loop {
                        let next = input_receiver
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .recv();
                        let Ok((index, audio)) = next else {
                            break;
                        };
                        let started = std::time::Instant::now();
                        let samples = match decode_to_pcm(&options.ffmpeg, audio) {
                            Ok(samples) => samples,
                            Err(error) => {
                                let _ = job_sender.send(Job {
                                    index,
                                    path: audio.to_path_buf(),
                                    words: None,
                                    decode: started.elapsed(),
                                    asr: std::time::Duration::ZERO,
                                    error: Some(error.into()),
                                });
                                continue;
                            }
                        };
                        let decode = started.elapsed();
                        let started = std::time::Instant::now();
                        let result =
                            asr.transcribe(&samples, options.language.as_deref(), options.threads);
                        let asr_time = started.elapsed();
                        let job = match result {
                            Ok(words) => Job {
                                index,
                                path: audio.to_path_buf(),
                                words: Some(words),
                                decode,
                                asr: asr_time,
                                error: None,
                            },
                            Err(error) => Job {
                                index,
                                path: audio.to_path_buf(),
                                words: None,
                                decode,
                                asr: asr_time,
                                error: Some(error.into()),
                            },
                        };
                        if job_sender.send(job).is_err() {
                            break;
                        }
                    }
                });
            }

            for diarizer in diarizers.iter_mut() {
                let job_receiver = &job_receiver;
                scope.spawn(move || {
                    loop {
                        let next = job_receiver
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .recv();
                        let Ok(job) = next else {
                            break;
                        };
                        let Some(words) = job.words else {
                            report(
                                job.index,
                                Err(job.error.unwrap_or_else(|| {
                                    EngineError::Asr(AsrError::Run(
                                        "transcrição falhou".to_string(),
                                    ))
                                })),
                            );
                            continue;
                        };
                        let started = std::time::Instant::now();
                        let samples = match decode_to_pcm(&options.ffmpeg, &job.path) {
                            Ok(samples) => samples,
                            Err(error) => {
                                report(job.index, Err(error.into()));
                                continue;
                            }
                        };
                        let second_decode = started.elapsed();
                        let started = std::time::Instant::now();
                        let (spans, outcome_warning) = match diarize_spans(
                            &samples,
                            &options.segmentation_model,
                            &options.embedding_model,
                            options.speakers,
                            Some(&mut *diarizer),
                        ) {
                            Ok(spans) => (usable_spans(spans), None),
                            Err(error) => (
                                usable_spans(Vec::new()),
                                Some(warning.map(str::to_string).unwrap_or_else(|| {
                                    format!("diarização indisponível ({error}); tudo como Voz 1")
                                })),
                            ),
                        };
                        let diarize = started.elapsed();
                        report(
                            job.index,
                            Ok(Outcome {
                                lines: lines_from_words(&words, &spans),
                                warning: outcome_warning,
                                decode: job.decode + second_decode,
                                asr: job.asr,
                                diarize,
                            }),
                        );
                    }
                });
            }
        });
    }

    fn transcribe_one(
        asr: &Asr,
        diarizer: Option<&mut sherpa_rs::diarize::Diarize>,
        startup_warning: Option<&str>,
        audio: &Path,
        options: &Options,
    ) -> Result<Outcome, EngineError> {
        let started = std::time::Instant::now();
        let samples = decode_to_pcm(&options.ffmpeg, audio)?;
        let decode = started.elapsed();

        let started = std::time::Instant::now();
        let words = asr.transcribe(&samples, options.language.as_deref(), options.threads)?;
        let asr_time = started.elapsed();

        let started = std::time::Instant::now();
        let (spans, warning) = match diarize_spans(
            &samples,
            &options.segmentation_model,
            &options.embedding_model,
            options.speakers,
            diarizer,
        ) {
            Ok(spans) => (usable_spans(spans), None),
            Err(error) => (
                usable_spans(Vec::new()),
                Some(startup_warning.map(str::to_string).unwrap_or_else(|| {
                    format!("diarização indisponível ({error}); tudo como Voz 1")
                })),
            ),
        };
        let diarize = started.elapsed();

        Ok(Outcome {
            lines: lines_from_words(&words, &spans),
            warning,
            decode,
            asr: asr_time,
            diarize,
        })
    }
}

/// Sem trechos de locutor, um único trecho cobre o áudio inteiro.
fn usable_spans(spans: Vec<Span>) -> Vec<Span> {
    if spans.is_empty() {
        vec![Span {
            start_cs: 0,
            end_cs: i64::MAX,
            speaker: 0,
        }]
    } else {
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_spans_everything_is_one_voice() {
        let spans = usable_spans(Vec::new());

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_cs, 0);
        assert_eq!(spans[0].end_cs, i64::MAX);
    }

    #[test]
    fn the_fallback_span_keeps_the_whole_text_in_one_line() {
        let words = crate::asr::words_from_tokens(&[
            crate::asr::RawToken {
                text: " ola".to_string(),
                t0_cs: 0,
                t1_cs: 10,
            },
            crate::asr::RawToken {
                text: " tudo".to_string(),
                t0_cs: 10,
                t1_cs: 20,
            },
        ]);

        let lines = lines_from_words(&words, &usable_spans(Vec::new()));

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "ola tudo");
    }

    #[test]
    fn existing_spans_are_kept_as_they_are() {
        let spans = vec![
            Span {
                start_cs: 0,
                end_cs: 100,
                speaker: 3,
            },
            Span {
                start_cs: 100,
                end_cs: 200,
                speaker: 5,
            },
        ];

        let kept = usable_spans(spans.clone());

        assert_eq!(kept, spans);
    }
}
