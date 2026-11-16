use super::render::{capped_body, completeness_line};
use super::*;
use crate::budget::{self, estimate_tokens};
use crate::ledger::{self, Delivery, Seen, Verdict};

impl SearchTool {
    pub fn search(&self, params: SearchParams) -> Result<String, ToolError> {
        let terms = extract_terms(&params.query, &params.keywords);
        if terms.is_empty() {
            return Err(ToolError::InvalidArguments(
                "query sem termos de busca (só stopwords ou vazia)".to_string(),
            ));
        }

        let max_results = params
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_MAX_RESULTS);
        let detail = params.detail.unwrap_or_default();
        let budget = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        let repeat_allowed = params.repeat.as_deref() == Some("allow");

        let snapshot = self.space.as_ref().and_then(|space| space.snapshot());
        let dense_possible = snapshot.is_some() && self.current_embedder().is_some();
        let matches = self.scanner.search(&terms, params.path.as_deref())?;
        if matches.is_empty() && !dense_possible {
            return Ok(format!(
                "Nenhum resultado encontrado para `{}`.",
                params.query.trim()
            ));
        }

        let mut by_file: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for found in matches {
            by_file
                .entry(found.file_path)
                .or_default()
                .push(found.line_number);
        }

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let chunker = &self.chunker;
        let mut files: Vec<(PathBuf, String, String, blake3::Hash)> =
            Vec::with_capacity(by_file.len());
        let mut matched_hashes: HashMap<String, blake3::Hash> =
            HashMap::with_capacity(by_file.len());
        let mut snippets: Vec<AstSnippet> = Vec::new();
        let mut file_indices: Vec<usize> = Vec::new();
        for (path, lines) in by_file {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut file_snippets = chunker.snippets_at_lines(&path, &content, &lines);
            if file_snippets.is_empty() {
                continue;
            }
            file_snippets.sort_by_key(|snippet| snippet.start_line);
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let file_hash = ledger::content_hash(&content);
            matched_hashes.insert(relative.clone(), file_hash);
            let file_idx = files.len();
            files.push((path, content, relative, file_hash));
            file_indices.resize(file_indices.len() + file_snippets.len(), file_idx);
            snippets.extend(file_snippets);
        }

        if snippets.is_empty() && !dense_possible {
            return Ok(format!(
                "Termos encontrados, porém nenhum bloco de código pôde ser delimitado para `{}`.",
                params.query.trim()
            ));
        }

        let dense_indices = if dense_possible {
            let snapshot = snapshot.as_ref().expect("snapshot checado");
            self.dense_snippets(
                snapshot,
                &params.query,
                params.path.as_deref(),
                &mut snippets,
                &mut files,
                &mut file_indices,
                &mut matched_hashes,
            )
        } else {
            Vec::new()
        };
        if snippets.is_empty() {
            return Ok(format!(
                "Nenhum resultado encontrado para `{}`.",
                params.query.trim()
            ));
        }
        let lexical_ranked = LexicalRanker::new().rank_indices(&terms, &snippets);
        let mut ranked: Vec<(usize, f32)> = if dense_indices.is_empty() {
            lexical_ranked
        } else {
            let lexical_indices: Vec<usize> = lexical_ranked
                .into_iter()
                .filter(|(_, score)| *score > 0.0)
                .map(|(index, _)| index)
                .collect();
            let fused =
                super::fusion::rrf(&lexical_indices, &dense_indices, 3.0, 60.0, snippets.len());
            fused
                .into_iter()
                .enumerate()
                .map(|(position, index)| (index, 1.0 - position as f32 * 0.001))
                .collect()
        };
        if !repeat_allowed {
            ranked.sort_by(|&(idx_a, score_a), &(idx_b, score_b)| {
                let (_, _, ref rel_a, ref hash_a) = files[file_indices[idx_a]];
                let (_, _, ref rel_b, ref hash_b) = files[file_indices[idx_b]];
                let tier_a =
                    match self
                        .backtrack
                        .visit_level(rel_a, &snippets[idx_a].qualified_name, hash_a)
                    {
                        crate::backtrack::VisitLevel::BodySeen => 2u8,
                        crate::backtrack::VisitLevel::SignatureSeen => 1u8,
                        crate::backtrack::VisitLevel::Unseen
                        | crate::backtrack::VisitLevel::Frontier => 0u8,
                    };
                let tier_b =
                    match self
                        .backtrack
                        .visit_level(rel_b, &snippets[idx_b].qualified_name, hash_b)
                    {
                        crate::backtrack::VisitLevel::BodySeen => 2u8,
                        crate::backtrack::VisitLevel::SignatureSeen => 1u8,
                        crate::backtrack::VisitLevel::Unseen
                        | crate::backtrack::VisitLevel::Frontier => 0u8,
                    };
                tier_a
                    .cmp(&tier_b)
                    .then_with(|| score_b.total_cmp(&score_a))
                    .then_with(|| idx_a.cmp(&idx_b))
            });
        }

        let total = snippets.len();
        let query_label = format!("search: {}", params.query.trim());
        let cut_label = format!("search(cortado): {}", params.query.trim());
        for &(index, _) in ranked.iter().skip(max_results) {
            let (_, _, ref relative, file_hash) = files[file_indices[index]];
            self.backtrack.record_visit(
                relative,
                &snippets[index].qualified_name,
                file_hash,
                crate::backtrack::VisitLevel::Frontier,
                Some(&cut_label),
            );
        }

        let query_seen = !repeat_allowed
            && self
                .backtrack
                .was_query_seen(&terms, params.path.as_deref(), &matched_hashes);

        let selected: Vec<(usize, f32)> = ranked.into_iter().take(max_results).collect();

        let mut rendered = Vec::with_capacity(selected.len());
        let mut full_bodies = 0usize;
        let mut full_tokens = 0usize;
        let mut signature_only = 0usize;

        for (position, (index, score)) in selected.iter().enumerate() {
            let (path, content, relative, file_hash) = &files[file_indices[*index]];
            let snippet = &snippets[*index];

            let heading = format!(
                "#### {}. `{}` (`{}:{}-{}`)",
                position + 1,
                snippet.symbol_name,
                relative,
                snippet.start_line,
                snippet.end_line
            );

            let code_tokens = estimate_tokens(&snippet.code);
            let wants_full = match detail {
                SearchDetail::Full => true,
                SearchDetail::Signatures => false,
                SearchDetail::Auto => {
                    full_bodies < MAX_FULL_BODIES_IN_AUTO
                        && (full_bodies == 0 || full_tokens + code_tokens <= budget * 2 / 3)
                }
            };

            if !wants_full {
                signature_only += 1;
                let prior =
                    self.backtrack
                        .visit_level(relative, &snippet.qualified_name, file_hash);
                let marker = if !repeat_allowed {
                    match prior {
                        crate::backtrack::VisitLevel::BodySeen => " *(já entregue nesta sessão)*",
                        crate::backtrack::VisitLevel::SignatureSeen => " *(assinatura já vista)*",
                        crate::backtrack::VisitLevel::Unseen
                        | crate::backtrack::VisitLevel::Frontier => "",
                    }
                } else {
                    ""
                };
                let body = if snippet.signature.is_empty() {
                    if marker.is_empty() {
                        String::new()
                    } else {
                        format!("{}\n\n", marker.trim_start())
                    }
                } else {
                    format!("`{}`{marker}\n\n", compact_signature(&snippet.signature))
                };
                rendered.push(Rendered {
                    section: Section {
                        heading,
                        body,
                        priority: (score * 200.0) as u8,
                    },
                    pending: Some(Pending {
                        path: path.clone(),
                        relative: relative.clone(),
                        id: snippet.qualified_name.clone(),
                        file_hash: *file_hash,
                        text: String::new(),
                        complete: false,
                        omitted: false,
                        record_ledger: false,
                        visit_level: crate::backtrack::VisitLevel::SignatureSeen,
                        via: query_label.clone(),
                    }),
                });
                continue;
            }

            let (code, capped) = capped_body(snippet);
            let mut body = format!("```rust\n{code}\n```\n");
            let candidate = Candidate {
                path: path.clone(),
                content: content.clone(),
                snippet: snippet.clone(),
            };
            let pending = self.apply_ledger(
                &candidate,
                relative,
                &query_label,
                &mut body,
                capped,
                repeat_allowed,
            );
            let is_pointer = pending.as_ref().is_some_and(|p| p.omitted);
            if is_pointer {
                if let Some(hint) = self
                    .backtrack
                    .backtrack_hint(relative, &snippet.qualified_name)
                    && let Some(line) = hint.format_line()
                {
                    body.push_str(&line);
                }
                full_tokens += estimate_tokens(&body) + estimate_tokens(&heading);
            } else {
                full_bodies += 1;
                full_tokens += code_tokens + estimate_tokens(&heading);
            }

            rendered.push(Rendered {
                section: Section {
                    heading,
                    body,
                    priority: (score * 200.0) as u8,
                },
                pending,
            });
        }

        let completeness = completeness_line(total, selected.len(), max_results, signature_only);
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
        let body = crate::support::dense::dense(items, budget, &self.ledger, &self.store);
        self.backtrack
            .record_query(&terms, params.path.as_deref(), matched_hashes);

        let mut out = format!("### Resultados da busca\n{completeness}\n\n");
        if let Some(snapshot) = snapshot.as_ref()
            && snapshot.entries.len() < snapshot.expected_symbols
        {
            out.push_str(&format!(
                "espaço: {}/{} símbolos indexados (build em andamento)\n\n",
                snapshot.entries.len(),
                snapshot.expected_symbols
            ));
        }
        if query_seen {
            out.push_str(
                "> **Backtrack**: busca equivalente já realizada nesta sessão; priorizando pontos ainda não abertos.\n\n",
            );
        }
        if signature_only > 0 {
            out.push_str(&format!(
                "> {signature_only} resultado(s) só com assinatura.\n\n"
            ));
        }
        out.push_str(&body);
        Ok(out)
    }

    pub(super) fn apply_ledger(
        &self,
        candidate: &Candidate,
        relative: &str,
        via: &str,
        body: &mut String,
        capped: bool,
        repeat_allowed: bool,
    ) -> Option<Pending> {
        let snippet = &candidate.snippet;
        let range = ledger::line_span(&candidate.content, snippet.start_line, snippet.end_line)?;
        let span_text = candidate.content.get(range)?;
        let complete = !capped && span_text.trim_end() == snippet.code.trim_end();
        let file_hash = ledger::content_hash(&candidate.content);
        let id = snippet.qualified_name.clone();

        let draft = Pending {
            path: candidate.path.clone(),
            relative: relative.to_string(),
            id: id.clone(),
            file_hash,
            text: span_text.to_string(),
            complete,
            omitted: false,
            record_ledger: true,
            visit_level: crate::backtrack::VisitLevel::BodySeen,
            via: via.to_string(),
        };

        if repeat_allowed {
            return Some(draft);
        }

        match self.ledger.check(
            &candidate.path,
            &id,
            &file_hash,
            span_text,
            snippet.start_line,
        ) {
            Verdict::Seen(seen) => {
                let handle = self.live_handle(&seen, &candidate.path, &id);
                *body = ledger::pointer_body(&seen, &handle);
                Some(Pending {
                    omitted: true,
                    ..draft
                })
            }
            Verdict::Changed(changed) => {
                *body = ledger::changed_body(&changed);
                Some(draft)
            }
            Verdict::Full { reason, .. } => {
                *body = format!("{}\n{body}", ledger::full_note(reason));
                Some(draft)
            }
            Verdict::Fresh => Some(draft),
        }
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

    pub(super) fn settle(&self, rendered: &[Rendered], selection: &budget::Selection) {
        let included: std::collections::HashSet<&str> = selection
            .included
            .iter()
            .map(|section| section.heading.as_str())
            .collect();

        let mut handled: HashMap<(&Path, &str), bool> = HashMap::new();
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let key = (pending.path.as_path(), pending.id.as_str());
            let included_here = included.contains(rendered.section.heading.as_str());

            match handled.get_mut(&key) {
                Some(seen) => *seen |= included_here,
                None => {
                    handled.insert(key, included_here);
                }
            }
        }

        let graph = self.graph.as_ref().map(|cache| cache.get());
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let key = (pending.path.as_path(), pending.id.as_str());
            let Some(true) = handled.get(&key).copied() else {
                continue;
            };
            if handled.remove(&key).is_none() {
                continue;
            }
            if !pending.record_ledger {
                self.backtrack.record_visit(
                    &pending.relative,
                    &pending.id,
                    pending.file_hash,
                    pending.visit_level,
                    Some(&pending.via),
                );
            } else if pending.omitted {
                self.ledger.note_omitted(&pending.path, &pending.id);
            } else {
                self.ledger.record(Delivery {
                    tool: "search",
                    path: &pending.path,
                    id: &pending.id,
                    file_hash: pending.file_hash,
                    text: &pending.text,
                    complete: pending.complete,
                    handle: None,
                });
                self.backtrack.record_visit(
                    &pending.relative,
                    &pending.id,
                    pending.file_hash,
                    pending.visit_level,
                    Some(&pending.via),
                );
                if let Some(graph) = &graph
                    && let Some(def) = graph
                        .defs
                        .iter()
                        .find(|def| def.relative == pending.relative && def.qualified == pending.id)
                {
                    let mut neighbor_refs = Vec::new();
                    for callee in graph.callees_for(def) {
                        neighbor_refs.push(crate::backtrack::NeighborRef {
                            relative: callee.relative.clone(),
                            symbol: callee.qualified.clone(),
                            line: callee.start_line,
                        });
                    }
                    for caller in graph.callers_for(def) {
                        neighbor_refs.push(crate::backtrack::NeighborRef {
                            relative: caller.relative.clone(),
                            symbol: caller.qualified.clone(),
                            line: caller.start_line,
                        });
                    }
                    self.backtrack.record_neighbors(
                        &pending.relative,
                        &pending.id,
                        pending.file_hash,
                        &neighbor_refs,
                    );
                }
            }
        }
    }
}
