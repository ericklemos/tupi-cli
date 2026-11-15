<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Ajuste de perfis e configuração Rust para performance (`rust-perf-config`)

**Status:** failed

## Objetivo

Reduzir o tempo de build e a latência das operações quentes das tools (grafo de código, `search`, `map`) no workspace tupi por meio de configuração de build/runtime Rust, com ganho comprovado por medição antes/depois. Estado atual verificado: o workspace não declara nenhum perfil de build (`Cargo.toml:1-68`, sem `[profile.*]`), não existe `.cargo/config.toml` (leitura em `.cargo/config.toml` retornou arquivo não encontrado) e o único binário é `tupi` (`crates/tupi/Cargo.toml:1-35`). Nenhum `#[global_allocator]` é definido no binário (`crates/tupi/src/main.rs:19`).

## Restrições

- Somente config/estrutura: nada de reescrita algorítmica ampla; hotspots de runtime entram como tasks separadas e opcionais.
- Sem novas dependências obrigatórias: alocador alternativo (mimalloc/jemalloc) deve ser feature opcional, com fallback ao alocador padrão.
- Portabilidade preservada: `target-cpu=native` não pode virar padrão do repositório; se adotado, vai atrás de flag/perfil local explícito.
- `panic = "abort"` só entra se for aceito explicitamente, pois um panic em qualquer task derruba o processo inteiro na TUI/servidor web.
- Sem baseline medido, nenhuma mudança é considerada ganho: cada item precisa de número antes/depois.
- Manter `crates/tupi` como único binário (`Cargo.toml:2-13`).
- Toda mudança é reversível por um único commit (bloco `[profile.*]` ou arquivo `.cargo/config.toml`), para permitir descarte quando a medição não mostrar ganho.

## Nuances

- Nenhum `[profile.release]` existe hoje: o release usa `opt-level 3`, `codegen-units 16`, `lto off`, `panic unwind`, `strip none` (padrão do Cargo) — `Cargo.toml:1-68`.
- O resolver está fixado em `"2"` no topo do workspace (`Cargo.toml:2`), embora edition seja 2024 (`Cargo.toml:15`); `resolver = "3"` adiciona resolução ciente de MSRV e reduz duplicação de versões — não é ganho de runtime, é higiene de build.
- `tokio` é declarado com `features = ["full"]` (`Cargo.toml:31`) para todos os crates, incluindo `tupi-store`/`tupi-tools` que não precisam de rede.
- `crates/tupi/Cargo.toml:18` liga `tupi-infer` com `local-models` sempre, puxando `llama-cpp-2` (`crates/tupi-infer/Cargo.toml:12,18`) — custo alto de compilação nativa em todo build do binário.
- `GraphBuilder::build` faz walk serial (`crates/tupi-tools/src/support/graph/builder.rs:389-411`) e loop serial de `read_to_string` + parse tree-sitter por arquivo (`builder.rs:60-386`), mantendo `content` de todos os arquivos vivos em `ParsedFile` — pico de RSS no repo inteiro.
- `GraphCache::get` executa o build segurando o write lock (`crates/tupi-tools/src/support/graph/cache.rs:46-63`): chamadas concorrentes de tools serializam no primeiro build.
- Já existe cache de parse por arquivo (`crates/tupi-tools/src/support/graph/parse_cache.rs`, com `with_max_bytes`), então paralelizar o parse é seguro desde que o cache continue sendo o ponto de entrada.
- `blake3` já faz detecção de features de CPU em runtime; `target-cpu=native` dá ganho marginal para hash e não muda o principal custo (I/O + tree-sitter + alocação).
- O binário tem TUI (ratatui/crossterm) e servidor web (axum) sempre compilados (`crates/tupi/Cargo.toml:7-30`); gatear por feature reduz tempo de build, não latência.
- DECISÃO PENDENTE (Q1 — alvo primário): priorizado 'ambos, build primeiro' como default; T1/T5 executam antes de T6. Reverter se o usuário priorizar latência.
- DECISÃO PENDENTE (Q2 — distribuição): assumido build local/desenvolvimento, logo `target-cpu=native` permanece apenas documentado e opt-in (T4.2), nunca no config versionado.
- DECISÃO PENDENTE (Q3 — `panic = "abort"`): NÃO aplicado por padrão. Sem `catch_unwind` no código, mas abort converteria falha isolada de task em queda da TUI/servidor; entra só se o usuário aceitar explicitamente.
- DECISÃO PENDENTE (Q4 — dependência nova): permitida apenas como feature opcional (T3); se T3.2 não medir ganho, a dependência é removida.
- DECISÃO PENDENTE (Q5 — baseline): não há baseline numérico conhecido hoje; T1.1 e T2 existem justamente para criá-lo antes de qualquer otimização.
- DECISÃO PENDENTE (Q6 — features do binário): build default permanece com `local-models`+TUI+web; T5.2 adiciona apenas a possibilidade de desligá-las, sem alterar o default.

