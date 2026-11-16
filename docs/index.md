# Documentação do projeto

## Arquitetura

- [Arquitetura](architecture.md) — camadas, fronteiras e componentes do workspace.

## Interface (TUI)

- [Uso das ferramentas na TUI](features/tui-tool-usage.md) — linha viva por tool
  (spinner, duração, glifo por categoria), agrupamento por turno e sub-chamadas de
  `pipeline` indentadas.

## Performance

- [Baseline de performance](perf/baseline.md) — números de build e do grafo,
  ajustes aplicados e lacunas de medição.
- [Baseline de build (T1.1)](perf/build-baseline.md) — tempos e tamanhos a frio
  antes de qualquer `[profile.*]`, com o script de reprodução.
- [Alocador e linker: medidos e descartados](perf/allocator-and-linker.md) —
  `mimalloc` e `mold` não trouxeram ganho; o que ficou como harness.

## Comandos

- [`tupi install`](features/tupi-install.md) — contrato interno do subcomando que
  declara o diretório do binário no `PATH` (shell detectado, bloco marcado,
  idempotência, idioma da saída).

## Regras de negócio

- [Regras de negócio](business.md) — entidades, invariantes e fluxos.

## Convenções

- [Convenções](conventions.md) — onde cada coisa mora, testes e commits.

## Regras

- [Documentação em `docs_folders`](rules/documentation.md)
- [Estado de agente vs. conhecimento de projeto](rules/knowledge-placement.md)
- [Workflow nativo em `configs/workflows/*.ron`](rules/workflows.md)
