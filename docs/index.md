# Documentação do projeto

## Arquitetura

- [Arquitetura](architecture.md) — camadas, fronteiras e componentes do workspace.
- [Baseline de performance](perf/baseline.md) — números de build e do grafo, com as
  lacunas de medição.

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
