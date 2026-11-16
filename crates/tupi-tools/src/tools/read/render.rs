use super::*;

impl ReadTool {
    pub(crate) fn render(
        &self,
        params: &ReadParams,
        path: &Path,
        content: &str,
        relative: &str,
    ) -> Result<Vec<Rendered>, ToolError> {
        match &params.symbol {
            Some(symbol) => self.symbol_sections(
                path,
                content,
                relative,
                symbol,
                params.repeat.as_deref() == Some("allow"),
            ),
            None => Ok(self.outline_sections(
                path,
                content,
                relative,
                params.repeat.as_deref() == Some("allow"),
            )),
        }
    }

    pub(super) fn outline_sections(
        &self,
        path: &Path,
        content: &str,
        relative: &str,
        repeat_allowed: bool,
    ) -> Vec<Rendered> {
        let snippets = self.chunker.snippets(path, content);
        if snippets.is_empty() {
            return vec![Rendered {
                section: whole_file_section(content, relative),
                pending: None,
            }];
        }

        let file_hash = ledger::content_hash(content);
        let short = ledger::short_hash(&file_hash);
        if !repeat_allowed
            && let Some((opened, unopened)) = self.backtrack.outline_summary(relative, &file_hash)
        {
            let mut body = format!(
                "> **Esqueleto já entregue nesta sessão** (blake3 `{short}`). Use `repeat: \"allow\"` para reexibir ou leia um símbolo específico.\n"
            );
            let mut parts = Vec::new();
            if !opened.is_empty() {
                parts.push(format!("já abertos: {}", format_symbol_list(&opened, 8)));
            }
            if !unopened.is_empty() {
                parts.push(format!(
                    "ainda não abertos: {}",
                    format_symbol_list(&unopened, 8)
                ));
            }
            if !parts.is_empty() {
                body.push_str(&format!("> **Backtrack**: {}\n", parts.join(" | ")));
            }
            return vec![Rendered {
                section: Section {
                    heading: format!(
                        "### `{relative}` — {} linhas · blake3 `{short}`",
                        content.lines().count()
                    ),
                    body,
                    priority: 255,
                },
                pending: None,
            }];
        }

        let symbols: Vec<String> = snippets
            .iter()
            .map(|snippet| snippet.qualified_name.clone())
            .collect();

        let mut sections: Vec<Rendered> = snippets
            .into_iter()
            .map(|snippet| {
                let label = if snippet.signature.is_empty() {
                    snippet.symbol_name
                } else {
                    snippet.signature
                };
                Rendered {
                    section: Section {
                        heading: format!(
                            "- `{label}` (:{}-{})",
                            snippet.start_line, snippet.end_line
                        ),
                        body: String::new(),
                        priority: 200,
                    },
                    pending: None,
                }
            })
            .collect();

        sections.insert(
            0,
            Rendered {
                section: Section {
                    heading: format!(
                        "### `{relative}` — {} linhas · blake3 `{short}`",
                        content.lines().count()
                    ),
                    body: String::new(),
                    priority: 255,
                },
                pending: Some(Pending::Outline {
                    relative: relative.to_string(),
                    file_hash,
                    symbols,
                }),
            },
        );
        sections
    }

    pub(super) fn symbol_sections(
        &self,
        path: &Path,
        content: &str,
        relative: &str,
        symbol: &str,
        repeat_allowed: bool,
    ) -> Result<Vec<Rendered>, ToolError> {
        let snippets = self.chunker.snippets(path, content);
        let file_hash = ledger::content_hash(content);
        let hash = ledger::short_hash(&file_hash);
        let graph = if relative.contains('@') {
            None
        } else {
            self.graph.as_ref().map(|cache| cache.get())
        };

        let mut sections = Vec::new();
        for snippet in snippets
            .into_iter()
            .filter(|snippet| snippet.symbol_name == symbol || snippet.qualified_name == symbol)
        {
            let display = snippet.qualified_name.clone();
            let heading = format!(
                "### `{display}` (`{relative}:{}-{}`) · blake3 `{hash}`",
                snippet.start_line, snippet.end_line
            );
            let fence = format!("```rust\n{}\n```\n", snippet.code);
            let mut body = fence.clone();
            let mut omitted = false;

            if !repeat_allowed {
                match self.ledger.check(
                    path,
                    &snippet.qualified_name,
                    &file_hash,
                    &snippet.code,
                    snippet.start_line,
                ) {
                    Verdict::Seen(seen) => {
                        let handle = self.live_handle(&seen, path, &snippet.qualified_name);
                        body = ledger::pointer_body(&seen, &handle);
                        if let Some(hint) = self
                            .backtrack
                            .backtrack_hint(relative, &snippet.qualified_name)
                            && let Some(line) = hint.format_line()
                        {
                            body.push_str(&line);
                        }
                        omitted = true;
                    }
                    Verdict::Changed(changed) => {
                        body = ledger::changed_body(&changed);
                    }
                    Verdict::Full { reason, .. } => {
                        body = format!("{}\n{fence}", ledger::full_note(reason));
                    }
                    Verdict::Fresh => {}
                }
            }

            let mut neighbor_refs: Vec<crate::backtrack::NeighborRef> = Vec::new();
            let mut neighbor_lines = Vec::new();
            if let Some(graph) = &graph
                && let Some(def) = graph.defs.iter().find(|def| {
                    def.relative == relative
                        && (def.qualified == snippet.qualified_name
                            || (def.name == snippet.symbol_name
                                && def.start_line == snippet.start_line))
                })
            {
                let callees = graph.callees_for(def);
                for callee in &callees {
                    neighbor_refs.push(crate::backtrack::NeighborRef {
                        relative: callee.relative.clone(),
                        symbol: callee.qualified.clone(),
                        line: callee.start_line,
                    });
                }
                if !omitted && !callees.is_empty() {
                    neighbor_lines.push(format_neighbors("chama", &callees));
                }
                let callers = graph.callers_for(def);
                for caller in &callers {
                    neighbor_refs.push(crate::backtrack::NeighborRef {
                        relative: caller.relative.clone(),
                        symbol: caller.qualified.clone(),
                        line: caller.start_line,
                    });
                }
                if !omitted && !callers.is_empty() {
                    neighbor_lines.push(format_neighbors("chamado por", &callers));
                }
            }

            sections.push(Rendered {
                section: Section {
                    heading,
                    body,
                    priority: 255,
                },
                pending: Some(Pending::Symbol {
                    path: path.to_path_buf(),
                    id: snippet.qualified_name.clone(),
                    file_hash,
                    text: snippet.code.clone(),
                    complete: true,
                    omitted,
                    relative: relative.to_string(),
                    neighbors: neighbor_refs,
                }),
            });

            if !omitted && !neighbor_lines.is_empty() {
                let first = neighbor_lines.remove(0);
                sections.push(Rendered {
                    section: Section {
                        heading: first,
                        body: if neighbor_lines.is_empty() {
                            String::new()
                        } else {
                            format!("{}\n", neighbor_lines.join("\n"))
                        },
                        priority: 180,
                    },
                    pending: None,
                });
            }
        }
        Ok(sections)
    }

    pub(super) fn live_handle(&self, seen: &Seen, path: &Path, id: &str) -> String {
        if let Some(handle) = &seen.handle
            && self.store.get(handle).is_some()
        {
            return handle.clone();
        }
        let handle = self.store.insert(seen.text.clone());
        self.ledger.remember_handle(path, id, &handle);
        handle
    }

    pub(crate) fn finish(&self, rendered: Vec<Rendered>, budget_tokens: Option<usize>) -> String {
        let budget = budget_tokens.unwrap_or(self.default_budget_tokens);
        let rendered: Vec<Rendered> = rendered
            .into_iter()
            .flat_map(|rendered| expand_over_budget(rendered, budget))
            .collect();

        let items: Vec<crate::support::dense::Item> = rendered
            .iter()
            .map(|rendered| crate::support::dense::Item {
                section: rendered.section.clone(),
                track: None,
            })
            .collect();

        // O helper ordena por prioridade antes de cortar; refazer a seleção aqui
        // só serve para o livro-caixa saber o que entrou de fato.
        let mut order: Vec<usize> = (0..rendered.len()).collect();
        order.sort_by(|&a, &b| rendered[b].section.priority.cmp(&rendered[a].section.priority));
        let ordered: Vec<Section> = order
            .iter()
            .map(|&idx| rendered[idx].section.clone())
            .collect();
        let selection = budget::select(&ordered, budget);
        self.settle(&rendered, &selection);

        crate::support::dense::dense(items, budget, &self.ledger, &self.store)
    }

    pub(super) fn settle(&self, rendered: &[Rendered], selection: &budget::Selection) {
        let included: HashSet<&str> = selection
            .included
            .iter()
            .map(|section| section.heading.as_str())
            .collect();

        let mut tally: HashMap<(&Path, &str), (usize, usize)> = HashMap::new();
        for rendered in rendered {
            let Some(Pending::Symbol { path, id, .. }) = &rendered.pending else {
                continue;
            };
            let entry = tally.entry((path.as_path(), id.as_str())).or_insert((0, 0));
            entry.0 += 1;
            if included.contains(rendered.section.heading.as_str()) {
                entry.1 += 1;
            }
        }

        let mut handled: HashSet<(&Path, &str)> = HashSet::new();
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            match pending {
                Pending::Outline {
                    relative,
                    file_hash,
                    symbols,
                } => {
                    if !included.contains(rendered.section.heading.as_str()) {
                        continue;
                    }
                    self.backtrack.record_visit(
                        relative,
                        crate::backtrack::OUTLINE_SYMBOL,
                        *file_hash,
                        crate::backtrack::VisitLevel::BodySeen,
                        Some("read: outline"),
                    );
                    for sym in symbols {
                        self.backtrack.record_visit(
                            relative,
                            sym,
                            *file_hash,
                            crate::backtrack::VisitLevel::SignatureSeen,
                            Some("read: outline"),
                        );
                    }
                }
                Pending::Symbol {
                    path,
                    id,
                    file_hash,
                    text,
                    complete,
                    omitted,
                    relative,
                    neighbors,
                } => {
                    let key = (path.as_path(), id.as_str());
                    if !handled.insert(key) {
                        continue;
                    }
                    let (_, seen) = tally.get(&key).copied().unwrap_or((1, 0));
                    if seen == 0 {
                        continue;
                    }
                    // Entrega parcial pelo orçamento ainda é entrega: registrar
                    // aqui faz a próxima leitura devolver um ponteiro com handle
                    // em vez de repetir o mesmo trecho truncado.
                    if *omitted {
                        self.ledger.note_omitted(path, id);
                    } else {
                        self.ledger.record(Delivery {
                            tool: "read",
                            path,
                            id,
                            file_hash: *file_hash,
                            text,
                            complete: *complete,
                            handle: None,
                        });
                        self.backtrack.record_visit(
                            relative,
                            id,
                            *file_hash,
                            crate::backtrack::VisitLevel::BodySeen,
                            Some("read"),
                        );
                        self.backtrack
                            .record_neighbors(relative, id, *file_hash, neighbors);
                    }
                }
            }
        }
    }
}

pub(super) fn expand_over_budget(rendered: Rendered, budget: usize) -> Vec<Rendered> {
    let parts = budget::split_section(&rendered.section, budget);
    if parts.len() <= 1 {
        return vec![rendered];
    }
    let pending = rendered.pending;
    parts
        .into_iter()
        .map(|section| Rendered {
            section,
            pending: pending.clone(),
        })
        .collect()
}

pub(super) fn whole_file_section(content: &str, relative: &str) -> Section {
    let lang = Path::new(relative)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    Section {
        heading: format!(
            "### `{relative}` — arquivo inteiro · blake3 `{}`",
            ledger::short_hash(&ledger::content_hash(content))
        ),
        body: format!("```{lang}\n{content}\n```\n"),
        priority: 255,
    }
}
