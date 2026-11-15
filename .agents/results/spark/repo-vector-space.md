# 💡 Spark: O Espaço Vetorial do Repo (o córtex local que só está legendo docs)

Sessão 5 · pergunta do usuário: *"temos alguns modelos de Liquid AI no repo; que
oportunidades existem? No que mais poderíamos usar?"*

## 1. O retrato frio (o que está no repo hoje)

| Peça | Onde vive | Para que serve **hoje** |
|---|---|---|
| `LiquidAI/LFM2.5-Embedding-350M-Q8_0` (1024 dims) | `crates/tupi-infer/src/download.rs:34-47`, `embeds em llama.rs:83-116` | metade *dense* da tool `docs` |
| `LiquidAI/LFM3-1.7B-Q4_K_M` | `download.rs:49-62`, `llama_generate.rs:96-104` | resumo de **uma linha** por documento |
| dois slots globais | `crates/tupi-kernel/src/embed.rs:35-59`, `generate.rs:23-47` | dois órgãos sensoriais do binário |
| `DocsTool` | `crates/tupi-tools/src/tools/docs/mod.rs:36-65` | **único** consumidor; escopo = `docs_folders` do `ai.project.json` |
| híbrido lexical + dense + RRF | `crates/tupi-tools/src/tools/docs/search.rs:17-162` | busca *semântica* — só dentro da pasta de docs |

Diagnóstico desconfortável: **o binário baixa, verifica (sha256) e carrega um
sistema nervoso completo para legendar arquivos de documentação.** Enquanto isso:

- a tool `search` — a porta de entrada de toda sessão — é **puramente lexical**
  (`crates/tupi-tools/src/tools/search/query.rs`, `LexicalRanker`), cega a
  sinônimo, a "isto aqui é a mesma ideia com outro nome";
- o `map`, que ordena símbolos por centralidade, não sabe nada de *intenção*;
- a memória (`.agents/**/memory.md`) é despejada inteira no contexto: é caro
  lembrar, porque não há como *procurar* no que se lembra;
- 2.244 símbolos em 150 arquivos, zero vetores.

Ou seja: temos um 350M e um 1.7B rodando **offline, de graça, sem mandar uma
linha de código para ninguém** — e o produto inteiro continua adivinhando por
substring.

## 2. 🌌 The Vision — `tupi space`: o repo com córtex

`tupi space build` embeda **o repo** (símbolos, docs, notas de memória, journals)
com o LFM2.5-350M e grava um índice incremental em `.tupi/space/`, indexado por
`blake3` do conteúdo — igual ao índice de docs já faz (`docs/store.rs`,
`index_dir`/`index_path`), só que a pasta não é `docs/`: é tudo.

A partir daí, **toda descoberta custa zero token**:

- `search` passa a fazer o que o `docs` já faz há meses: lexical + dense + RRF.
  O caso "eu não sei o nome do símbolo" — o mais comum de todos — deixa de custar
  cinco tentativas pagas e vira uma consulta de 20 ms em CPU.
- `map` e `map(depth="files")` reordenam por *relevância à pergunta*, não só por
  grafo de chamadas. O agente lê 5 arquivos certos em vez de 15.
- a memória ganha retrieval: em vez de injetar todos os bullets, o slot escolhe
  os ancorados que falam do assunto. Casa com a memória-que-vence da sessão 2 —
  o hash continua sendo o juiz; o embedding só decide **quem é candidato**.
- `docs` deixa de ser uma tool e vira um modo do espaço.

E a consequência de negócio, que é onde isso fica interessante:

> **O código nunca sai da máquina.** Um agente que descobre, indexa, resume e
> classifica localmente pode ser vendido para banco, governo, defesa, saúde —
> mercados onde Cursor/Claude Code são *proibidos por política*, não por preço.

Não é "modo offline": é **a única arquitetura em que a descoberta é grátis e
auditável**. O modelo remoto caro continua existindo — mas só é chamado depois
que o córtex local já reduziu o problema ao osso.

## 3. 🤯 The Paradigm Shift

| Assumimos | Viraria |
|---|---|
| embeddings são "para RAG de documentação" | embeddings são a **memória espacial do repo** |
| modelo remoto é o cérebro, modelo local é enfeite | local é a **periferia sensorial** (ver, lembrar, ordenar); remoto é o córtex pré-frontal caro, chamado a granel |
| custo de IA = tokens de prompt | custo de IA = **tokens que sobraram depois do filtro local** |
| contratar agente exige mandar código para terceiro | contratar agente pode ser **um binário + 500 MB de GGUF verificados por sha256** |
| o produto é o agente | o produto é o **espaço** — e ele fica dentro de casa |

Esta ideia faz o produto parecer pequeno de propósito: hoje o Tupi é um app que
conversa; com o espaço, ele vira a **camada de significado do repositório** — e
o cérebro que nenhum CLI concorrente consegue alugar.

## 4. 🚀 Why Not? (objeções, derrubadas)

**"350M não entende código."** Não precisa *entender*: precisa desempatar entre
20 candidatos que o lexical já trouxe. O `fuse` por RRF (`docs/search.rs:136-162`)
foi escrito exatamente para não depender de calibração de score entre fontes
fracas. Modelo pequeno + fusão é o padrão certo.

**"Vetores no git são lixo."** Não vão para o git. O índice é cache local,
reconstruído incrementalmente por hash — o mesmo desenho do índice de docs. O que
se versiona é a config (`docs_folders`-like) e o sha256 do modelo, que o
`ensure_model` já valida antes de carregar (`download.rs:64-113`).

**"1.7B é burro demais para agente."** Ninguém pediu para ele escrever código.
Ele faz três coisas e faz barato: resumir, classificar, reordenar. Cada uma
substitui uma ida ao provider.

**"CPU vai travar a máquina."** 350M Q8 em 1024 dims é dezenas de ms por chunk;
roda em background, como o refresh de docs já faz (`DocsIndex::refresh`,
`index.rs:78`). E os testes reais que já existem (`crates/tupi/src/config.rs:119-132`)
mostram o load em segundos.

**"Já temos `search` lexical e ele é bom."** Bom para quem sabe o nome. O custo
da falha lexical não é zero: é *pago em tokens do modelo caro*, em cada tentativa
seguinte.

**"Isso não é só incrementalismo?"** Não, porque muda a função de custo da
descoberta (de linear-em-tentativas para log-em-consultas) e abre um mercado que
hoje é fechado por contrato, não por feature.

## 5. 🔬 The Nano-Test (o mais barato possível)

Três comandos, uma tarde, nenhuma rede:

1. **`tupi space build`** — embedar 2.244 símbolos + docs + os bullets de memória
   com o 350M já baixado. Sem tocar em provider.
2. **20 perguntas em português sem identificador** ("onde a sessão lembra o que
   foi lido?", "o que decide se um arquivo é relido?", "onde o custo vira
   orçamento?"). Rodar `search` lexical **vs.** `space query`, medir recall@5.
3. **`space twins`** — top-20 pares de símbolos com cosseno mais alto no repo.

**Sinais que importam:**
- semântico ganha do lexical em **≥ 40%** das 20 perguntas → o córtex paga;
- **≥ 3** pares de gêmeos semânticos que sejam *de verdade* duplicata esquecida
  (não `impl`/`tests`) → existe um produto de review ali;
- **0** requisições de rede no teste inteiro → o SKU air-gapped está vivo;
- se o semântico só empatar com o lexical, a ideia morre a custo ~zero de API
  (só CPU) — que é exatamente o tipo de morte barata que vale a pena comprar.

## 6. Cemitério de boas ideias (laterais que não foram escolhidas, mas estão vivas)

1. **`tupi twins` — duplicata semântica como produto.** "O bug que é um
   copy-paste esquecido" é classe de defeito que nenhum linter pega e nenhum
   agente remoto procura, porque exige olhar o repo *inteiro* e de graça.
2. **Drift semântico doc↔código.** Hash detecta mudança de *forma*; embedding
   detecta mudança de *sentido* (mesma assinatura, corpo reescrito, doc agora
   mente). É o complemento exato da memória-que-vence da sessão 2.
3. **Roteador local de personalidade/workflow.** KNN do prompt contra os
   journals: "qual agente já resolveu algo parecido?" — grátis, offline, e
   transforma `.agents/journals/` de diário em base de decisão.
4. **Juiz local de primeira linha.** O 1.7B responde primeiro; só escala para o
   modelo caro onde `assert`/`exec`/`impact` discordarem do local. Isso não é a
   Escada da sessão 4 — é o **gate de gratuidade** dela.
5. **LoRA do LFM3 no repo ("o sotaque da casa").** O dataset já está no git
   (histórico + reviews + memória). Um mês, não uma tarde — mas o resultado é um
   modelo que escreve *como este repo escreve* e passa o CI deste repo.
6. ~~**Publicar o espaço por MCP.**~~ **MATADA pelo usuário:** não se entrega
   cérebro a Cursor/Claude. O espaço é fosso, não cano — ver §9.

## 7. Veredito do Spark

O ativo mais subaproveitado do repo não é código — é **um modelo de 350M
carregado para uma tarefa só**. A pergunta certa não é "o que mais o Liquid AI
pode fazer?", é: *por que a descoberta neste produto ainda é lexical?*

## 8. Anexo — e com uma RTX 5090?

Pergunta do usuário depois da sessão. Vale mais que um "sim": ela **muda o
alvo**, não só a velocidade.

**Fato do repo primeiro:** hoje o Tupi **não usa a GPU**. `llama-cpp-2` entra sem
feature de aceleração (`crates/tupi-infer/Cargo.toml`) e o modelo carrega com
`LlamaModelParams::default()` (`crates/tupi-infer/src/llama.rs:28`) — nenhum
`n_gpu_layers`, nenhum backend CUDA/Vulkan em parte alguma do workspace. Uma 5090
hoje roda tudo em CPU. O primeiro ato é flag de build, não arquitetura.

**O que 32 GB / ~1,8 TB/s realmente compram, em ordem de importância:**

1. **Indexar deixa de ser comando e vira efeito colateral.** O repo inteiro em
   segundos → indexa no save (`notify` já é dependência). O hash sobrevive, mas
   muda de papel: de *economia* para *invalidação*.
2. **Pairwise total = lint semântico em tempo real.** 2.244² pares ≈ 2,5M
   cossenos de 1024 dims ≈ 5 GFLOP: milissegundos. "Duplicata esquecida" para de
   ser relatório e vira rabisco no editor — compilador de intenção.
3. **O degrau do meio da Escada (sessão 4) fica local.** Um 14–32B Q4 cabe; o
   cloud passa a ser só o topo, não o miolo.
4. **Rerank cross-encoder passa a caber no orçamento.** O `llama_colbert.rs` que
   não existe hoje passa a ser justificável: top-50 → top-8 por consulta.
5. **Speculative decoding de graça:** o 350M como draft do 14B (decode é
   bandwidth-bound).
6. **LoRA "sotaque da casa"** vira uma tarde de treino, não um mês — o gargalo
   passa a ser o dataset, que já está no git.

**O que ela não compra:** 32B local não é frontier, 32 GB com contexto longo
ainda estoura, e CI sem GPU precisa de fallback CPU determinístico — senão os
testes reais que já existem (`crates/tupi/src/config.rs:119-132`) param de rodar.
O nano-teste não muda: 20 perguntas, recall@5, semântico vs. lexical.

**Bônus de negócio:** a 5090 é a prova física de que o SKU air-gapped é
vendável — "roda na sua workstation, o código não sai". Quem tem 5090 é
exatamente o comprador que hoje não pode usar SaaS.

## 9. Correção de direção — o espaço é fosso, não cano

> Decisão do usuário: "não quero dar nada para o Cursor ou o Claude". A ideia de
> `space serve` por MCP cai — e cai por princípio, não por custo.

**Por que a decisão é melhor que a ideia original (e eu deveria ter visto):** o
plano da Attest era virar cano porque *recibo* ganha valor quanto mais gente
passa por ele — é rede. **Conhecimento não é rede.** Um espaço vetorial servido a
um CLI concorrente entrega exatamente a vantagem que faria alguém trocar de
ferramenta: "ele entende o meu repo". Servir por MCP seria treinar o cliente a
não precisar de nós — o mesmo erro de quem entrega o único diferencial e cobra o
instalador.

**A inversão:** o espaço é a razão de **não** trocar. O Tupi é o único CLI cuja
busca **entende** a intenção porque o córtex está aqui dentro — na TUI, nas tools
que já existem, no mesmo binário. Nada de porta, nada de protocolo de terceiro,
nada de exportar embeddings.

**O que continua valendo:** o SKU air-gapped (5.8) fica **mais forte**, não
menos. Vender "binário + GGUF, offline e auditável" para quem não pode mandar
código a terceiro é o oposto exato de servir o espaço a um fornecedor externo.
Consistência de tese: o produto inteiro é "o código e o conhecimento dele não
saem da máquina".

**Consequência prática:** toda ideia futura que dependa de expor o espaço (API,
plugin, protocolo, "integração com X") entra já como 🔴 no backlog, salvo decisão
explícita em contrário do usuário.
