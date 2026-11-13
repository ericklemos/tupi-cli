use tupi_kernel::ChatRequest;

/// Chave usada no redb para lembrar o modo de debug entre execuções.
pub const SHOW_PROMPTS_KEY: &str = "show_prompts_messages";

pub fn value(enabled: bool) -> &'static str {
    if enabled { "true" } else { "false" }
}

pub fn enabled(value: Option<&str>) -> bool {
    value == Some("true")
}

/// Texto legível com o pacote que será enviado ao provedor.
///
/// Mostra o modelo, as tools registradas e cada mensagem com seu papel.
/// Chamadas de tool aparecem identificadas pelo id, para casar com o
/// ToolFinished correspondente na interface.
pub fn render_prompt(request: &ChatRequest) -> String {
    let mut out = format!("modelo: {}\ntools: {}", request.model, request.tools.len());

    for tool in &request.tools {
        out.push_str(&format!("\n  - {}", tool.name));
    }

    for message in &request.messages {
        out.push_str("\n\n[");
        out.push_str(message.role.as_str());
        out.push(']');

        if let Some(call_id) = &message.tool_call_id {
            out.push_str(&format!(" call_id={call_id}"));
        }

        if !message.content.is_empty() {
            out.push('\n');
            out.push_str(&message.content);
        }

        for call in &message.tool_calls {
            out.push_str(&format!(
                "\n  tool_call {} {} {}",
                call.id, call.name, call.arguments
            ));
        }
    }

    out
}

/// Renderiza o artefato com a última mensagem do usuário e o corpo exato
/// enviado ao provedor.
pub fn render_last_prompt(user_message: &str, payload: &serde_json::Value) -> String {
    let body = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
    format!(
        "# Última mensagem do usuário\n\n{user_message}\n\n# Último payload enviado ao provedor\n\n```json\n{body}\n```\n"
    )
}

/// Grava o artefato em `<dir>/last-prompt-message.md`, sem falhar o turno.
pub fn write_last_prompt(dir: &std::path::Path, user_message: &str, payload: &serde_json::Value) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join("last-prompt-message.md");
    let _ = std::fs::write(path, render_last_prompt(user_message, payload));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_artifact_shows_the_user_message_and_the_payload() {
        let payload = serde_json::json!({"model": "m", "messages": []});
        let rendered = render_last_prompt("oi", &payload);
        assert!(rendered.contains("oi"));
        assert!(rendered.contains("\"model\": \"m\""));
    }

    #[test]
    fn writing_creates_the_directory_and_the_markdown_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("tupi");
        let payload = serde_json::json!({"ok": true});
        write_last_prompt(&target, "hello", &payload);
        let text = std::fs::read_to_string(target.join("last-prompt-message.md")).unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("\"ok\": true"));
    }
}
