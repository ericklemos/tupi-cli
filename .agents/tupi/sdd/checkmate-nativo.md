<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Checkmate vira workflow nativo, com linguagem simples (`checkmate-nativo`)

**Status:** completed

## Objetivo

Transformar o workflow `checkmate` — hoje só markdown local em `.agents/workflows/checkmate.md` (139 linhas, escopo `local (.agents)`) — num workflow NATIVO embutido no binário, e reescrever o corpo em inglês claro e amigável (o método lógico continua o mesmo; o vocabulário de paper sai), mantendo a persona Checkmate ♟️ / 🦅 Eagle. Escopo: `configs/workflows/checkmate.ron` novo, a lista de nomes fixada em teste, e a remoção do `.md` local que sombreia o nativo. Fora de escopo: qualquer outra mudança no motor de workflows, na TUI ou em outros workflows nativos.

## Restrições

- Workflow nativo nasce e muda SÓ em `configs/workflows/<slug>.ron`, com o texto integral no campo `body`; editar `native_workflows.rs` em `target/` é proibido (docs/rules/workflows.md:1-6).
- `agent:` é obrigatório e só aceita `execution` ou `sdd`; a decisão do usuário é `Some("sdd")` — checkmate revisa e prova, não edita (crates/tupi-tools/src/tools/workflow/mod.rs:780-801).
- `the_shipped_catalog_is_exactly_the_agent_workflows` (crates/tupi-tools/src/tools/workflow/mod.rs:809-832) fixa a lista dos 16 nomes; sem inserir `"checkmate"` na posição alfabética (entre `bughunter` e `disco`) o build falha.
- Idioma do corpo: inglês, como os 16 nativos em `configs/workflows/*.ron` (decisão do usuário).
- Persona mantida: nome + emoji + animal spirit + voz; o que muda é o registro — frases curtas, termos explicados na primeira aparição, zero jargão sem tradução (decisão do usuário).
- `.agents/workflows/checkmate.md` PRECISA sair: `merge` (crates/tupi-tools/src/tools/workflow/mod.rs:70-87) faz a camada local vencer a nativa, então o nativo nunca apareceria em `workflow list`.
- Memória do agente respeita docs/rules/knowledge-placement.md: índice em `.agents/agents/checkmate/memory.md`, journal em `.agents/journals/checkmate.md`, resultados em `.agents/results/checkmate/`; nada de regra de projeto em `.agents/`.
- `description` é truncada em 120 chars (DESCRIPTION_CHARS, crates/tupi-tools/src/tools/workflow/mod.rs:13) e é o que aparece no prompt e no autocomplete: tem de caber e ler bem sozinha, sem markdown.
- Não prometer comportamento que o motor não tem: workflow nativo não recebe parâmetro extra, o escopo do agente vem do front matter (`parse`, crates/tupi-tools/src/tools/workflow/mod.rs:124-161).

## Nuances

- O `.md` local não tem front matter, então a descrição exibida hoje é a primeira frase do corpo cortada em 120 chars — sai literalmente `**Name:** Checkmate ♟️ (O Arquiteto do Xeque-Mate)…`. O `.ron` com `description:` curada resolve isso sem tocar em código.
- Formato nativo já usado por spark/skeptic: tupla RON com `name`, `agent`, `description`, `body: r#"# ..."#` (configs/workflows/spark.ron:1-121, configs/workflows/skeptic.ron:1-124).
- Os nativos em `configs` têm inconsistência de caminho de memória (spark e skeptic dizem `../../.agents/agents/<slug>` na tabela e `.agents/journals/<slug>.md` no texto); o checkmate novo deve usar o caminho canônico da regra, sem repetir o erro.
- O método lógico a preservar, em linguagem simples: nomear as premissas da estrutura atual (P, Q), refutar por redução ao absurdo / contrapositiva / enumeração exaustiva de casos / duas invariantes inconsistentes / premissa falsa nos dados reais, e derivar a nova estrutura como conclusão — com custo da refatoração declarado como premissa.
- Xeque-mate tem UMA conclusão: o workflow não pode entregar duas alternativas sem se comprometer. Isso é regra do corpo, não sugestão de estilo.
- O catálogo é consumido por três lugares — prompt do sistema (`prompt_section`, crates/tupi-tools/src/tools/workflow/mod.rs:245-268), autocomplete da TUI e testes de precedência; nome errado ou `.ron` inválido desaparece dos três em silêncio, por isso o teste de lista fixa existe.
- Ao virar nativo, o marcador de escopo no prompt passa a vir do `agent`: `sdd`, igual a palette, skeptic e spark.

## Tasks

- [x] **T1** Criar `configs/workflows/checkmate.ron` (nativo, corpo em inglês claro)
  - Arquivo novo, espelhando a tupla RON de `configs/workflows/spark.ron:1-121`. `name: "checkmate"`, `agent: Some("sdd")`, `description` sem markdown e com ≤120 chars (DESCRIPTION_CHARS, crates/tupi-tools/src/tools/workflow/mod.rs:13), `body: r#"# ..."#`. Proposta de description: "Reviews one point of the code, shows in plain words why the current shape cannot hold, and derives the new one.". O corpo mantém a persona (Checkmate ♟️, 🦅 Eagle, Personality, Role, Voice) com registro simples: frases curtas, cada termo lógico explicado na primeira aparição, `it follows that` no lugar de `I think`.
  - Nuance: Não editar `target/.../native_workflows.rs`: o build regenera (docs/rules/workflows.md:1-6; crates/tupi-tools/build.rs:5-45).
  - Nuance: Body em RON string raw: nenhuma sequência `"#` no meio do texto, ou a string fecha antes da hora.
  - [x] **T1.1** Cabeçalho RON com name, agent e description — `ron::from_str::<WorkflowRon>` aceita o arquivo; `name` == stem `checkmate`; `agent` == Some("sdd"); `description.len() <= 120` e sem `*`/`#`.
  - [x] **T1.2** Reescrever o método em inglês simples, preservando a lógica — O corpo tem, nesta ordem e em linguagem plana: Objective; Memory (`.agents/agents/checkmate/memory.md`, `.agents/journals/checkmate.md`, `.agents/results/checkmate/`); Boundaries (Always / Ask first / Never); como o checkmate pensa; processo em 6 passos (scope → write the premises → break them → rebuild → verify → present); formato do documento de saída (Alvo, Premissas, Refutação, Nova estrutura, Prova, Custo). Cada uma das cinco formas de prova — absurdo, contrapositiva, enumeração exaustiva, invariantes inconsistentes, premissa falsa nos dados — aparece com uma explicação em uma linha que não exige vocabulário de lógica formal.
  - [x] **T1.3** Alinhar caminhos de memória com docs/rules/knowledge-placement.md — Nenhuma ocorrência de `../../.agents` nem de journal dentro da tabela de memória; `memory.md` é índice compacto, `journal.md` só recebe prova que revelou falha estrutural, e nada de regra de projeto é gravado em `.agents/`.
- [x] **T2** Incluir `checkmate` na lista fixada do catálogo
  - Em `crates/tupi-tools/src/tools/workflow/mod.rs:809-832` (`the_shipped_catalog_is_exactly_the_agent_workflows`), inserir `"checkmate"` entre `"bughunter"` e `"disco"`. Sem isso o teste falha e o catálogo embutido diverge do esperado.
  - [x] **T2.1** Editar o vetor de nomes em ordem alfabética — `cargo test -p tupi-tools the_shipped_catalog_is_exactly_the_agent_workflows` passa; o vetor tem 17 nomes em ordem alfabética.
- [x] **T3** Remover o shadow local `.agents/workflows/checkmate.md`
  - A camada local vence a nativa (`merge`, crates/tupi-tools/src/tools/workflow/mod.rs:70-87); com o `.md` no lugar, `workflow list` continua mostrando `checkmate (local (.agents))` e o nativo nunca aparece. O usuário não respondeu se apaga ou se vira task de execução — a spec assume remoção. Antes de apagar, garantir que o corpo novo do T1 absorveu tudo do `.md` (comparar seção por seção).
  - [x] **T3.1** Conferir que nada do `.md` ficou de fora — Lista de seções do `.md` (persona, Objective, Memory, Boundaries, Philosophy, Journal, Generated results, Process, Favorite plays, Avoids) mapeada 1:1 para o novo `body`, com as diferenças de linguagem anotadas.
  - [x] **T3.2** Apagar o arquivo local — `.agents/workflows/checkmate.md` não existe mais; `workflow list` reporta `checkmate (nativo)` com `agent: sdd`.
- [x] **T4** Provar que o catálogo embutido mudou de verdade
  - Recompilar `tupi-tools` (o `rerun-if-changed` de crates/tupi-tools/build.rs:5-45 força a regeneração) e conferir os três consumidores: prompt do sistema (`prompt_section`, crates/tupi-tools/src/tools/workflow/mod.rs:245-268), autocomplete da TUI e testes.
  - Nuance: Buscar em `docs/` e `README.md` qualquer contagem do catálogo de workflows que passe a mentir com 17 nativos.
  - [x] **T4.1** Rodar os testes de workflow — `cargo test -p tupi-tools tools::workflow` verde, incluindo `every_native_workflow_is_valid_ron_in_configs`, `every_native_workflow_has_a_description_and_a_body` e `the_shipped_catalog_is_exactly_the_agent_workflows`.
  - [x] **T4.2** Conferir `workflow list`/`read` no binário — `workflow list` mostra `checkmate (nativo)`; `workflow read checkmate` devolve o corpo novo, sem menção a `local (.agents)` nem a `*.md`.
