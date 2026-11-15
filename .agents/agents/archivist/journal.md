# Journal — The Archivist 📚

Só descobertas críticas: padrões que exigiram várias observações, conhecimento quase perdido, contradições resolvidas. Nada de log de rotina.

## Sessão 1 — as regras nascem, o índice ainda não

> Nenhuma tool desta sessão expõe o relógio; a data fica em aberto em vez de inventada.

- **Padrão que se repetiu quatro vezes:** sparks, skeptic, optimizer e gatekeeper descrevem o mesmo desenho de estado — `memory.md` como índice compacto, `journal.md` para descobertas, `.agents/journals/<slug>.md` quando o workflow exige. Quatro ocorrências sem nome é uma convenção pedindo estatuto; virou `docs/rules/knowledge-placement.md`, com o complemento que faltava: o que governa o time mora em `docs_folders`, não em `.agents/`.

- **Conhecimento quase perdido:** `docs/` existia e estava vazio — pasta citada por comando, por config e por workflow, sem uma única linha de índice. Quem chegasse depois concluiria que ela não é usada. `docs/rules/` nasce aqui.

- **Contradição resolvida (não virou regra):** a memória afirmava "`docs_folders` vem vazio por padrão (`project_config.rs:281-288`)". A leitura do código mostra dois casos distintos: arquivo ausente → `fallback` (`crates/tupi/src/project_config.rs:314-322`) devolve `vec![DEFAULT_DOCS_FOLDER]`; arquivo presente sem a chave → `#[serde(default)]` produz lista vazia. A regra ficou escrita sobre o efeito observável (pasta não declarada não é indexada), não sobre o padrão alegado.

- **Descartado de propósito:** os erros de compilação em `crates/tupi-providers/src/antigravity.rs` vistos na memória desta sessão e o aviso `unused import: sdd::*` em `crates/tupi-store/src/lib.rs:337`. São estado transitório, não princípio — a segunda ocorrência é que decide.
