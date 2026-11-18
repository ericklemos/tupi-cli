//! A tool `mcp`: uma tool só com união de ações para os servidores declarados
//! em `ai.project.json`. Servidor grande não vira dezenas de tools flat.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use serde::Deserialize;
use serde_json::{Value, json};
use tupi_kernel::{Effect, Tool, ToolError, ToolSpec};

use super::ServerSpec;
use super::client::{Connection, ToolInfo};

/// Ações aceitas nesta fase; `call` entra na subtarefa seguinte.
const ACTIONS: [&str; 3] = ["servers", "tools", "call"];

/// Teto da descrição listada: o schema inteiro do servidor não cabe no turno.
const DESCRIPTION_CHARS: usize = 120;

type Pool = Mutex<HashMap<String, Arc<Mutex<Connection>>>>;

/// Conexões vivas por spec, no espírito de `WATCHERS`: o servidor é spawnado
/// uma vez e reusado entre calls.
fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Trava sem `unwrap`: pool envenenado por um panic de outra call não vira
/// segundo panic.
fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Chave neutra do pool: nome, comando e ambiente do spec. O ambiente pode
/// carregar credencial, então a chave nunca é impressa.
fn pool_key(spec: &ServerSpec) -> String {
    let env: Vec<String> = spec
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        spec.name,
        spec.command.join("\u{0}"),
        env.join("\u{1}"),
        spec.read_only
    )
}

fn action_of(arguments: &Value) -> &str {
    arguments
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|action| !action.is_empty())
        .unwrap_or("servers")
}

fn argument_str<'a>(arguments: &'a Value, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
}

fn shorten(text: &str) -> String {
    let text = text.trim();
    let mut out: String = text.chars().take(DESCRIPTION_CHARS).collect();
    if text.chars().count() > DESCRIPTION_CHARS {
        out.push('…');
    }
    out
}

#[derive(Deserialize)]
struct McpParams {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    server: Option<String>,
}

/// Tool única de todos os servidores MCP declarados no projeto.
pub struct McpTool {
    servers: Vec<ServerSpec>,
}

impl McpTool {
    pub fn new(servers: Vec<ServerSpec>) -> Self {
        Self { servers }
    }

    pub fn servers(&self) -> &[ServerSpec] {
        &self.servers
    }

    fn names(&self) -> Vec<&str> {
        self.servers.iter().map(|spec| spec.name.as_str()).collect()
    }

    fn spec_for(&self, name: &str) -> Option<&ServerSpec> {
        self.servers.iter().find(|spec| spec.name == name)
    }

    /// Conexão do spec, spawnando e fazendo o handshake só na primeira vez.
    fn connect(&self, spec: &ServerSpec) -> Result<Arc<Mutex<Connection>>, ToolError> {
        let key = pool_key(spec);
        if let Some(connection) = guard(pool()).get(&key) {
            return Ok(Arc::clone(connection));
        }
        // Fora do lock: o handshake pode esperar o servidor terceiro.
        let connection = Arc::new(Mutex::new(Connection::spawn(spec)?));
        guard(pool()).insert(key, Arc::clone(&connection));
        Ok(connection)
    }

    /// `mcp(action="servers")`: nome, transporte e status de cada servidor.
    /// Falha de um servidor não derruba os outros nem a tool.
    fn list_servers(&self) -> String {
        let mut body = String::new();
        for spec in &self.servers {
            let status = match self.connect(spec) {
                Ok(connection) => match guard(&connection).tools() {
                    Ok(tools) => format!("ok · {} tool(s)", tools.len()),
                    Err(error) => format!("failed · {error}"),
                },
                Err(error) => format!("failed · {error}"),
            };
            body.push_str(&format!("- {} (local) · {status}\n", spec.name));
        }
        format!(
            "### mcp · {} servidor(es) declarado(s)\n{body}",
            self.servers.len()
        )
    }

    /// `mcp(action="tools", server=..)`: nome e descrição das tools do servidor.
    fn list_tools(&self, server: &str) -> Result<String, ToolError> {
        let spec = self.spec_for(server).ok_or_else(|| {
            ToolError::InvalidArguments(format!(
                "mcp: servidor `{server}` não declarado em ai.project.json — use server={}",
                self.names().join("|")
            ))
        })?;
        let connection = self.connect(spec)?;
        let tools: Vec<ToolInfo> = guard(&connection).tools()?;
        let mut body = format!("### mcp · {server} · {} tool(s)\n", tools.len());
        for tool in &tools {
            let description = shorten(&tool.description);
            if description.is_empty() {
                body.push_str(&format!("- {}\n", tool.name));
            } else {
                body.push_str(&format!("- {}: {description}\n", tool.name));
            }
        }
        Ok(body)
    }

    /// `true` quando o servidor do argumento declara `read_only: true`.
    fn read_only(&self, arguments: &Value) -> bool {
        self.spec_for(argument_str(arguments, "server"))
            .is_some_and(|spec| spec.read_only)
    }
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp".to_string(),
            description: format!(
                "Servidores MCP declarados em ai.project.json ({}): `action=servers` lista nome, transporte e status; `action=tools` com `server` lista nome e descrição das tools do servidor. O schema completo de cada tool só aparece em `action=call`.",
                if self.servers.is_empty() {
                    "nenhum".to_string()
                } else {
                    self.names().join(", ")
                }
            ),
            schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ACTIONS,
                        "description": "servers (padrão) lista os servidores; tools lista as tools de um servidor."
                    },
                    "server": {
                        "type": "string",
                        "description": "Nome do servidor declarado em ai.project.json."
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool do servidor, para `action=call`."
                    },
                    "arguments": {
                        "type": "object",
                        "description": "Argumentos da tool, para `action=call`."
                    }
                }
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        fn clip(text: &str) -> String {
            let max_chars = (crate::budget::DEFAULT_BUDGET_TOKENS as f64
                * crate::budget::CHARS_PER_TOKEN)
                .floor() as usize;
            let chars: Vec<char> = text.chars().collect();
            if chars.len() <= max_chars {
                return text.to_string();
            }
            let head = max_chars * 2 / 3;
            let tail = max_chars - head;
            let mut out: String = chars[..head].iter().collect();
            out.push_str(&format!(
                "\n... ({} caracteres omitidos)\n",
                chars.len() - head - tail
            ));
            out.extend(&chars[chars.len() - tail..]);
            out
        }

        fn render_content(content: &Value) -> String {
            let Some(items) = content.as_array() else {
                return content.as_str().map(str::to_string).unwrap_or_default();
            };
            let mut parts: Vec<String> = Vec::new();
            for item in items {
                let kind = item.get("type").and_then(Value::as_str).unwrap_or("unknown");
                match kind {
                    "text" => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                    "image" => parts.push(format!(
                        "[imagem {}]",
                        item.get("mimeType").and_then(Value::as_str).unwrap_or("?")
                    )),
                    "resource" => parts.push(format!(
                        "[recurso {}]",
                        item.get("resource")
                            .and_then(|resource| resource.get("uri"))
                            .and_then(Value::as_str)
                            .unwrap_or("?")
                    )),
                    other => parts.push(format!("[{other}]")),
                }
            }
            parts.join("\n")
        }

        if self.servers.is_empty() {
            return Err(ToolError::InvalidArguments(
                "mcp: nenhum servidor declarado em ai.project.json".to_string(),
            ));
        }
        let params: McpParams = serde_json::from_value(arguments.clone())
            .map_err(|error| ToolError::InvalidArguments(format!("mcp: {error}")))?;
        let action = params
            .action
            .as_deref()
            .map(str::trim)
            .filter(|action| !action.is_empty())
            .unwrap_or("servers");
        match action {
            "servers" => Ok(self.list_servers()),
            "tools" => {
                let server = argument_str(&arguments, "server");
                if server.is_empty() {
                    return Err(ToolError::InvalidArguments(format!(
                        "mcp: `action=tools` exige server={}",
                        self.names().join("|")
                    )));
                }
                self.list_tools(server)
            }
            "call" => {
                let server = argument_str(&arguments, "server");
                let tool = argument_str(&arguments, "tool");
                if server.is_empty() || tool.is_empty() {
                    return Err(ToolError::InvalidArguments(format!(
                        "mcp: `action=call` exige server={} e tool=<nome>",
                        self.names().join("|")
                    )));
                }
                let spec = self.spec_for(server).ok_or_else(|| {
                    ToolError::InvalidArguments(format!(
                        "mcp: servidor `{server}` não declarado em ai.project.json — use server={}",
                        self.names().join("|")
                    ))
                })?;
                let payload = arguments
                    .get("arguments")
                    .filter(|value| value.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let connection = self.connect(spec)?;
                let result = guard(&connection).call(tool, payload)?;
                let body = clip(&render_content(result.get("content").unwrap_or(&Value::Null)));
                let mut out = format!("### mcp · {server} · {tool}\n");
                if result
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    out.push_str("erro devolvido pelo servidor:\n");
                }
                out.push_str(&body);
                Ok(out)
            }
            other => Err(ToolError::InvalidArguments(format!(
                "mcp: ação desconhecida `{other}` — use {}",
                ACTIONS.join("|")
            ))),
        }
    }

    fn effect(&self, arguments: &Value) -> Effect {
        match action_of(arguments) {
            "servers" | "tools" => Effect::ReadOnly,
            "call" if self.read_only(arguments) => Effect::ReadOnly,
            _ => Effect::Opaque,
        }
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let action = action_of(arguments);
        let server = argument_str(arguments, "server");
        if server.is_empty() {
            return format!("mcp · {action}");
        }
        if action == "call" {
            let tool = argument_str(arguments, "tool");
            if tool.is_empty() {
                return format!("{server} · call");
            }
            return format!("{server} · {tool}");
        }
        format!("{server} · {action}")
    }
}
