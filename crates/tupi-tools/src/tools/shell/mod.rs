use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::proc;
use crate::store::ResponseStore;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Deserialize)]
pub struct ShellParams {
    pub command: String,
    /// Termos do filtro: substring, case-insensitive, OR entre eles.
    /// Só filtram com `filter: true` explícito; sem isso a saída volta inteira.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Liga o filtro duro de `terms`. Ausente ou falso deixa a saída intacta.
    #[serde(default)]
    pub filter: Option<bool>,
    /// Ranking mole dos blocos por relevancia lexical.
    #[serde(default)]
    pub question: Option<String>,
    /// Linhas de contexto em volta de cada casamento.
    #[serde(default)]
    pub context: Option<usize>,
    /// Teto de blocos devolvidos.
    #[serde(default)]
    pub max_blocks: Option<usize>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct ShellTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl ShellTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

/// bash -lc sem sandbox. So formatador entra: qualquer composicao de
/// comandos e recusada antes de chegar ao shell.
pub(crate) fn allowed(command: &str, allow: &[String]) -> bool {
    /// Programas que executam outro comando: liberar um liberaria tudo.
    const WRAPPERS: &[&str] = &[
        "sh", "bash", "zsh", "dash", "fish", "env", "xargs", "sudo", "doas", "su", "nohup",
        "timeout", "nice", "watch", "parallel", "eval", "exec", "python", "python3", "node",
        "deno", "bun", "perl", "ruby", "php", "lua", "make", "just",
    ];

    /// Espelho dos binarios de `ShellTool::call::NATIVE`: este portao roda fora
    /// do `call`, entao repete os nomes para recusar antes da allowlist.
    const NATIVE_BINARIES: &[&str] = &[
        "cat", "head", "tail", "sed", "grep", "rg", "awk", "find", "ls", "tree", "wc", "sha256sum",
        "sha1sum", "shasum", "md5sum", "curl", "wget", "rm", "mv", "cp", "touch",
    ];

    /// Globais que so tiram ruido: nenhum deles muda de diretorio nem abre pager.
    const GIT_HARMLESS: &[&str] = &["--no-pager", "--no-optional-locks"];

    /// Subcomandos de leitura do git: escrever historico (checkout, commit,
    /// reset, add) fica de fora mesmo quando o binario esta liberado.
    const GIT_READ: &[&str] = &[
        "status",
        "diff",
        "log",
        "show",
        "blame",
        "annotate",
        "grep",
        "ls-files",
        "ls-tree",
        "rev-parse",
        "rev-list",
        "describe",
        "shortlog",
        "cat-file",
        "show-ref",
        "name-rev",
        "whatchanged",
        "diff-tree",
        "diff-files",
        "diff-index",
        "for-each-ref",
        "merge-base",
        "show-branch",
        "check-ignore",
        "check-attr",
        "version",
        "help",
    ];

    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if composes(trimmed) || trimmed.contains("--manifest-path") {
        return false;
    }

    // Formatador e o contrato antigo da tool: vale com qualquer lista.
    if trimmed == "cargo fmt" || trimmed.starts_with("cargo fmt ") {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("rustfmt") {
        let rest = rest.trim();
        if rest.is_empty() || rest.split_whitespace().all(|token| token.starts_with('-')) {
            return true;
        }
    }

    let mut tokens = trimmed.split_whitespace();
    let program = tokens
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let args: Vec<&str> = tokens.collect();

    // Fora `cargo fmt`, ja liberado acima, o cargo fica com o tool exec, que roda
    // build/test/check com os alvos declarados: o shell nao vira um `cargo run`.
    if program == "cargo" {
        return false;
    }

    if program == "git" {
        // git -c e --config-env aceitam alias que rodam comando: a allowlist do
        // binario seria contornada por dentro dele.
        if args
            .iter()
            .any(|arg| arg.starts_with("-c") || arg.starts_with("--config-env"))
        {
            return false;
        }
        // So leitura, e sem -C/--git-dir/--work-tree: o subcomando precisa ser o
        // primeiro argumento util, entao nenhum global com valor passa adiante.
        let sub = args.iter().copied().find(|arg| !GIT_HARMLESS.contains(arg));
        match sub {
            Some(sub) if GIT_READ.contains(&sub) => {}
            _ => return false,
        }
    }

    // Binario com tool equivalente nao volta por shell.allow: a recusa e do
    // binario, nao da lista do projeto (mesmo contrato do `cargo fmt`).
    if NATIVE_BINARIES.contains(&program) {
        return false;
    }

    !WRAPPERS.contains(&program) && allow.iter().any(|name| name == program)
}

/// Composicao de shell: mais de um comando numa linha, redirecionamento ou
/// substituicao por crase. Nenhum desses caracteres cabe num comando unico.
fn composes(command: &str) -> bool {
    command
        .chars()
        .any(|c| matches!(c, ';' | '&' | '|' | '>' | '<' | '$' | '\n' | '\r' | '\u{0060}'))
}

impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: "Roda um comando no root e devolve a saida. Por padrao a saida bruta volta inteira; terms so filtra com filter: true, e entao o cabecalho declara quantas linhas o filtro omitiu. O binario precisa estar na allowlist do projeto (ai.project.json, chave shell.allow); composicao de comandos nao e permitida. Comando que tem tool equivalente (grep/rg, cat/head/tail, ls/find/tree, sed, rm/mv/cp, curl/wget, sha256sum) e recusado sempre, mesmo listado em shell.allow: a recusa ensina a tool que faz o mesmo. Fora `cargo fmt`, cargo pertence ao tool exec; git so roda subcomando de leitura (status, diff, log, show).".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Comando unico executado por bash -lc, com cwd no diretorio raiz. Sem ; & | > < $ crase nem --manifest-path."
                    },
                    "terms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Termos do filtro (substring, case-insensitive, nao e regex). Nao filtram por padrao: sem filter: true a saida volta inteira e o cabecalho avisa que terms foi ignorado."
                    },
                    "filter": {
                        "type": "boolean",
                        "description": "Liga o filtro duro de terms. Padrao false. Ligado, o cabecalho declara quantas linhas o filtro omitiu e o total real."
                    },
                    "question": {
                        "type": "string",
                        "description": "Ranking lexical dos blocos. Sem terms, a saida e fatiada em paragrafos e ranqueada por esta pergunta."
                    },
                    "context": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Linhas de contexto em volta de cada casamento (grep -C). Padrao 2, teto 20."
                    },
                    "max_blocks": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de blocos devolvidos. Padrao 6, teto 40."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tempo para este comando. Padrao 120s."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        struct Chunk {
            start: usize,
            end: usize,
            text: String,
        }

        fn wanted(terms: &[String]) -> Vec<String> {
            terms
                .iter()
                .map(|term| term.trim().to_lowercase())
                .filter(|term| !term.is_empty())
                .collect()
        }

        fn hit(line: &str, terms: &[String]) -> bool {
            let lower = line.to_lowercase();
            terms.iter().any(|term| lower.contains(term.as_str()))
        }

        fn count_matches(text: &str, terms: &[String]) -> usize {
            let terms = wanted(terms);
            if terms.is_empty() {
                return 0;
            }
            text.lines().filter(|line| hit(line, &terms)).count()
        }

        fn chunk_lines(text: &str, terms: &[String], context: usize) -> Vec<Chunk> {
            let terms = wanted(terms);
            if terms.is_empty() {
                return Vec::new();
            }
            let lines: Vec<&str> = text.lines().collect();
            if lines.is_empty() {
                return Vec::new();
            }
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if !hit(line, &terms) {
                    continue;
                }
                let start = index.saturating_sub(context);
                let end = (index + context).min(lines.len() - 1);
                match ranges.last_mut() {
                    Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                    _ => ranges.push((start, end)),
                }
            }
            ranges
                .into_iter()
                .map(|(start, end)| Chunk {
                    start: start + 1,
                    end: end + 1,
                    text: lines[start..=end].join("\n"),
                })
                .collect()
        }

        fn flush_group(
            lines: &[&str],
            group: &mut Vec<usize>,
            max_lines: usize,
            out: &mut Vec<Chunk>,
        ) {
            for piece in group.chunks(max_lines) {
                if let (Some(first), Some(last)) = (piece.first(), piece.last()) {
                    out.push(Chunk {
                        start: first + 1,
                        end: last + 1,
                        text: lines[*first..=*last].join("\n"),
                    });
                }
            }
            group.clear();
        }

        fn paragraph_chunks(text: &str, max_lines: usize) -> Vec<Chunk> {
            let lines: Vec<&str> = text.lines().collect();
            let max_lines = max_lines.max(1);
            let mut out = Vec::new();
            let mut group: Vec<usize> = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    flush_group(&lines, &mut group, max_lines, &mut out);
                } else {
                    group.push(index);
                }
            }
            flush_group(&lines, &mut group, max_lines, &mut out);
            out
        }

        /// Nomes que o bash resolve por dentro: nao existem como arquivo no
        /// PATH, entao a checagem de instalacao nao se aplica a eles.
        const BUILTINS: &[&str] = &[
            "echo", "printf", "pwd", "test", "true", "false", "cd", "export", "alias", "type",
            "read", "set", "unset", "source", "umask", "kill", "local", "shift", "return", "exit",
        ];

        /// Binarios que costumam faltar e o equivalente que a allowlist ja
        /// aceita. E so sugestao: nada aqui e executado.
        const EQUIVALENT: &[(&str, &str)] = &[
            ("rg", "grep -rn -e \"<padrao>\" <caminho>"),
            ("ripgrep", "grep -rn -e \"<padrao>\" <caminho>"),
            ("ag", "grep -rn -e \"<padrao>\" <caminho>"),
            ("ack", "grep -rn -e \"<padrao>\" <caminho>"),
            ("fd", "find <caminho> -name \"<padrao>\""),
            ("fdfind", "find <caminho> -name \"<padrao>\""),
            ("bat", "sed -n '1,200p' <arquivo>"),
            ("tree", "find <caminho> -maxdepth 2 -print"),
            ("sd", "sed -i -e 's/<velho>/<novo>/g' <arquivo>"),
        ];

        /// Primeiro token do comando: o programa que o bash procuraria.
        fn program_of(command: &str) -> &str {
            command.split_whitespace().next().unwrap_or_default()
        }

        /// Caminho explicito e conferido no disco; nome simples vai no PATH do
        /// proprio processo (o bash herda o mesmo ambiente). Sem PATH, nao bloqueia.
        fn on_disk(token: &str, base: &std::path::Path) -> bool {
            if token.contains('/') {
                return base.join(token).is_file();
            }
            let Some(path) = std::env::var_os("PATH") else {
                return true;
            };
            std::env::split_paths(&path)
                .map(|dir| dir.join(token))
                .any(|full| full.is_file())
        }

        /// `false` so quando da para afirmar que o binario nao esta instalado.
        fn installed(program: &str, base: &std::path::Path) -> bool {
            let basename = program.rsplit('/').next().unwrap_or(program);
            if basename.is_empty() || BUILTINS.contains(&basename) {
                return true;
            }
            on_disk(program, base)
        }

        /// Tools nativas que fazem o trabalho de um binario fora da allowlist.
        /// E so texto: nada daqui e executado nem amplia o que pode rodar.
        const NATIVE: &[(&str, &str)] = &[
            ("cat", "read"),
            ("head", "read"),
            ("tail", "read"),
            ("sed", "edit"),
            ("grep", "search"),
            ("rg", "search"),
            ("awk", "search"),
            ("find", "map ou search"),
            ("ls", "map"),
            ("tree", "map"),
            ("wc", "read"),
            ("sha256sum", "read (o cabecalho traz o blake3 do arquivo)"),
            ("sha1sum", "read (o cabecalho traz o blake3 do arquivo)"),
            ("shasum", "read (o cabecalho traz o blake3 do arquivo)"),
            ("md5sum", "read (o cabecalho traz o blake3 do arquivo)"),
            ("curl", "fetch"),
            ("wget", "fetch"),
            ("rm", "move"),
            ("mv", "move"),
            ("cp", "move (copy: true)"),
            ("touch", "write"),
        ];

        /// Substituto nativo do binario, quando existe um: a recusa termina
        /// ensinando o que fazer sem liberar nada.
        fn native_hint(basename: &str) -> String {
            match NATIVE.iter().find(|(name, _)| *name == basename) {
                Some((_, tool)) => format!(
                    " Alternativa que nao precisa liberar nada: a tool `{tool}` faz o mesmo trabalho."
                ),
                None => String::new(),
            }
        }

        /// Motivo da recusa e, principalmente, a alternativa equivalente:
        /// recusar sem dizer o caminho custa um retry as cegas. Nada aqui
        /// pede mais poder do que a regra ja permite.
        fn refusal(command: &str, allow: &[String]) -> String {
            let command = command.trim();
            let invitation = " Se nenhuma alternativa serve, registre o atrito com a tool `feedback` (action: \"record\", kind: \"refusal\").";
            if composes(command) || command.contains("--manifest-path") {
                return format!(
                    "comando recusado: {command}. Composicao e redirecionamento nao sao permitidos \
(|, ;, >, <, $, crase; tambem nao --manifest-path): um comando por chamada. Para varios padroes \
no mesmo passo use um `-e` por padrao: grep -rn -e \"A\" -e \"B\" <caminho>; a tool `search` faz \
o mesmo com `keywords: [\"A\", \"B\"]`.{invitation}"
                );
            }
            let basename = program_of(command).rsplit('/').next().unwrap_or_default();
            if basename.is_empty() {
                return "comando recusado: comando vazio; informe um comando com o binario na allowlist do projeto".to_string();
            }
            let hint = native_hint(basename);
            // Binario com tool equivalente nao volta nem por shell.allow: o
            // bloqueio e do binario, entao a recusa nao convida a editar a lista.
            if !hint.is_empty() {
                return format!(
                    "comando recusado: {command}. O binario `{basename}` tem tool equivalente e nao volta por shell.allow: o bloqueio e do binario, nao da lista do projeto.{hint}{invitation}"
                );
            }
            if allow.iter().any(|name| name == basename) {
                let rule = match basename {
                    "git" => "o git no shell so aceita subcomando de leitura (status, diff, log, show, blame, rev-parse): escrever historico (checkout, commit, reset, add, stash) fica fora",
                    "cargo" => "fora `cargo fmt`, o cargo pertence a tool exec (build, check, test, clippy), que declara os alvos",
                    _ => "wrappers que executam outro comando (sh, bash, env, xargs, make) nao passam pelo shell nem quando estao em shell.allow: rode o comando final direto",
                };
                return format!(
                    "comando recusado: {command}. O binario `{basename}` esta em shell.allow, mas {rule}.{invitation}"
                );
            }
            format!(
                "comando recusado: {command}. O binario `{basename}` nao esta liberado: inclua \"{basename}\" na chave shell.allow de ai.project.json.{invitation}"
            )
        }

        /// Binarios liberados quando o projeto nao declara `shell.allow`.
        /// A lista e minima de proposito: `git` (so leitura) e `jq`, nada mais.
        /// Todo binario com tool equivalente (grep, ls, cat, sed, awk, find,
        /// head, tail, mv, cp, rm) recusa sempre, com ou sem `shell.allow`; o
        /// binario do projeto entra so quando o projeto realmente precisa dele.
        /// `cargo` tambem fica de fora: so `cargo fmt` passa, pela regra fixa
        /// de formatador; build/test/run pertencem ao tool exec.
        const DEFAULT_ALLOW: &[&str] = &["git", "jq"];

        let params: ShellParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;

        let project_allow = || -> Option<Vec<String>> {
            #[derive(serde::Deserialize)]
            struct Root {
                #[serde(default)]
                shell: Option<ShellBlock>,
            }

            #[derive(serde::Deserialize)]
            struct ShellBlock {
                #[serde(default)]
                allow: Option<Vec<String>>,
            }

            let text = std::fs::read_to_string(self.root_dir.join("ai.project.json")).ok()?;
            let root: Root = serde_json::from_str(&text).ok()?;
            root.shell?.allow
        };

        let allow: Vec<String> = project_allow()
            .unwrap_or_else(|| DEFAULT_ALLOW.iter().map(|name| name.to_string()).collect());
        if !allowed(&params.command, &allow) {
            return Err(ToolError::InvalidArguments(refusal(&params.command, &allow)));
        }

        // Binario ausente nao derruba o turno: nada roda, e a resposta diz o
        // que falta e qual equivalente liberado faz o mesmo trabalho.
        let program = program_of(params.command.trim());
        let basename = program.rsplit('/').next().unwrap_or(program);
        if !installed(program, &self.root_dir) {
            let mut notice = format!(
                "\n- comando: {}\n- binario: {basename} nao esta instalado neste ambiente · nada foi executado\n",
                params.command.trim()
            );
            match EQUIVALENT.iter().find(|(name, _)| *name == basename) {
                Some((_, hint)) => notice.push_str(&format!(
                    "- equivalente liberado: {hint}\n- repita a chamada com o equivalente acima\n"
                )),
                None => notice.push_str(
                    "- sem equivalente conhecido: use um binario ja liberado (grep, sed, find, ls, cat, head, tail, stat, du, jq) ou libere o binario em shell.allow\n",
                ),
            }
            let sections = vec![Section {
                heading: "### shell — binario ausente".to_string(),
                body: notice,
                priority: 255,
            }];
            let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
            return Ok(finish(&self.store, sections, budget_tokens));
        }

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let timeout = Duration::from_secs(
            params
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );
        let raw = proc::bash(params.command.trim(), &root, timeout)?;
        let status = if raw.timed_out {
            "timeout"
        } else if raw.success {
            "ok"
        } else {
            "falhou"
        };

        let question = params
            .question
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let context = params.context.unwrap_or(2).min(20);
        let max_blocks = params.max_blocks.unwrap_or(6).clamp(1, 40);
        let filter_active = params.filter == Some(true) && !params.terms.is_empty();
        let terms_ignored = !params.terms.is_empty() && !filter_active;
        let filtered = filter_active || question.is_some();

        let mut candidates: Vec<crate::support::web::Block> = Vec::new();
        let mut matched = 0usize;
        let mut total = 0usize;
        for (stream, text) in [
            ("stdout", raw.stdout.as_str()),
            ("stderr", raw.stderr.as_str()),
        ] {
            total += text.lines().count();
            let chunks = if filter_active {
                matched += count_matches(text, &params.terms);
                chunk_lines(text, &params.terms, context)
            } else if question.is_some() {
                paragraph_chunks(text, 40)
            } else {
                Vec::new()
            };
            for chunk in chunks {
                candidates.push(crate::support::web::Block {
                    anchor: format!("{stream} {}-{}", chunk.start, chunk.end),
                    text: chunk.text,
                });
            }
        }
        let ranked: Vec<crate::support::web::Block> =
            crate::support::web::rank(candidates, question)
                .into_iter()
                .take(max_blocks)
                .collect();

        let combined = format!("{}{}", raw.stdout, raw.stderr);
        let mut summary = format!(
            "\n- comando: {}\n- status: {status} · {:.1}s\n",
            params.command.trim(),
            raw.duration.as_secs_f64()
        );
        if terms_ignored {
            summary.push_str(
                "- terms ignorados: o filtro so liga com \"filter\": true; nada foi omitido\n",
            );
        }
        if filter_active {
            summary.push_str(&format!(
                "- linhas: {matched} de {total} · {} linhas omitidas pelo filtro · {} blocos\n",
                total.saturating_sub(matched),
                ranked.len()
            ));
        } else if question.is_some() {
            summary.push_str(&format!("- blocos: {}\n", ranked.len()));
        } else {
            summary.push_str(&format!(
                "\n~~~text\n{}\n~~~\n",
                if combined.trim().is_empty() {
                    "(sem saida)".to_string()
                } else {
                    crate::exec::clip(combined.trim_end(), 4_000, 2_000)
                }
            ));
        }

        let mut sections = vec![Section {
            heading: format!("### shell — {status}"),
            body: summary,
            priority: 255,
        }];

        if filter_active && matched == 0 {
            let lines: Vec<&str> = combined.lines().collect();
            const HEAD: usize = 5;
            const TAIL: usize = 5;
            let (heading, body) = if lines.is_empty() {
                (
                    "#### nenhum termo casou · sem saida".to_string(),
                    "\n~~~text\n(sem saida)\n~~~\n".to_string(),
                )
            } else if lines.len() <= HEAD + TAIL {
                // Saida curta cabe inteira: aparece uma vez so, sem "..." e sem repetir linha.
                (
                    "#### nenhum termo casou · saida bruta".to_string(),
                    format!("\n~~~text\n{}\n~~~\n", lines.join("\n")),
                )
            } else {
                let head = lines[..HEAD].join("\n");
                let tail = lines[lines.len() - TAIL..].join("\n");
                (
                    "#### nenhum termo casou · inicio e fim da saida".to_string(),
                    format!("\n~~~text\n{head}\n...\n{tail}\n~~~\n"),
                )
            };
            sections.push(Section {
                heading,
                body,
                priority: 240,
            });
        }

        for block in &ranked {
            sections.push(Section {
                heading: format!("#### {}", block.anchor),
                body: format!(
                    "\n~~~text\n{}\n~~~\n",
                    crate::exec::clip(&block.text, 4_000, 1_000)
                ),
                priority: 200,
            });
        }

        if !filtered {
            sections.push(Section {
                heading: "#### saida completa".to_string(),
                body: format!(
                    "\n~~~text\n{}\n~~~\n",
                    crate::exec::clip(combined.trim_end(), 20_000, 20_000)
                ),
                priority: 40,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("shell");
        let terms: Vec<&str> = arguments
            .get("terms")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if terms.is_empty() {
            format!("shell: {command}")
        } else {
            format!("shell: {command} · termos: {}", terms.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn allow(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn tool_for(dir: &Path) -> ShellTool {
        ShellTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024)),
            4000,
        )
    }

    #[test]
    fn cargo_fmt_is_allowed() {
        assert!(allowed("cargo fmt", &[]));
        assert!(allowed("cargo fmt -- --check", &[]));
        assert!(allowed("cargo fmt --check", &[]));
    }

    #[test]
    fn rustfmt_without_a_file_is_allowed() {
        assert!(allowed("rustfmt --version", &[]));
        assert!(allowed("rustfmt --check", &[]));
    }

    #[test]
    fn a_shell_metacharacter_is_refused() {
        assert!(!allowed("git status; rm -rf /", &allow(&["git"])));
        assert!(!allowed("git status && touch x", &allow(&["git"])));
        assert!(!allowed("git status | tee x", &allow(&["git"])));
        assert!(!allowed("git status $(id)", &allow(&["git"])));
    }

    #[test]
    fn anything_outside_the_formatter_allowlist_is_refused() {
        assert!(!allowed("cargo clean", &[]));
        assert!(!allowed("git status", &[]));
        assert!(!allowed("rustfmt src/main.rs", &[]));
        assert!(!allowed("", &[]));
        assert!(!allowed("cargo fmt --manifest-path /etc/x/Cargo.toml", &[]));
    }

    #[test]
    fn an_absolute_path_is_checked_by_its_basename() {
        assert!(allowed("/usr/bin/git status", &allow(&["git"])));
        assert!(!allowed("/usr/bin/rm -rf /", &allow(&["git"])));
    }

    #[test]
    fn a_wrapper_is_refused_even_when_configured() {
        for wrapper in ["env", "sh", "bash", "xargs", "sudo", "python3", "make"] {
            assert!(
                !allowed(&format!("{wrapper} --version"), &allow(&[wrapper])),
                "{wrapper} deveria ser recusado"
            );
        }
    }

    #[test]
    fn formatters_stay_allowed_when_the_allowlist_is_replaced() {
        assert!(allowed("cargo fmt --check", &allow(&["rg"])));
        assert!(!allowed("git status", &allow(&["rg"])));
    }

    #[test]
    fn a_binary_from_the_project_file_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["uname","grep","ls"]}}"#,
        )
        .unwrap();
        let out = tool_for(dir.path())
            .call(serde_json::json!({"command": "uname -s"}))
            .unwrap();
        assert!(out.contains("ok"), "{out}");
        // Listar no shell.allow nao reabre o binario que tem tool equivalente: o
        // bloqueio e do binario, nao da lista do projeto.
        for command in ["grep -rn x .", "ls"] {
            let error = tool_for(dir.path())
                .call(serde_json::json!({ "command": command }))
                .unwrap_err();
            assert!(
                error.to_string().contains("comando recusado"),
                "{command} deveria ser recusado: {error}"
            );
        }
        let message = tool_for(dir.path())
            .call(serde_json::json!({"command": "grep -rn x ."}))
            .unwrap_err()
            .to_string();
        assert!(message.contains("nao volta por shell.allow"), "{message}");
        assert!(message.contains("`search`"), "{message}");
        assert!(message.contains("feedback"), "{message}");
    }

    #[test]
    fn a_binary_outside_the_default_allowlist_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "uname -s"}))
            .unwrap_err();
        assert!(error.to_string().contains("comando recusado"), "{error}");
    }

    #[test]
    fn the_default_allowlist_is_git_and_jq() {
        let dir = tempfile::tempdir().unwrap();
        // Sem ai.project.json, o binario que tem tool equivalente recusa
        // antes de executar: a lista padrao nao reabre esse caminho.
        for command in [
            "grep -rn foo src",
            "ls",
            "cat Cargo.toml",
            "wc -l x",
            "sed -n 1p x",
            "sort x",
            "diff a b",
            "find . -name x",
        ] {
            let error = tool_for(dir.path())
                .call(serde_json::json!({ "command": command }))
                .unwrap_err();
            assert!(
                error.to_string().contains("comando recusado"),
                "{command} deveria ser recusado: {error}"
            );
        }
        assert!(allowed("cargo fmt --check", &[]));
        // `git` (so leitura) continua aceito pela lista padrao.
        let out = tool_for(dir.path())
            .call(serde_json::json!({ "command": "git status" }))
            .expect("git status deveria passar pela allowlist padrao");
        assert!(out.contains("comando: git status"), "{out}");
    }

    #[test]
    fn describe_shows_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path());
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "cargo fmt"})),
            "shell: cargo fmt"
        );
    }

    fn log_tool(dir: &Path) -> ShellTool {
        // `cat` tem tool equivalente e nao volta por shell.allow: o log chega
        // por `cut`, que reproduz o arquivo linha a linha e nao tem substituto.
        std::fs::write(
            dir.join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["cut"]}}"#,
        )
        .unwrap();
        tool_for(dir)
    }

    fn log_file(dir: &Path, text: &str) -> String {
        let path = dir.join("log.txt");
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    #[test]
    fn terms_return_only_matching_blocks_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3 FAILED\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["FAILED"],
                "filter": true,
                "context": 1
            }))
            .unwrap();
        assert!(out.contains("3 FAILED"), "{out}");
        assert!(out.contains("a2"), "{out}");
        assert!(out.contains("a4"), "{out}");
        assert!(!out.contains("a1"), "{out}");
        assert!(out.contains("1 de 5"), "{out}");
    }

    #[test]
    fn no_match_shows_the_whole_short_output_once() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["NAOEXISTE"],
                "filter": true
            }))
            .unwrap();
        assert!(out.contains("0 de 5"), "{out}");
        assert!(out.contains("a1"), "{out}");
        assert!(out.contains("a5"), "{out}");
        assert!(
            !out.contains("..."),
            "saida curta cabe inteira, sem reticencias: {out}"
        );
        assert!(
            !out.contains("0 correspondencias"),
            "rotulo confuso quando nada casou: {out}"
        );
    }

    #[test]
    fn no_filter_keeps_the_previous_summary() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na3 FAILED");
        let out = tool
            .call(serde_json::json!({"command": format!("cut -c1- {log}")}))
            .unwrap();
        assert!(out.contains("3 FAILED"), "{out}");
        assert!(!out.contains("linhas:"), "{out}");
    }

    #[test]
    fn terms_without_filter_do_not_hide_lines() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\n- b2\n- b3\n- b4\n- b5\n- b6");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["-"]
            }))
            .unwrap();
        assert!(out.contains("a1"), "{out}");
        assert!(out.contains("- b6"), "{out}");
        assert!(out.contains("terms ignorados"), "{out}");
        assert!(!out.contains("linhas:"), "{out}");
    }

    #[test]
    fn filter_declares_the_omitted_lines() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\n- b3\n- b4\n- b5\n- b6");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["-"],
                "filter": true
            }))
            .unwrap();
        assert!(out.contains("4 de 6"), "{out}");
        assert!(out.contains("2 linhas omitidas pelo filtro"), "{out}");
    }

    #[test]
    fn a_question_ranks_the_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "ruido sem sentido\n\no token e validado aqui");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "question": "onde o token e validado"
            }))
            .unwrap();
        assert!(
            out.find("token").unwrap() < out.find("ruido").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn describe_shows_the_command_and_the_terms() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path());
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "git status", "terms": ["shell"]})),
            "shell: git status · termos: shell"
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "git status"})),
            "shell: git status"
        );
    }

    #[test]
    fn an_absurd_context_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3 FAILED\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["FAILED"],
                "context": 100000
            }))
            .unwrap();
        assert!(out.contains("a1"), "{out}");
        assert!(out.contains("a5"), "{out}");
    }

    #[test]
    fn a_huge_single_line_is_clipped() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let huge = "X".repeat(200_000);
        let log = log_file(dir.path(), &format!("alvo {huge}"));
        let out = tool
            .call(serde_json::json!({"command": format!("cut -c1- {log}"), "terms": ["alvo"]}))
            .unwrap();
        assert!(out.contains("alvo"), "deveria manter o casamento");
        assert!(out.len() < 100_000, "saida sem teto: {}", out.len());
    }

    #[test]
    fn git_config_override_is_refused() {
        assert!(allowed("git status", &allow(&["git"])));
        assert!(!allowed("git -c alias.pwn=!id pwn", &allow(&["git"])));
        assert!(!allowed("git --config-env=x=Y status", &allow(&["git"])));
    }

    #[test]
    fn cargo_outside_the_formatter_is_refused() {
        // Build/test/run pertencem ao tool exec: aqui so passa o formatador.
        assert!(!allowed("cargo run", &allow(&["cargo"])));
        assert!(!allowed("cargo test", &allow(&["cargo"])));
        assert!(!allowed("cargo install ripgrep", &allow(&["cargo"])));
        assert!(allowed("cargo fmt", &[]));
        assert!(allowed("cargo fmt --check", &[]));
    }

    #[test]
    fn the_default_allowlist_refuses_cargo_run() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "cargo run"}))
            .unwrap_err();
        assert!(error.to_string().contains("comando recusado"), "{error}");
    }

    #[test]
    fn git_is_restricted_to_read_subcommands() {
        assert!(allowed("git status", &allow(&["git"])));
        assert!(allowed("git diff --stat", &allow(&["git"])));
        assert!(allowed("git --no-pager log -3", &allow(&["git"])));
        assert!(allowed("/usr/bin/git show HEAD", &allow(&["git"])));
        for write in [
            "git checkout main",
            "git commit -m x",
            "git reset --hard",
            "git add .",
            "git stash",
            "git -C /tmp status",
        ] {
            assert!(
                !allowed(write, &allow(&["git"])),
                "{write} deveria ser recusado"
            );
        }
        assert!(!allowed("git status", &allow(&["grep"])));
    }

    #[test]
    fn a_single_line_without_match_appears_once() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "ZQXJ 1 .");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["-"],
                "filter": true
            }))
            .unwrap();
        assert_eq!(
            out.matches("ZQXJ").count(),
            1,
            "a unica linha apareceu repetida: {out}"
        );
        assert!(!out.contains("..."), "{out}");
        assert!(!out.contains("0 correspondencias"), "{out}");
    }

    #[test]
    fn long_output_without_match_keeps_head_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let text: Vec<String> = (1..=20).map(|n| format!("l{n}")).collect();
        let log = log_file(dir.path(), &text.join("\n"));
        let out = tool
            .call(serde_json::json!({
                "command": format!("cut -c1- {log}"),
                "terms": ["NAOEXISTE"],
                "filter": true
            }))
            .unwrap();
        assert!(out.contains("l1"), "{out}");
        assert!(out.contains("l20"), "{out}");
        assert!(!out.contains("l10"), "o meio deveria estar elidido: {out}");
        assert_eq!(out.matches("...").count(), 1, "elisao repetida: {out}");
    }

    #[test]
    fn a_missing_binary_is_reported_instead_of_failing_the_call() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["tupi-binario-ausente"]}}"#,
        )
        .unwrap();
        let out = tool_for(dir.path())
            .call(serde_json::json!({"command": "tupi-binario-ausente --version"}))
            .unwrap();
        assert!(out.contains("binario ausente"), "{out}");
        assert!(
            out.contains("tupi-binario-ausente nao esta instalado"),
            "{out}"
        );
        assert!(out.contains("nada foi executado"), "{out}");
        assert!(out.contains("sem equivalente conhecido"), "{out}");
    }

    #[test]
    fn rg_is_refused_by_the_native_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["rg"]}}"#,
        )
        .unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "rg -n foo"}))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("comando recusado"), "{message}");
        assert!(message.contains("`search`"), "{message}");
        assert!(!message.contains("nao esta instalado"), "{message}");
    }

    #[test]
    fn a_composition_refusal_teaches_the_alternative() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "grep -rn 'A|B' src"}))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("comando recusado"), "{message}");
        assert!(message.contains("grep -rn -e \"A\" -e \"B\""), "{message}");
        assert!(message.contains("keywords: [\"A\", \"B\"]"), "{message}");
        assert!(message.contains("`search`"), "{message}");
    }

    #[test]
    fn a_binary_refusal_names_the_release_path_and_the_substitute() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "sha256sum target/debug/tupi"}))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("comando recusado"), "{message}");
        // Binario com tool equivalente: o bloqueio e do binario, entao a recusa
        // nao manda editar o shell.allow do projeto.
        assert!(message.contains("nao volta por shell.allow"), "{message}");
        assert!(!message.contains("na chave shell.allow"), "{message}");
        assert!(message.contains("`read"), "{message}");
        assert!(message.contains("blake3"), "{message}");
        // O que resta do atrito tem onde ser registrado.
        assert!(message.contains("feedback"), "{message}");
        assert!(!message.contains("Composicao"), "{message}");
    }
}
