# 💡 Spark: Tupi Delta — você paga pelo que **mudou**, não pelo que existe

> Sessão 3 · pergunta de origem: "tem algo que podemos fazer para tornar mais
> econômico?" · premissa: custo de *rodar* o agente (tokens), não custo de dev.

> **STATUS: ESTACIONADO — aguardando discussão.** Nada implementado; nenhum
> código deste repo foi alterado por esta proposta. Para retomar, ver
> "Perguntas em aberto" no fim do documento.

## 🌌 The Vision

Hoje o custo de um turno é proporcional ao **estado**: o repo, a memória, o
histórico. Por isso sessão longa em monorepo é caro — e é exatamente por isso
que empresa não põe agente no CI de repositório grande.

E se o custo fosse proporcional ao **delta**? Um turno manda o que mudou no repo
desde o último turno, mais as perguntas em aberto, mais um manifesto de hashes
do que já foi provado. Nada de recolar corpo de arquivo que ninguém tocou. O
provider nunca vê o repo inteiro: vê a *prova de que o repo continua o mesmo*.

Consequência: **uma sessão de 200 turnos num repo de 500k linhas custa o mesmo
que uma de 20 turnos num repo de 5k linhas.** O custo passa a escalar com o
churn do código, não com o tamanho do código nem com o comprimento da conversa.

E o mesmo mecanismo ataca o cold start: sessão nova não relê nada — ela
*verifica* os hashes das notas que já existem. Se bateram, a memória vale; se
não, expirou (ver `.agents/results/spark/expiring-memory.md`). Intrassessão e
entressessão comem o mesmo remédio.

## 🤯 The Paradigm Shift

O Tupi já tem a peça, mas ela está cega. `shrink_old_results`
(`crates/tupi-domain/src/context.rs:284-338`) colapsa resultados antigos de tool
num ponteiro fixo — **por idade** ("os N mais recentes ficam"). A memória do
projeto registrou o preço disso: a poda não filtra por tool, não diz de onde
veio, e `repeat="allow"` incondicional — ou seja, corta token **destruindo
evidência**. Resultado prático: a poda precisa ser conservadora, e o prompt
segue gordo.

Inversão: podar **por prova, não por idade**. Colapsa um resultado quando o hash
da fonte que ele leu não mudou — e o ponteiro passa a ser um recibo com nome da
tool, caminho, hash e handle (`[pendente: read src/x.rs @blake3 a1b2 · corpo
inalterado · peek h_9f3c]`). Aí a poda pode ser *radical*: você não guarda "os
N últimos", guarda os **não-provados**.

Três unidades mudam de dono:

| Hoje | Tupi Delta |
|---|---|
| unidade de contexto = token | unidade de contexto = hash verificado |
| custo ∝ tamanho do repo | custo ∝ churn |
| o modelo "releu" | o modelo "provou" |

E o recibo da sessão 1 deixa de ser só auditoria: **o recibo é a chave do
cache.** O mesmo artefato que prova que o agente não inventou é o que permite
não reenviar. Confiança e economia virem a mesma feature — a única forma de
fazer isso sem virar "economia burra que alucina".

## 🚀 Why Not? (as objeções óbvias, já respondidas)

- **"O modelo precisa ver o corpo para raciocinar."** Precisa do corpo *quando
  é um miss*. Miss passa a ser explícito e registrado: o ponteiro tem handle, e
  `peek` devolve o trecho exato. O `DeliveryLedger` já distingue `Seen` de
  `Changed` (`crates/tupi-tools/src/support/ledger.rs`) — o sinal existe.
- **"Verificar hash custa caro."** É blake3 local, microssegundos, e os tools de
  leitura/escrita já calculam e imprimem o hash. `ParseCache` já guarda digest
  por arquivo (`support/graph/parse_cache.rs`).
- **"Nota velha faz o agente errar."** Nota velha *expira*, e o gate falha. A
  falha é barulhenta, não silenciosa — é o oposto do RAG.
- **"Isso é otimização incremental."** Não, porque muda a curva: sai de
  "economizei 15% do prompt" para "o custo parou de depender do tamanho do repo".
  É a diferença entre comprimir e mudar a função de custo.
- **"Provider não cobra por hash."** Não precisa cobrar: ele cobra pelo que
  você *manda*, e você manda menos. A economia é 100% sua.

## 🔬 The Nano-Test (o mais barato possível)

Zero infra nova. Pegue uma sessão real gravada e responda **uma** pergunta:

> do total de tokens de prompt, quantos são reenvio de bytes que o ledger já
> prova inalterados?

Implementação mínima: `tupi cost --session <id>` que soma `estimate_tokens` das
mensagens `Role::Tool` cujo hash da fonte ainda bate, mais os corpos que o
`DeliveryLedger` marcou `Seen`. Duas horas de código, um número na tela.

Leitura do número:
- **> 40%** → produto: o custo do agente é, na maior parte, memória de curto
  prazo redundante. A tese está confirmada e o pitch se escreve sozinho.
- **10–40%** → é aí que entra o prefix-cache (abaixo), não a poda.
- **~0%** → a poda já resolveu; a economia real está em outro lugar (turnos, não
  tokens). Morre barato, como deve.

## 🥈 Degraus que já existem e são quase de graça

1. **Disciplina de prefixo cacheado.** `Usage::cached_ratio`
   (`crates/tupi-domain/src/usage.rs:37-43`) e `Stats.cached_tokens`
   (`crates/tupi-app/src/telemetry.rs:58-70`) já medem a fatia cacheada — ninguém
   está vendo o número. Prefixo de sistema + memória byte-idêntico entre turnos,
   `cached_ratio` no status bar. Na maioria dos providers, token cacheado custa
   ~10× menos: é dinheiro no chão por falta de disciplina de bytes, não por falta
   de feature.
2. **Cascata com o grafo como oráculo grátis.** Lookup que o grafo responde
   (definição, chamada, import) não precisa chegar ao provider: 0 token. Só a
   novidade escala para o modelo grande. `Tool::describe_call` + queries do grafo
   já dão o sinal de complexidade de graça.
3. **Padrão "assinatura, não corpo".** `search`/`map` já têm modo de assinatura;
   garantir que esse é o default e que o corpo só vem com `detail=full` é a
   política de custo mais barata que existe: não gastar.

## 🎯 Por que isso é 10x e não 10%

Porque o comprador que hoje recusa agente em CI não recusa por qualidade,
recusa por **previsibilidade da conta**. "Custo por mudança de código" é uma
linha de orçamento. "Custo por sessão em repo grande" é um susto. Quem entrega
a primeira define a categoria.

---

## ⏸️ Perguntas em aberto (pauta da próxima conversa)

1. **Escopo de "econômico".** A proposta otimiza custo de *token* (rodar o
   agente). Se o alvo era custo de *desenvolvimento* ou de *infra*, a tese muda
   inteiro e esta proposta deveria ser reaberta, não estendida.
2. **Ordem de ataque.** Fazer o degrau barato primeiro (prefixo cacheado +
   `cached_ratio` visível + `signatures` como default) ou ir direto ao
   `tupi cost --session` para obter o número que justifica todo o resto?
3. **Tolerância a miss.** Podar por prova com que agressividade? Um ponteiro
   errado custa um `peek`; um ponteiro cego custa uma alucinação. Qual é o
   limite aceitável de "o agente não tinha o corpo em contexto"?
4. **Compatibilidade com o prefix cache do provider.** Colapsar mensagens
   antigas quebra o prefixo byte-idêntico por definição. É preciso decidir
   *quando* colapsar (por turno? por marco de trabalho?) para não anular o
   ganho de `cached_tokens`.

## 🔗 Referências

- `crates/tupi-domain/src/context.rs:284-338` — `shrink_old_results` (poda por idade).
- `crates/tupi-domain/src/usage.rs:37-43` — `Usage::cached_ratio`.
- `crates/tupi-app/src/telemetry.rs:58-70` — `Stats.cached_tokens`.
- `crates/tupi-tools/src/support/ledger.rs` — `DeliveryLedger`, `Seen`/`Changed`.
- `crates/tupi-tools/src/support/graph/parse_cache.rs` — digest por arquivo.
- Sessões anteriores: `.agents/results/spark/attestation-receipt.md`,
