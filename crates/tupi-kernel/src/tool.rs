use std::sync::Arc;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// O array `tools` exatamente como o provider o envia. O orçamento de
/// contexto conta tokens sobre esta mesma forma: estimar um JSON parecido
/// mas não igual (sem aspas no nome, sem o envelope `function`) fazia a
/// contagem divergir do que é cobrado de verdade.
pub fn tools_payload(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": spec.name,
                    "description": spec.description,
                    "parameters": spec.schema,
                }
            })
        })
        .collect()
}

thread_local! {
    static TOOL_CANCEL: std::cell::RefCell<Option<Arc<std::sync::atomic::AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

pub fn with_tool_cancel<F, R>(flag: Arc<std::sync::atomic::AtomicBool>, f: F) -> R
where
    F: FnOnce() -> R,
{
    struct Guard(Option<Arc<std::sync::atomic::AtomicBool>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            let prev = self.0.take();
            TOOL_CANCEL.with(|cell| {
                *cell.borrow_mut() = prev;
            });
        }
    }
    let prev = TOOL_CANCEL.with(|cell| cell.borrow_mut().replace(flag));
    let _guard = Guard(prev);
    f()
}

pub fn is_tool_cancelled() -> bool {
    TOOL_CANCEL.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
    })
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
    #[error("símbolo ambíguo: {0}")]
    Ambiguous(String),
    #[error("arquivo mudou desde a leitura: {0}")]
    StaleHash(String),
    #[error("sintaxe inválida após a edição: {0}")]
    InvalidSyntax(String),
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

    /// A call altera o workspace (ou o estado durável): o runner não a
    /// executa em paralelo com outra call da mesma step.
    fn mutates(&self) -> bool {
        false
    }

    /// Fail-closed: apenas tools seguras para investigação/SDD retornam true.
    fn sdd_allowed(&self) -> bool {
        false
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

    pub fn specs_for_mode(&self, mode: crate::AgentMode) -> Vec<ToolSpec> {
        match mode {
            crate::AgentMode::Execution => self.specs(),
            crate::AgentMode::Sdd => self
                .tools
                .iter()
                .filter(|tool| tool.sdd_allowed())
                .map(|tool| tool.spec())
                .collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools
            .iter()
            .find(|tool| tool.spec().name == name)
            .cloned()
    }

    /// `true` se há uma tool registrada com esse nome.
    pub fn has(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.spec().name == name)
    }

    /// Remove e devolve as tools registradas, na ordem de registro.
    pub fn into_tools(mut self) -> Vec<Arc<dyn Tool>> {
        std::mem::take(&mut self.tools)
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
            r#"{"path":"a.rs"}"#
        );
    }

    #[test]
    fn the_tools_payload_is_the_json_the_provider_sends() {
        let payload = tools_payload(&[ToolSpec {
            name: "read".into(),
            description: "lê".into(),
            schema: serde_json::json!({"type": "object"}),
        }]);

        assert_eq!(
            payload,
            vec![serde_json::json!({
                "type": "function",
                "function": {
                    "name": "read",
                    "description": "lê",
                    "parameters": {"type": "object"},
                }
            })]
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

    #[test]
    fn agent_mode_defaults_to_execution_and_specs_for_mode_filters_sdd() {
        assert_eq!(crate::AgentMode::default(), crate::AgentMode::Execution);
        assert_eq!(
            crate::AgentMode::from_u8(crate::AgentMode::Sdd.as_u8()),
            crate::AgentMode::Sdd
        );

        struct AllowedTool;
        impl Tool for AllowedTool {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "read".into(),
                    description: "".into(),
                    schema: serde_json::json!({}),
                }
            }
            fn call(&self, _: Value) -> Result<String, ToolError> {
                Ok("ok".into())
            }
            fn sdd_allowed(&self) -> bool {
                true
            }
        }

        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(EchoTool("edit")));
        reg.register(Arc::new(AllowedTool));

        let exec_names: Vec<_> = reg
            .specs_for_mode(crate::AgentMode::Execution)
            .into_iter()
            .map(|s| s.name)
            .collect();
        let sdd_names: Vec<_> = reg
            .specs_for_mode(crate::AgentMode::Sdd)
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(exec_names, vec!["edit", "read"]);
        assert_eq!(sdd_names, vec!["read"]);
    }
}
