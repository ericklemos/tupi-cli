# 💡 Spark: Tupi Attest — o recibo do código que a IA escreveu

> Sessão 1 do Spark. Ideia escolhida entre quatro (as outras três estão no fim).

## 🌌 The Vision

2029. Ninguém abre mais um PR gerado por agente e pergunta "confia?". Todo PR
chega com um **recibo** anexado, e ele é pequeno — alguns KB, não o diff.

O recibo diz, para cada afirmação que o agente fez no corpo do PR:

| Afirmação | Evidência | Veredito |
|---|---|---|
| "`parse_project` valida `models`" | `read project_config.rs:192-246` hash `a3f1…` | ✅ lido, hash atual |
| "nada mais chama `SessionStore::push`" | `impact_check SessionStore::push` → 0 chamadores | ✅ falsificado e sobreviveu |
| "isso não quebra os testes" | `exec test -p tupi` → 142 ok | ✅ executado |
| "a memória do contexto é preservada" | — nenhuma tool — | ⚠️ **PALPITE** |
| "o `ledger` evita reenvio" | `read ledger.rs:104-267` hash `f8a2…` | ✅ lido |
| "o custo cai 30%" | — nenhuma tool — | ⚠️ **PALPITE** |

Duas linhas em vermelho. É isso. O revisor humano não lê 900 linhas de diff
gerado: ele lê as duas linhas onde o agente **não provou nada** e vai direto
lá. O review de código escrito por máquina deixa de ser leitura de diff e
passa a ser **caça ao palpite**.

O Tupi deixa de ser o chat. O chat continua ali, open source, de graça, para
sempre — ele é o *pintor*. O que se vende é a **autenticação da obra**.

O recibo é:
- **append-only e assinado** (merkle por símbolo → por arquivo → por PR),
- **provider-agnóstico** — qualquer agente pode emitir, porque o `tupi-kernel`
  já tem `ProviderRegistry` e não sabe quem está do outro lado,
- **verificável offline** por quem só tem o repo e a chave pública,
- **e local-first**: o exitoso é o build, não o SaaS. On-prem é o default; a
  nuvem é só o espelho público do selo (transparência estilo Certificate
  Transparency).

E existe o selo: um badge no PR, `verified by tupi`, clicável, que abre o
recibo. Em seis meses, "PR sem recibo" começa a parecer "PR sem teste".

## 🤯 The Paradigm Shift

Hoje o projeto se descreve assim (README, primeira linha): *"Chat de terminal
em Rust"*. O mercado tem cinquenta desses. Concorrer ali é brigar por 10%.

O ativo real do Tupi não é o chat. É esta frase do `AGENTS.md`:

> *"Nenhuma afirmação entra no contexto sem um falsificador: se nenhuma tool
> pode dizer 'está errado', aquilo é palpite."*

Isso não é uma regra de economia de tokens. **É uma epistemologia
implementada.** O `DeliveryLedger` (`crates/tupi-tools/src/support/ledger.rs`)
foi escrito para não reenviar o que o modelo já viu — e no caminho criou a
coisa mais cara do mundo: prova de que o modelo **viu de fato**, com hash do
estado exato. É um notário que nasceu como otimizador de cache.

A inversão:

| | Hoje | Tupi Attest |
|---|---|---|
| Unidade de valor | a resposta | o recibo |
| Concorrente | todo CLI de código com IA | ninguém (complementa todos) |
| Comprador | o dev | a plataforma, o time de segurança, o auditor |
| Modelo | assinatura do chat | selo + on-prem + espelho público |
| Métrica de sucesso | tokens economizados | **palpites pegos antes do merge** |

E o mais bonito: **o recibo é subproduto do funcionamento normal**. Nada de
telemetria nova, nada de instrumentar o agente. Ele já registra tudo — só
joga fora.

## 🚀 Why Not?

**"Ninguém paga por isso."** Auditoria de supply chain é um mercado que já
existe e já tem comprador com orçamento: SLSA, in-toto, sigstore, SOC 2. Todos
atestam **como o binário foi construído**. Nenhum atesta **como o código foi
escrito**. A IA acabou de abrir esse buraco, e o buraco é do tamanho do
mercado inteiro.

**"Claude Code / Cursor vão fazer isso amanhã."** Não podem. Quem escreve não
pode ser o próprio notário — é o mesmo motivo pelo qual o banco não audita o
próprio balanço. A neutralidade é o produto inteiro, e ela só é vendável por
quem não tem um modelo para vender. O Tupi é `ProviderRegistry` + tools: ele já
é agnóstico por arquitetura, não por decisão de marketing.

**"O agente mente e o ledger não pega."** Pega, e é exatamente para isso que a
`assert` (`AssertKind::{Exists, Public, UsedOutside}`), o `impact_check` e o
`trace` estão lá. A mentira vira *claim sem evidência* — e claim sem evidência
é **marcável**. O sistema não precisa detectar mentira; só precisa não deixar
palpite passar por fato. Falso negativo é um custo aceitável; falso positivo
"✅" num palpite é o único pecado capital.

**"Assinar tudo custa caro."** O recibo assina hashes, não conteúdo. Símbolo →
arquivo → PR, três níveis. Kilobyte, não megabyte. E o custo de computação é
local; o selo só publica a raiz.

**"Vai gerar ruído e ninguém vai ler."** Gerar ruído é o default de qualquer
relatório. A defesa é brutal: **o recibo só mostra duas categorias — o que foi
provado e o que foi palpite.** Nada de "considerações", nada de resumo. No
máximo dez linhas. Se tiver mais, é bug de design.

## 🔬 The Nano-Test

Uma tarde, sem servidor, sem SaaS, sem provider novo, sem schema novo:

```bash
tupi attest --range origin/main..HEAD --out receipt.md
```

Passos:
1. Reaproveitar o `DeliveryLedger` em modo *replay*: para cada símbolo que o
   agente alegou ter lido, comparar o hash registrado com o hash atual do
   arquivo. Hash igual → ✅. Hash diferente → ☠️ "leu uma versão morta".
2. Cruzar o corpo do PR com a lista de claims e marcar quem não tem tool
   correspondente → `⚠️ PALPITE`.
3. Rodar em **três PRs reais, deste repositório**, gerados por agente. Gerar
   os três recibos à mão se preciso.

Sinal de vida, nesta ordem de força:
1. Aparece **pelo menos um** `⚠️ PALPITE` num PR que foi mergeado sem ninguém
   notar. Um só já paga a ideia.
2. Aparece um `☠️ leu uma versão morta` — o agente citou código que já mudou.
3. **Alguém pede, sem eu pedir, para colocar o recibo no template de PR.** Esse
   é o único sinal que importa. O resto é entusiasmo meu.

Se os três PRs saírem com 100% ✅ e nenhum interesse espontâneo: é porque o
agente é bem-comportado ou porque o revisor humano já é rigoroso. Nos dois
casos, mata a ideia e volta para a fila.

---

## Fila de ideias (não escolhidas nesta rodada)

- **Tupi Context Server** — vender o grafo de chamadas como daemon/API local
  que qualquer agente consome (o "LSP do contexto"). Boa, mas é o mesmo
  negócio, outro preço. *Guarda-chuva da próxima sessão.*
- **Marketplace de workflows** — as personalidades (spark, skeptic, gatekeeper)
  como pacotes distribuíveis. Ótimo para adoção, péssimo para receita: dá para
  copiar em um arquivo markdown.
- **Memória de projeto compartilhada por time** — o "git da memória", onde o
  conhecimento de um repo evolui em PRs. Muito boa; é a **distribuição** do
  Tupi Attest, não uma ideia separada. Revisitar depois que o recibo existir.
