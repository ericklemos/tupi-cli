<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Resultado — Ajuste de perfis e configuração Rust para performance (`rust-perf-config`)

**Status:** completed
**Início:** 1794705202
**Fim:** 1794705350

## Resumo

- tasks: 6/6
- subtasks: 16/16 ok
- falhas: 2

## Verificação

- nenhum aceite registrado

## Docs

- status: updated
- arquivos: docs/perf/baseline.md, docs/index.md
- reindexado: sim
- motivo: Docs atualizados e reindexados (13 arquivos, 63 chunks, 0 pendentes).

**Conclusão para result.md**

Conhecimento durável revisado em `docs/perf/`: `baseline.md` foi corrigido onde estava obsoleto — as "Lacunas" ainda diziam que alocador (T3), tempo de link (T4.1), tamanho do binário release e pico de RSS não tinham medição, quando `allocator-and-linker.md` já os medira e descartara; a linha de T5.2 afirmava `transcribe` gateável, mas o `crates/tupi/Cargo.toml` atual mantém `tupi-transcribe` sempre compilado, com apenas `cuda`/`metal`/`vulkan` escolhendo backend. Também registrei que `.cargo/config.toml` agora existe com só `[env] CUDACXX` (nenhum `rustflags`/linker — portabilidade preservada) e que o bloqueio do caminho headless desapareceu (`install::run` existe em `crates/tupi/src/install.rs:1232`). `docs/index.md` ganhou a seção "Performance" com os três arquivos.

Atenção: a conclusão auto-gerada no topo deste arquivo ("0/16 subtasks ok, T1.1 falhou") está defasada — `tasks.json` marca 6/6 tasks e 16/16 subtasks como `done` e `meta.json` registra `plan_completed`.

## Conclusão

Docs atualizados e reindexados (13 arquivos, 63 chunks, 0 pendentes).

**Conclusão para result.md**

Conhecimento durável revisado em `docs/perf/`: `baseline.md` foi corrigido onde estava obsoleto — as "Lacunas" ainda diziam que alocador (T3), tempo de link (T4.1), tamanho do binário release e pico de RSS não tinham medição, quando `allocator-and-linker.md` já os medira e descartara; a linha de T5.2 afirmava `transcribe` gateável, mas o `crates/tupi/Cargo.toml` atual mantém `tupi-transcribe` sempre compilado, com apenas `cuda`/`metal`/`vulkan` escolhendo backend. Também registrei que `.cargo/config.toml` agora existe com só `[env] CUDACXX` (nenhum `rustflags`/linker — portabilidade preservada) e que o bloqueio do caminho headless desapareceu (`install::run` existe em `crates/tupi/src/install.rs:1232`). `docs/index.md` ganhou a seção "Performance" com os três arquivos.

Atenção: a conclusão auto-gerada no topo deste arquivo ("0/16 subtasks ok, T1.1 falhou") está defasada — `tasks.json` marca 6/6 tasks e 16/16 subtasks como `done` e `meta.json` registra `plan_completed`.
