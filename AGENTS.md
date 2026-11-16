# AGENTS.md — regras duras do repositório

Este arquivo entra inteiro no prompt e é truncado em 32 KiB (`MAX_AGENTS_BYTES`,
`crates/tupi/src/agents_config.rs:3`). O bloco entre marcadores é gerado pela
tool `setup` (`merge`, `crates/tupi-tools/src/tools/setup/mod.rs:73`), que
reescreve só esse trecho: regra autoral vive fora dos marcadores, e aqui,
nunca dentro do bloco gerado. Este cabeçalho ocupa no máximo 1/5 do teto
(6,5 KiB) — texto novo entra apagando texto velho.

## 1. Descoberta vem antes da resposta

O prompt não carrega o repositório. Comece em `map(depth="dirs")`, afunile com
`map(depth="files", path=…)` e só então abra `read`/`search`. Caminho ou
símbolo que o `map`/`search` não mostrou não existe: não presuma `src/`,
`tests/` nem nomes de função. Convenção e decisão do projeto moram em `docs/`
(`docs_folders` em `ai.project.json:5`) e são lidas pela tool `docs`
(`docs(action="map")`, reindexada com `docs(action="refresh")`); estado de
workflow fica em `.agents/`.

## 2. A tool vence a memória

Não afirme conteúdo que não foi lido nesta sessão. `read` imprime o blake3 do
arquivo; hash divergente em `edit` significa que o disco mudou — releia, não
force. Não releia o que já veio nesta sessão: use o corpo no contexto ou
`peek`. Quando a busca não achar, diga que não achou, em vez de deduzir.

## 3. Edição cirúrgica e com hash

`edit` troca só o corpo `{ … }` de um símbolo e exige `expected_hash`.
`write`/`write_many` são para arquivo novo ou substituição integral
(hash/`overwrite` obrigatórios). Nunca reescreva o arquivo inteiro para mudar
um trecho e nunca edite o que pertence a outra subtarefa: o que está no disco
é verdade, não sugestão. Caminho que escapa do root é recusado
(`paths::resolve_for_write`, `crates/tupi-tools/src/support/paths.rs:53`) —
não tente contornar por `shell`.

## 4. Convergência

Com o suficiente para responder, responda; não explore por reflexo nem repita
o que já foi entregue. Saída no idioma do usuário, direta, citando
`caminho:linha`. `/sdd <slug>` apenas retoma um plano: não execute enquanto o
usuário não pedir (“continue”).

## 5. Custo: local primeiro, API só para raciocínio

O que é mecânico roda local — embedder de `docs`, gerador de resumos e
`transcribe` (`local_embedding_enabled`/`docs_embedding_enabled`/
`summaries_enabled`, `crates/tupi/src/config.rs:311`; slots em
`crates/tupi/src/local_models.rs:14`). A API fica no turno principal e no
executor do SDD (`provider`/`model`, `crates/tupi/src/main.rs:22`); não existe
roteador automático local↔API, então não prometa um. No turno, gaste menos:
`map` antes de `read`, `search` antes de varrer, `read_many`/`test_many` para
lote, `peek` em vez de repetir leitura. Os tetos são números em `configs/`:
`max_tool_steps`/`expose` (`crates/tupi/src/tool_config.rs:16`), `max_tokens`
e o contexto com `budget`/`target`/`recent_results`
(`crates/tupi/src/config.rs:171`). Para gastar menos, baixe o número.
Instrução que nenhuma tool recusa e nenhum número limita é pedido, não regra
(`docs/rules/model-control.md`).
