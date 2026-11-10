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
