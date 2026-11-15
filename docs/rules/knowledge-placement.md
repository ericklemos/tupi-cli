# Estado de agente em `.agents/`, conhecimento de projeto em `docs_folders`

**Context:** Os workflows nativos guardam estado em `.agents/agents/<slug>/memory.md` (índice compacto) e `.agents/journals/<slug>.md` (descobertas), prática seguida por spark, skeptic, optimizer e gatekeeper. Artefatos que governam o time — regras e documentação de projeto — precisam estar sob `docs_folders`, as únicas pastas que a tool `docs` varre (`DocsIndex::new`, `crates/tupi-tools/src/tools/docs/index.rs:58-69`). Regra enterrada em `.agents/` existe no disco e não existe para a busca.
**Rule:** Agentes DEVEM persistir memória e journal em `.agents/agents/<slug>/{memory,journal}.md` (mais `.agents/journals/<slug>.md` quando o workflow exigir) e DEVEM gravar documentação de projeto e regras somente sob a primeira pasta de `docs_folders` (padrão `docs/`). NUNCA gravar regra de projeto ou doc de projeto em `.agents/`.
**Example (Do):** `.agents/agents/archivist/memory.md` apenas indexa; a regra em si vive em `docs/rules/workflows.md`.
**Example (Don't):** salvar a regra em `.agents/agents/archivist/rules/workflows.md` — fora do índice de docs, nenhum outro agente a encontra.
