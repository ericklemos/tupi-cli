# Documentação do projeto

## Filosofia

- [Filosofia](philosophy.md) — local para o que é mecânico, API só onde há
  raciocínio e nenhuma liberdade ao modelo.

## Arquitetura

- [Arquitetura](architecture.md) — camadas, fronteiras e componentes do workspace.

## Ferramentas

- [Tool `shell`](features/shell-tool.md) — saída bruta por padrão (`terms` só
  filtra com `filter: true`), allowlist restrita (`cargo` fora, `git` só leitura),
  binário ausente vira aviso com equivalente e recusa sempre ensina a alternativa;
  sem memória do projeto dentro do resultado.

## Modelos locais

- [Modelo de decisão `d1`](features/d1-decider.md) — artefato canônico
  (Q4_K_M, 1.099.629.888 bytes, SHA-256 e destino), download sob demanda por
  `ensure_decider_model` e custo de disco/rede; nada o consome ainda.

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

## Decisões

- [ADR 0001](decisions/0001-cache-do-exec.md) — o cache de `exec` fica, com a
  chave restrita a insumos de build.
- [ADR 0002](decisions/0002-shell-poder-e-ruido.md) — o `shell` reduz poder e
  ruído: filtro opt-in, allowlist restrita, recusa acionável.

## Regras

- [Documentação em `docs_folders`](rules/documentation.md)
- [Estado de agente vs. conhecimento de projeto](rules/knowledge-placement.md)
- [Nenhuma instrução ao modelo sem tool ou limite](rules/model-control.md)
- [Workflow nativo em `configs/workflows/*.ron`](rules/workflows.md)
