//! Conexão stdio com um servidor MCP: spawn, handshake e listagem de tools.
//!
//! O transporte é bloqueante de propósito: [`tupi_kernel::Tool::call`] é
//! síncrono e roda dentro de `spawn_blocking` em `crates/tupi-app/src/app.rs`,
//! então nada aqui usa tokio. O timeout é por leitura: o stdout do filho é lido
//! numa thread que publica linhas num canal, e cada resposta espera em
//! `recv_timeout`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tupi_kernel::ToolError;

use super::ServerSpec;

/// Versão de protocolo anunciada no `initialize`.
const PROTOCOL_VERSION: &str = "2024-11-05";
/// Nome do cliente no `clientInfo` do handshake.
const CLIENT_NAME: &str = "tupi";
/// Teto de espera por resposta do servidor, por leitura.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// Quanto se espera a thread de stderr publicar a cauda antes de montar a
/// mensagem de erro de um servidor que morreu.
const STDERR_GRACE: Duration = Duration::from_millis(250);
/// Quantas linhas de stderr acompanham a mensagem de erro.
const STDERR_TAIL_LINES: usize = 8;
/// Recorte de cada linha de stderr na mensagem de erro.
const STDERR_LINE_CHARS: usize = 200;
/// Teto de páginas em `tools/list`: paginação em loop não pode ser infinita.
const MAX_TOOL_PAGES: usize = 64;

/// Tool anunciada por um servidor, no formato neutro de `tupi-tools`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    /// Nome chamado em `tools/call`.
    pub name: String,
    /// Descrição declarada pelo servidor (vazia quando ausente).
    pub description: String,
    /// JSON Schema dos argumentos.
    pub input_schema: Value,
}

impl ToolInfo {
    fn from_value(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str)?.to_string();
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let input_schema = value
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "type": "object" }));
        Some(Self {
            name,
            description,
            input_schema,
        })
    }
}

/// Servidor stdio já inicializado.
///
/// Criar a conexão custa um processo filho e um handshake: quem chama deve
/// guardar a conexão e não respawnar por call.
pub struct Connection {
    name: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    next_id: u64,
    server_info: Value,
    read_only: bool,
    read_timeout: Duration,
}

impl Connection {
    /// Spawna o filho descrito por `spec` e completa `initialize` +
    /// `notifications/initialized`.
    ///
    /// Falha de spawn, de `initialize` ou de leitura viram `ToolError` com o
    /// nome do servidor e a cauda do stderr; o binário nunca cai por causa de um
    /// servidor terceiro.
    pub fn spawn(spec: &ServerSpec) -> Result<Self, ToolError> {
        Self::spawn_with_timeout(spec, READ_TIMEOUT)
    }

    /// Igual a [`Connection::spawn`], com teto explícito de espera por leitura.
    ///
    /// Zero é recusado: esperar zero segundo nunca é intenção de quem configura.
    pub fn spawn_with_timeout(spec: &ServerSpec, timeout: Duration) -> Result<Self, ToolError> {
        if timeout.is_zero() {
            return Err(ToolError::Other(format!(
                "servidor mcp `{}`: timeout de leitura não pode ser zero",
                spec.name
            )));
        }
        let (program, args) = spec.command.split_first().ok_or_else(|| {
            ToolError::Other(format!("servidor mcp `{}`: `command` vazio", spec.name))
        })?;
        let mut command = Command::new(program);
        command.args(args);
        if !spec.env.is_empty() {
            // `env` nunca entra em mensagem: pode carregar credencial.
            command.envs(&spec.env);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        isolate(&mut command);

        let mut child = command.spawn().map_err(|error| {
            ToolError::Other(format!(
                "servidor mcp `{}`: falha ao iniciar `{program}`: {error}",
                spec.name
            ))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| pipe_error(&spec.name, "stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| pipe_error(&spec.name, "stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| pipe_error(&spec.name, "stderr"))?;

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        read_stderr_into(stderr, Arc::clone(&stderr_tail));
        let lines = read_stdout_into(stdout);

        let mut connection = Self {
            name: spec.name.clone(),
            child: Some(child),
            stdin: Some(stdin),
            lines,
            stderr_tail,
            next_id: 1,
            server_info: Value::Null,
            read_only: spec.read_only,
            read_timeout: timeout,
        };
        connection.handshake()?;
        Ok(connection)
    }

    /// Nome declarado do servidor.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `serverInfo` devolvido pelo `initialize`.
    pub fn server_info(&self) -> &Value {
        &self.server_info
    }

    /// `true` quando o servidor declara que as tools dele não escrevem.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Todas as tools do servidor, seguindo `nextCursor` até a última página.
    pub fn tools(&mut self) -> Result<Vec<ToolInfo>, ToolError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_TOOL_PAGES {
            let params = match &cursor {
                Some(cursor) => serde_json::json!({ "cursor": cursor }),
                None => serde_json::json!({}),
            };
            let result = self.request("tools/list", params)?;
            if let Some(list) = result.get("tools").and_then(Value::as_array) {
                tools.extend(list.iter().filter_map(ToolInfo::from_value));
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => return Ok(tools),
            }
        }
        Err(ToolError::Other(format!(
            "servidor mcp `{}`: `tools/list` passou de {MAX_TOOL_PAGES} páginas",
            self.name
        )))
    }

    /// `tools/call`: executa a tool do servidor e devolve o `result` cru, com
    /// `content[]` e `isError` para o chamador renderizar.
    pub fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, ToolError> {
        let params = serde_json::json!({ "name": tool, "arguments": arguments });
        self.request("tools/call", params)
    }

    fn handshake(&mut self) -> Result<(), ToolError> {
        let params = serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {
                "name": CLIENT_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        let result = self.request("initialize", params)?;
        self.server_info = result.get("serverInfo").cloned().unwrap_or(Value::Null);
        self.notify("notifications/initialized")?;
        Ok(())
    }

    fn notify(&mut self, method: &str) -> Result<(), ToolError> {
        let message = serde_json::json!({ "jsonrpc": "2.0", "method": method });
        self.send(method, &message)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        let id = self.next_id;
        self.next_id += 1;
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send(method, &message)?;
        self.await_response(id, method)
    }

    fn send(&mut self, method: &str, message: &Value) -> Result<(), ToolError> {
        let name = self.name.clone();
        let mut line = serde_json::to_string(message).map_err(|error| {
            ToolError::Other(format!(
                "servidor mcp `{name}`: falha ao serializar `{method}`: {error}"
            ))
        })?;
        line.push('\n');
        let written = match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()),
            None => {
                return Err(ToolError::Other(format!(
                    "servidor mcp `{name}`: stdin já fechado em `{method}`"
                )));
            }
        };
        written.map_err(|error| {
            ToolError::Other(format!(
                "servidor mcp `{name}`: falha ao enviar `{method}`: {error}{}",
                self.stderr_note()
            ))
        })
    }

    fn await_response(&mut self, id: u64, method: &str) -> Result<Value, ToolError> {
        let deadline = Instant::now() + self.read_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(self.timeout_error(method));
            }
            let line = match self.lines.recv_timeout(remaining) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return Err(self.timeout_error(method)),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ToolError::Other(format!(
                        "servidor mcp `{}`: encerrou antes de responder `{method}`{}",
                        self.name,
                        self.stderr_note()
                    )));
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                // Linha que não é JSON-RPC é log do servidor, não resposta.
                continue;
            };
            let is_reply = message.get("id").and_then(Value::as_u64) == Some(id)
                && message.get("method").is_none();
            if is_reply {
                if let Some(error) = message.get("error") {
                    return Err(ToolError::Other(format!(
                        "servidor mcp `{}`: `{method}` falhou: {}",
                        self.name,
                        describe_error(error)
                    )));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            // Requisição vinda do servidor (sampling, roots...): recusar é
            // melhor do que travar esperando uma resposta nossa que não vem.
            let is_server_request = message.get("id").is_some()
                && message.get("method").is_some()
                && message.get("result").is_none()
                && message.get("error").is_none();
            if is_server_request {
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id").cloned().unwrap_or(Value::Null),
                    "error": {
                        "code": -32601,
                        "message": "método não suportado pelo cliente tupi",
                    },
                });
                let _ = self.send(method, &reply);
            }
        }
    }

    fn timeout_error(&self, method: &str) -> ToolError {
        ToolError::Other(format!(
            "servidor mcp `{}`: sem resposta em {:?} para `{method}`{}",
            self.name,
            self.read_timeout,
            self.stderr_note()
        ))
    }

    /// Cauda do stderr já publicada, esperando uma janela curta quando ainda
    /// está vazia: o filho pode ter morrido logo depois de escrever nela.
    fn stderr_note(&self) -> String {
        let deadline = Instant::now() + STDERR_GRACE;
        loop {
            let note = self.stderr_now();
            if !note.is_empty() || Instant::now() >= deadline {
                return note;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stderr_now(&self) -> String {
        let tail = match self.stderr_tail.lock() {
            Ok(tail) => tail.iter().cloned().collect::<Vec<_>>().join(" | "),
            Err(_) => return String::new(),
        };
        let tail = tail.trim();
        if tail.is_empty() {
            String::new()
        } else {
            format!(" (stderr: {tail})")
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Fechar o stdin avisa o servidor; kill + wait garante que não sobra
        // processo zumbi depois que a connection sai de escopo.
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            kill_tree(&mut child);
            let _ = child.wait();
        }
    }
}

fn pipe_error(name: &str, which: &str) -> ToolError {
    ToolError::Other(format!(
        "servidor mcp `{name}`: {which} indisponível no processo filho"
    ))
}

fn describe_error(error: &Value) -> String {
    match (
        error.get("message").and_then(Value::as_str),
        error.get("code").and_then(Value::as_i64),
    ) {
        (Some(message), Some(code)) => format!("{message} (código {code})"),
        (Some(message), None) => message.to_string(),
        _ => error.to_string(),
    }
}

/// Thread leitora do stdout: publica uma linha por mensagem e termina no EOF.
fn read_stdout_into(stdout: impl Read + Send + 'static) -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    receiver
}

/// Thread leitora do stderr: guarda só a cauda, para a mensagem de erro.
fn read_stderr_into(stderr: impl Read + Send + 'static, tail: Arc<Mutex<VecDeque<String>>>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let line: String = line.trim().chars().take(STDERR_LINE_CHARS).collect();
            if line.is_empty() {
                continue;
            }
            if let Ok(mut tail) = tail.lock() {
                if tail.len() == STDERR_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
    });
}

#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

#[cfg(unix)]
fn kill_tree(child: &mut Child) {
    let pgid = child.id() as i32;
    // SAFETY: o filho nasceu com `process_group(0)`, então pid == pgid.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_tree(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake(name: &str, script: &str) -> ServerSpec {
        ServerSpec::local(
            name,
            vec!["sh".to_string(), "-c".to_string(), script.to_string()],
        )
    }

    /// Servidor fake mínimo: responde `initialize` e `tools/list` em duas
    /// páginas, ignora notificações e emite log antes da primeira resposta.
    #[cfg(unix)]
    const TWO_PAGES: &str = r#"
id_of() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }
while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*)
  printf 'log do servidor antes da resposta\n'
  printf '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"oi"}}\n'
  printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"1.0"}}}\n' "$(id_of "$line")"
  ;;
*'"method":"tools/list"'*)
  case "$line" in
  *'"cursor":"page-2"'*)
    printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"write_file","description":"grava","inputSchema":{"type":"object","properties":{}}}]}}\n' "$(id_of "$line")"
    ;;
  *)
    printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"read_file","description":"lê","inputSchema":{"type":"object"}}],"nextCursor":"page-2"}}\n' "$(id_of "$line")"
    ;;
  esac
  ;;
*) : ;;
esac
done
"#;

    #[cfg(unix)]
    #[test]
    fn handshake_reports_server_info_and_lists_every_page() {
        let mut connection = Connection::spawn(&fake("filesystem", TWO_PAGES)).expect("handshake");
        assert_eq!(connection.name(), "filesystem");
        assert_eq!(
            connection.server_info().get("name").and_then(Value::as_str),
            Some("fake")
        );
        let tools = connection.tools().expect("tools/list");
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["read_file", "write_file"], "páginas e ordem");
        assert_eq!(tools[1].description, "grava");
        assert_eq!(tools[0].input_schema, serde_json::json!({"type": "object"}));
    }

    #[cfg(unix)]
    #[test]
    fn a_dying_server_reports_stderr_without_leaking_env() {
        // Nenhuma mensagem — `Debug` do spec, Display do erro, a forma
        // serializada que vai para o modelo — pode carregar `env`: ali cabe
        // credencial.
        fn assert_clean(text: &str, what: &str) {
            for secret in ["s3cr3t-value", "FAKE_TOKEN"] {
                assert!(!text.contains(secret), "{what} vazou `{secret}`: {text}");
            }
        }

        let mut spec = fake("quebrado", "printf 'boom: config invalida\\n' >&2\nexit 1\n");
        spec.env
            .insert("FAKE_TOKEN".to_string(), "s3cr3t-value".to_string());
        assert_clean(&format!("{spec:?}"), "Debug do spec");

        let error = Connection::spawn(&spec)
            .err()
            .expect("servidor morto falha no handshake");
        let message = error.to_string();
        assert!(message.contains("boom"), "{message}");
        assert_clean(&message, "Display do erro");
        assert_clean(&format!("{error:?}"), "Debug do erro");
        let serialized = serde_json::to_string(&message).expect("serializa a mensagem");
        assert_clean(&serialized, "erro serializado");

        // Servidor que morre no meio do `tools/list`: a cauda do stderr entra na
        // mensagem, o `env` continua fora.
        let script = r#"
id_of() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }
while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*)
  printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"meio","version":"1.0"}}}\n' "$(id_of "$line")"
  ;;
*'"method":"tools/list"'*)
  printf 'falhou no meio\n' >&2
  exit 3
  ;;
esac
done
"#;
        let mut spec = fake("meio", script);
        spec.env
            .insert("FAKE_TOKEN".to_string(), "s3cr3t-value".to_string());
        let mut connection = Connection::spawn(&spec).expect("handshake do meio");
        let error = connection.tools().err().expect("tools/list falha");
        let message = error.to_string();
        assert!(message.contains("falhou no meio"), "{message}");
        assert_clean(&message, "erro com cauda de stderr");
        assert_clean(&format!("{error:?}"), "Debug do erro");
    }

    #[cfg(unix)]
    #[test]
    fn drop_reaps_the_child_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let script = format!(
            r#"
printf '%s' "$$" > '{pid}'
id_of() {{ printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }}
while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*)
  printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2024-11-05","serverInfo":{{"name":"lento","version":"1.0"}}}}}}\n' "$(id_of "$line")"
  # Ignora o EOF do stdin e fica vivo: só o kill do drop encerra este processo.
  exec sleep 120
  ;;
esac
done
"#,
            pid = pid_file.display()
        );
        let connection = Connection::spawn(&fake("lento", &script)).expect("handshake");
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("pid do filho")
            .trim()
            .parse()
            .expect("pid numérico");
        // SAFETY: sinal 0 só consulta a existência do processo.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "filho vivo antes do drop");

        drop(connection);

        // O filho não morre sozinho (`exec sleep 120`), então sumir da tabela de
        // processos prova o kill *e* o `wait` que colhe o zumbi.
        let deadline = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(
                Instant::now() < deadline,
                "o filho `{pid}` continua vivo (ou zumbi) depois do drop"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_spec_without_command_is_refused() {
        let error = Connection::spawn(&ServerSpec::local("vazio", Vec::new()))
            .err()
            .expect("command vazio falha");
        assert!(error.to_string().contains("command"), "{error}");
    }

    /// Servidor que abre a conexão e nunca responde: o cliente desiste no teto
    /// de leitura, nomeando o servidor, em vez de travar a thread.
    #[cfg(unix)]
    #[test]
    fn a_silent_server_times_out_naming_the_server() {
        let started = Instant::now();
        let error = Connection::spawn_with_timeout(
            &fake("mudo", "while IFS= read -r line; do :; done\n"),
            Duration::from_millis(300),
        )
        .err()
        .expect("servidor mudo falha no initialize");
        let message = error.to_string();
        assert!(message.contains("mudo"), "{message}");
        assert!(message.contains("initialize"), "{message}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timeout demorou {:?}",
            started.elapsed()
        );
    }

    /// Servidor que fecha o stdout depois do handshake: a leitura seguinte
    /// devolve erro com o nome do servidor, em vez de panic.
    #[cfg(unix)]
    #[test]
    fn a_server_that_closes_stdout_is_an_error_not_a_panic() {
        let script = r#"
id_of() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }
while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*)
  printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"morre","version":"1.0"}}}\n' "$(id_of "$line")"
  exec 1>&-
  ;;
esac
done
"#;
        let mut connection = Connection::spawn(&fake("morre", script)).expect("handshake");
        let started = Instant::now();
        let error = connection.tools().err().expect("tools/list falha");
        let message = error.to_string();
        assert!(message.contains("morre"), "{message}");
        assert!(message.contains("tools/list"), "{message}");
        assert!(started.elapsed() < Duration::from_secs(10), "sem travar");
    }

    #[test]
    fn a_zero_read_timeout_is_refused() {
        let spec = ServerSpec::local("zero", vec!["sh".to_string()]);
        let error = Connection::spawn_with_timeout(&spec, Duration::ZERO)
            .err()
            .expect("timeout zero é recusado");
        assert!(error.to_string().contains("timeout"), "{error}");
    }
}
