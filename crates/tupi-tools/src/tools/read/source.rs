use super::*;

impl ReadTool {
    pub(crate) fn prepare_rev(
        &self,
        params: &ReadParams,
        rev: &str,
    ) -> Result<(PathBuf, String, String), ToolError> {
        let rev = rev.trim();
        if rev.is_empty()
            || rev.starts_with('-')
            || rev
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace() || ch == ':')
        {
            return Err(ToolError::InvalidArguments(format!("rev inválida: {rev}")));
        }
        let root = self
            .root_dir
            .canonicalize()
            .map_err(|_| ToolError::OutsideRoot(params.path.clone()))?;
        let candidate = Path::new(&params.path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let normalized = crate::paths::normalize(&joined);
        if !normalized.starts_with(&root) {
            return Err(ToolError::OutsideRoot(params.path.clone()));
        }
        let relative = normalized
            .strip_prefix(&root)
            .unwrap_or(&normalized)
            .display()
            .to_string();
        let output = crate::proc::program(
            "git",
            &[
                "show".to_string(),
                "--end-of-options".to_string(),
                format!("{rev}:{relative}"),
            ],
            &root,
            std::time::Duration::from_secs(30),
        )?;
        if !output.success {
            return Err(ToolError::NotFound(format!("{rev}:{relative}")));
        }
        Ok((normalized, output.stdout, format!("{relative}@{rev}")))
    }

    pub(crate) fn rev_sections(
        &self,
        params: &ReadParams,
        rev: &str,
    ) -> Result<(String, Vec<Section>), ToolError> {
        let (path, content, relative) = self.prepare_rev(params, rev)?;
        let request = ReadParams {
            path: params.path.clone(),
            symbol: params.symbol.clone(),
            budget_tokens: params.budget_tokens,
            repeat: Some("allow".to_string()),
            rev: None,
        };
        let rendered = self.render(&request, &path, &content, &relative)?;
        if rendered.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "símbolo `{}` não encontrado em `{relative}`",
                params.symbol.as_deref().unwrap_or_default()
            )));
        }
        let hash = crate::ledger::short_hash(&crate::ledger::content_hash(&content));
        let mut reference = format!("[ref] {relative} @{hash} (somente leitura)");
        if let Some(symbol) = &params.symbol {
            reference.push_str(" :: ");
            reference.push_str(symbol);
        }
        Ok((
            reference,
            rendered.into_iter().map(|item| item.section).collect(),
        ))
    }

    pub(crate) fn prepare(
        &self,
        params: &ReadParams,
    ) -> Result<(PathBuf, String, String), ToolError> {
        let candidate = Path::new(&params.path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root_dir.join(candidate)
        };

        let canonical = joined
            .canonicalize()
            .map_err(|_| ToolError::NotFound(params.path.clone()))?;
        let root = self
            .root_dir
            .canonicalize()
            .map_err(|_| ToolError::OutsideRoot(params.path.clone()))?;

        if !canonical.starts_with(&root) {
            return Err(ToolError::OutsideRoot(params.path.clone()));
        }

        let content = std::fs::read_to_string(&canonical).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ToolError::NotFound(params.path.clone())
            } else {
                ToolError::Other(format!("falha ao ler {}: {error}", canonical.display()))
            }
        })?;

        let relative = canonical
            .strip_prefix(&root)
            .unwrap_or(&canonical)
            .display()
            .to_string();

        Ok((canonical, content, relative))
    }
}
