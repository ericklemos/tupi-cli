use std::sync::Arc;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("argumento inválido: {0}")]
    InvalidArguments(String),
    #[error("caminho fora do diretório raiz: {0}")]
    OutsideRoot(String),
    #[error("arquivo não encontrado: {0}")]
    NotFound(String),
    #[error("símbolo não encontrado: {0}")]
    SymbolNotFound(String),
    #[error("handle desconhecido ou expirado: {0}")]
    UnknownHandle(String),
    #[error("{0}")]
    Other(String),
}

pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn call(&self, arguments: Value) -> Result<String, ToolError>;

    /// Descrição curta dos argumentos para a interface, sem expor o corpo lido.
    /// O padrão é o JSON compacto; a tool pode mostrar só o que importa.
    fn describe_call(&self, arguments: &Value) -> String {
        let raw = serde_json::to_string(arguments).unwrap_or_default();
        let mut out: String = raw.chars().take(80).collect();
        if raw.chars().count() > 80 {
            out.push('…');
        }
        out
    }
}

#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::Message;

    use crate::ChatRequest;

    struct EchoTool(&'static str);

    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: self.0.to_string(),
                description: "devolve ok".to_string(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, _arguments: Value) -> Result<String, ToolError> {
            Ok("ok".to_string())
        }
    }

    #[test]
    fn registry_keeps_specs_in_registration_order() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool("echo")));
        registry.register(Arc::new(EchoTool("outra")));

        let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();

        assert_eq!(names, vec!["echo", "outra"]);
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nada").is_none());
        assert!(!registry.is_empty());
    }

    #[test]
    fn chat_request_can_carry_tools() {
        let request = ChatRequest::new("m", vec![Message::user("oi")]).with_tools(vec![ToolSpec {
            name: "read".into(),
            description: "lê".into(),
            schema: serde_json::json!({"type": "object"}),
        }]);

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "read");
    }

    #[test]
    fn default_describe_is_compact_json() {
        let tool = EchoTool("echo");
        assert_eq!(
            tool.describe_call(&serde_json::json!({"path": "a.rs"})),
            "{\"path\":\"a.rs\"}"
        );
    }

    #[test]
    fn tool_error_has_a_readable_message() {
        let error = ToolError::OutsideRoot("../../etc/passwd".into());
        assert_eq!(
            error.to_string(),
            "caminho fora do diretório raiz: ../../etc/passwd"
        );
    }
}
