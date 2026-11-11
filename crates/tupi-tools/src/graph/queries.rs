use tupi_kernel::ToolError;
use tree_sitter::Query;

use crate::ast::Language;

pub struct GraphQueries {
    #[allow(dead_code)]
    pub rust_defs: Query,
    pub rust_calls: Query,
}

impl GraphQueries {
    pub fn new() -> Result<Self, ToolError> {
        let rust = tree_sitter_rust::LANGUAGE.into();

        let rust_defs = Query::new(
            &rust,
            r#"
            (function_item name: (identifier) @name) @def
            (struct_item name: (type_identifier) @name) @def
            (enum_item name: (type_identifier) @name) @def
            (trait_item name: (type_identifier) @name) @def
            "#,
        )
        .map_err(|error| ToolError::Other(format!("query de definições Rust: {error}")))?;

        let rust_calls = Query::new(
            &rust,
            r#"
            (call_expression function: (identifier) @call.name)
            (call_expression function: (field_expression value: (identifier) @call.receiver field: (field_identifier) @call.name))
            (call_expression function: (scoped_identifier path: (identifier) @call.receiver name: (identifier) @call.name))
            (call_expression function: (scoped_identifier name: (identifier) @call.name) @call.path)
            (call_expression function: (scoped_identifier path: (scoped_identifier) @call.path name: (identifier) @call.name))
            (call_expression arguments: (arguments (identifier) @call.name))
            (call_expression arguments: (arguments (scoped_identifier name: (identifier) @call.name)))
            (macro_invocation macro: (identifier) @call.name)
            "#,
        )
        .map_err(|error| ToolError::Other(format!("query de chamadas Rust: {error}")))?;

        Ok(Self {
            rust_defs,
            rust_calls,
        })
    }

    #[allow(dead_code)]
    pub fn defs_query(&self, language: Language) -> &Query {
        match language {
            Language::Rust => &self.rust_defs,
        }
    }

    pub fn calls_query(&self, language: Language) -> &Query {
        match language {
            Language::Rust => &self.rust_calls,
        }
    }
}
