//! Subcomando `tupi transcribe`.

use crate::lang::Lang;
use std::path::{Path, PathBuf};
use tupi_transcribe::engine::Engine;
use tupi_transcribe::model::{self, ModelSpec};

/// Opções aceitas por `tupi transcribe`.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub files: Vec<PathBuf>,
    pub out: Option<PathBuf>,
    /// `None` significa automático: turbo com GPU/CUDA, small sem.
    pub model: Option<String>,
    pub language: String,
    pub speakers: Option<i32>,
    pub timestamps: bool,
}

/// Executa o subcomando e devolve o código de saída.
pub async fn run(args: &[String]) -> anyhow::Result<i32> {
    let options = parse(args).map_err(|error| anyhow::anyhow!(error))?;
    let root = crate::tool_root()?;
    let lang = crate::install::output_lang(&root);
    let outputs = plan_outputs(&options.files, options.out.as_deref())
        .map_err(|error| anyhow::anyhow!(error))?;

    let cuda_built = cfg!(feature = "cuda");
    let gpu_available = std::path::Path::new("/dev/nvidia0").exists()
        || std::process::Command::new("nvidia-smi")
            .arg("-L")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    let explicit_model = options.model.is_some();
    let mut model_name = options
        .model
        .clone()
        .unwrap_or_else(|| model::default_model(cuda_built, gpu_available).to_string());
    println!(
        "transcrevendo com o modelo {model_name} (cuda compilado: {cuda_built}, gpu do sistema: {gpu_available})"
    );
    println!("para usar outro modelo: --model base|small|medium|turbo|<caminho .bin>");

    let jobs = options
        .files
        .len()
        .min(
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
        )
        .clamp(1, 8);
    let contexts = if jobs > 1 { 2 } else { 1 };

    let models = crate::data_dir(|key| std::env::var(key).ok());
    let fetcher = tupi_infer::download::HttpFetcher;

    let segmentation = ensure(&models, model::segmentation_spec(), &fetcher).await?;
    let embedding = ensure(&models, model::embedding_spec(), &fetcher).await?;

    let engine_options = tupi_transcribe::engine::Options {
        segmentation_model: segmentation,
        embedding_model: embedding,
        language: if options.language == "auto" {
            None
        } else {
            Some(options.language.clone())
        },
        speakers: options.speakers,
        ffmpeg: PathBuf::from("ffmpeg"),
        threads: std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1),
    };

    let mut whisper_path = resolve_whisper(&model_name, &models, &fetcher).await?;
    let mut engine = Engine::load(&whisper_path, &engine_options, jobs, contexts)?;

    // O whisper.cpp só decide o backend ao carregar o contexto. `/dev/nvidia0` e
    // `nvidia-smi` podem enxergar a GPU e o backend CUDA ainda assim não subir —
    // e o modelo default de GPU em CPU é a pior combinação possível.
    if !explicit_model && !engine.gpu() {
        let fallback = model::default_model(false, false);
        if fallback != model_name {
            let note = engine.gpu_note().unwrap_or("backend GPU indisponível");
            eprintln!(
                "aviso: o whisper.cpp não inicializou a GPU ({note}); trocando {model_name} por {fallback} em CPU"
            );
            model_name = fallback.to_string();
            whisper_path = resolve_whisper(&model_name, &models, &fetcher).await?;
            engine = Engine::load(&whisper_path, &engine_options, jobs, contexts)?;
        }
    }
    println!(
        "backend do whisper: {}",
        if engine.gpu() { "gpu" } else { "cpu" }
    );
    if jobs > 1 {
        println!(
            "processando até {jobs} arquivos em paralelo ({} contexto(s) de whisper)",
            engine.contexts()
        );
    }

    let total = options.files.len();
    let mut dispatch = Vec::new();
    let mut dispatch_indices = Vec::new();
    let mut failures = Vec::new();
    for (index, audio) in options.files.iter().enumerate() {
        if !audio.exists() {
            eprintln!(
                "[{}/{}] {}: arquivo não encontrado",
                index + 1,
                total,
                audio.display()
            );
            failures.push((index, audio.clone(), "arquivo não encontrado".to_string()));
            continue;
        }
        let output = &outputs[index];
        if let Some(parent) = output.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        dispatch.push(audio.clone());
        dispatch_indices.push(index);
    }

    let done = std::sync::Mutex::new(Vec::<(usize, PathBuf)>::new());
    let failed = std::sync::Mutex::new(Vec::<(usize, PathBuf, String)>::new());
    let console = std::sync::Mutex::new(());

    {
        let report = |slot: usize,
                      outcome: Result<
            tupi_transcribe::engine::Outcome,
            tupi_transcribe::engine::EngineError,
        >| {
            let index = dispatch_indices[slot];
            let audio = &options.files[index];
            let _guard = console
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match outcome {
                Ok(outcome) => {
                    if let Some(warning) = &outcome.warning {
                        eprintln!("aviso: {warning}");
                    }
                    let text = tupi_transcribe::merge::render(&outcome.lines, options.timestamps);
                    match std::fs::write(&outputs[index], text) {
                        Ok(()) => {
                            println!("[{}/{}] {}", index + 1, total, audio.display());
                            println!(
                                "    decode {:.2}s · whisper {:.2}s · diarização {:.2}s",
                                outcome.decode.as_secs_f64(),
                                outcome.asr.as_secs_f64(),
                                outcome.diarize.as_secs_f64()
                            );
                            done.lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push((index, audio.clone()));
                        }
                        Err(error) => {
                            eprintln!("[{}/{}] {}: {error}", index + 1, total, audio.display());
                            failed
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push((index, audio.clone(), error.to_string()));
                        }
                    }
                }
                Err(error) => {
                    eprintln!("[{}/{}] {}: {error}", index + 1, total, audio.display());
                    failed
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push((index, audio.clone(), error.to_string()));
                }
            }
        };
        engine.transcribe_batch(&dispatch, &engine_options, &report);
    }

    let mut done = done.into_inner().unwrap_or_default();
    done.sort_by_key(|(index, _)| *index);
    let mut failed = failed.into_inner().unwrap_or_default();
    failed.extend(failures);
    failed.sort_by_key(|(index, _, _)| *index);

    let done: Vec<PathBuf> = done.into_iter().map(|(_, path)| path).collect();
    let failed: Vec<(PathBuf, String)> = failed
        .into_iter()
        .map(|(_, path, error)| (path, error))
        .collect();

    println!("{}", summary(&done, &failed, lang));
    Ok(if failed.is_empty() { 0 } else { 1 })
}

async fn resolve_whisper(
    requested: &str,
    models: &Path,
    fetcher: &tupi_infer::download::HttpFetcher,
) -> anyhow::Result<PathBuf> {
    let candidate = PathBuf::from(requested);
    if candidate.exists() {
        return Ok(candidate);
    }
    let spec = model::whisper_spec(requested).ok_or_else(|| {
        anyhow::anyhow!(
            "modelo desconhecido: {requested} (use base, small, medium, turbo ou um caminho .bin)"
        )
    })?;
    ensure(models, spec, fetcher).await
}

async fn ensure(
    models: &Path,
    spec: ModelSpec,
    fetcher: &tupi_infer::download::HttpFetcher,
) -> anyhow::Result<PathBuf> {
    Ok(tupi_infer::download::ensure_model(
        models,
        spec.repo,
        spec.file,
        spec.sha256,
        spec.bytes,
        fetcher,
    )
    .await
    .map_err(|error| anyhow::anyhow!("{}: {error}", spec.file))?)
}

/// Resumo final: o que passou e o que falhou, com motivo.
pub fn summary(done: &[PathBuf], failed: &[(PathBuf, String)], lang: Lang) -> String {
    let mut report = String::new();
    match lang {
        Lang::PtBr => {
            report.push_str(&format!("{} arquivo(s) transcrito(s):\n", done.len()));
            for file in done {
                report.push_str(&format!("- {}\n", file.display()));
            }
            if failed.is_empty() {
                report.push_str("tudo certo.\n");
            } else {
                report.push_str(&format!("{} falha(s):\n", failed.len()));
                for (file, reason) in failed {
                    report.push_str(&format!("- {}: {}\n", file.display(), reason));
                }
            }
        }
        Lang::En => {
            report.push_str(&format!("{} file(s) transcribed:\n", done.len()));
            for file in done {
                report.push_str(&format!("- {}\n", file.display()));
            }
            if failed.is_empty() {
                report.push_str("all good.\n");
            } else {
                report.push_str(&format!("{} failure(s):\n", failed.len()));
                for (file, reason) in failed {
                    report.push_str(&format!("- {}: {}\n", file.display(), reason));
                }
            }
        }
    }
    report
}

/// Interpreta os argumentos que vêm depois de `transcribe`.
pub fn parse(values: &[String]) -> Result<Options, String> {
    let mut options = Options {
        files: Vec::new(),
        out: None,
        model: None,
        language: "pt".to_string(),
        speakers: None,
        timestamps: false,
    };

    let mut index = 0;
    while index < values.len() {
        let value = values[index].as_str();
        match value {
            "--out" => {
                index += 1;
                options.out = Some(PathBuf::from(value_of(values, index, "--out")?));
            }
            "--model" => {
                index += 1;
                let requested = value_of(values, index, "--model")?;
                options.model = if requested == "auto" {
                    None
                } else {
                    Some(requested.to_string())
                };
            }
            "--language" => {
                index += 1;
                options.language = value_of(values, index, "--language")?.to_string();
            }
            "--speakers" => {
                index += 1;
                let raw = value_of(values, index, "--speakers")?;
                options.speakers = Some(
                    raw.parse()
                        .map_err(|_| format!("número de vozes inválido: {raw}"))?,
                );
            }
            "--timestamps" => options.timestamps = true,
            flag if flag.starts_with('-') => return Err(format!("opção desconhecida: {flag}")),
            file => options.files.push(PathBuf::from(file)),
        }
        index += 1;
    }

    if options.files.is_empty() {
        return Err("informe pelo menos um arquivo de áudio".to_string());
    }

    Ok(options)
}

fn value_of<'a>(values: &'a [String], index: usize, flag: &str) -> Result<&'a str, String> {
    values
        .get(index)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} exige um valor"))
}

/// Resolve o `.txt` de cada áudio e recusa duas entradas que gerariam o mesmo arquivo.
pub fn plan_outputs(files: &[PathBuf], out: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let multiple = files.len() > 1;

    if multiple
        && let Some(path) = out
        && path.exists()
        && !path.is_dir()
    {
        return Err(format!(
            "--out precisa ser um diretório quando há vários áudios: {}",
            path.display()
        ));
    }

    let outputs: Vec<PathBuf> = files
        .iter()
        .map(|file| output_path(file, out, multiple))
        .collect();

    for (index, path) in outputs.iter().enumerate() {
        if outputs[..index].contains(path) {
            return Err(format!(
                "dois áudios geram o mesmo arquivo ({}); use --out para separar",
                path.display()
            ));
        }
    }

    Ok(outputs)
}

fn output_path(file: &Path, out: Option<&Path>, multiple: bool) -> PathBuf {
    let name = file
        .file_stem()
        .map(Path::new)
        .unwrap_or_else(|| Path::new(""))
        .with_extension("txt");

    match out {
        Some(directory) if multiple || directory.is_dir() => directory.join(name),
        Some(path) => path.to_path_buf(),
        None => file.with_file_name(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn parse_ok(values: &[&str]) -> Options {
        parse(&args(values)).expect("deveria aceitar")
    }

    #[test]
    fn parses_several_files_and_flags() {
        let options = parse_ok(&["a.ptt", "b.m4a", "--timestamps", "--speakers", "2"]);

        assert_eq!(
            options.files,
            vec![PathBuf::from("a.ptt"), PathBuf::from("b.m4a")]
        );
        assert!(options.timestamps);
        assert_eq!(options.speakers, Some(2));
    }

    #[test]
    fn without_files_the_parse_fails() {
        let error = parse(&args(&["--timestamps"])).unwrap_err();

        assert!(error.contains("arquivo"));
    }

    #[test]
    fn an_unknown_flag_fails() {
        let error = parse(&args(&["a.ptt", "--nope"])).unwrap_err();

        assert!(error.contains("--nope"));
    }

    #[test]
    fn the_defaults_are_pt_and_auto() {
        let options = parse_ok(&["a.ptt"]);

        assert!(options.model.is_none());
        assert_eq!(options.language, "pt");
        assert_eq!(options.speakers, None);
        assert!(!options.timestamps);
    }

    #[test]
    fn out_with_several_inputs_must_be_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("saida.txt");
        std::fs::write(&target, "").expect("escrever");

        let error = plan_outputs(
            &[PathBuf::from("a.ptt"), PathBuf::from("b.ptt")],
            Some(&target),
        )
        .unwrap_err();

        assert!(error.contains("diretório"));
    }

    #[test]
    fn the_default_output_sits_next_to_the_audio() {
        let outputs = plan_outputs(&[PathBuf::from("dir/a.ptt")], None).unwrap();

        assert_eq!(outputs, vec![PathBuf::from("dir/a.txt")]);
    }

    #[test]
    fn out_directory_becomes_the_target_file() {
        let dir = tempfile::tempdir().expect("tempdir");

        let outputs = plan_outputs(&[PathBuf::from("a.ptt")], Some(dir.path())).unwrap();

        assert_eq!(outputs, vec![dir.path().join("a.txt")]);
    }

    #[test]
    fn two_inputs_with_the_same_stem_collide() {
        let error =
            plan_outputs(&[PathBuf::from("a.ptt"), PathBuf::from("a.m4a")], None).unwrap_err();

        assert!(error.contains("--out"));
    }

    #[test]
    fn the_summary_lists_each_failure_with_its_reason() {
        let done = vec![PathBuf::from("a.ptt")];
        let failed = vec![(PathBuf::from("b.ptt"), "sem fala".to_string())];

        let report = summary(&done, &failed, Lang::PtBr);

        assert!(report.contains("a.ptt"));
        assert!(report.contains("b.ptt"));
        assert!(report.contains("sem fala"));
    }

    #[test]
    fn a_clean_run_says_everything_worked() {
        let done = vec![PathBuf::from("a.ptt")];

        let report = summary(&done, &[], Lang::PtBr);

        assert!(report.contains("a.ptt"));
        assert!(!report.contains("falh"));
    }
}
