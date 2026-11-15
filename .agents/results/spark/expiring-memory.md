# 💡 Spark: Toda memória de IA deveria expirar — e o `blake3` pode ser o prazo de validade

> Sessão 2 do The Spark. Sessão 1 (Tupi Attest) já foi para o backlog executável;
> esta ideia não é a mesma coisa com outro nome: Attest audita o que um PR
> *afirma*, isto trata do que um repositório *sabe* — e de quando esse saber
> deixa de ser verdade.

## 🌌 A Visão

Imagine um repo onde o conhecimento não mora em README desatualizado, nem em
cabeça de gente que saiu, nem em embedding que sempre devolve *alguma coisa*.

Mora num arquivo versionado — e cada linha sabe do que ela depende:

```md
## Por que o ledger não deduplica por conteúdo antes do clamp
âncora: crates/tupi-tools/src/support/ledger.rs#dedupe @ blake3:9f2c…
âncora: crates/tupi-app/src/pipeline.rs#record_finished @ blake3:41ab…
```

Quando o código muda, a linha não vira lixo silencioso: ela **vence**.
`tupi memory --check` devolve exit code 1 e a lista do que ficou falso. Um
novato entra no projeto e não lê o passado — ele **clona o cérebro do repo**
(`tupi memory --clone owner/repo`), com recibos anexados de por que cada nota
existe. Um agente de qualquer modelo lê a mesma memória, porque é markdown, não
é feature de fornecedor.

E o passo que faz isso virar produto: as pessoas passam a **escrever** memória
para o repositório — não para si mesmas. Sênior curando o cérebro de um sistema
crítico vira um ativo licenciável. Onboarding de um monolito legado não é
semanas de leitura: é um pacote de memória com procedência.

## 🤯 O Paradigma Shift

| Hoje (RAG, memória de assistente) | O que proponho |
|---|---|
| Sempre devolve algo; falha em silêncio | Devolve **ou declara vencida**; falha alto, em CI |
| Memória é texto | Memória é texto **com hash de dependência** |
| Memória é por sessão / por fornecedor | Memória é por **repositório**, portátil entre modelos |
| Conhecimento é documentação | Conhecimento é **teste** |
| `memory.md` cresce e ninguém sabe o que ainda vale | `memory.md` tem TTL **estrutural**, não temporal |

A frase que resume a aposta: **memória de IA sem procedência não é memória, é
boato bem formatado.** E ninguém no mercado aceita a consequência lógica — se
você não sabe de onde veio, você não sabe se ainda vale.

## 🔬 O Estado Atual do Código (por que a distância é curta)

- `crates/tupi-app/src/memory.rs:55-60` — `Evidence` já existe, com `tools` e
  `injected`, mas guarda **texto**, não identidade do que foi lido.
- `crates/tupi-app/src/memory.rs:63-90` — `sheet()` monta o material do
  sumarizador e corta em 2400 chars: é aqui que os hashes morreriam por design.
- `crates/tupi-app/src/memory.rs:26-31` — `MemoryBlock { text, facts, tokens, … }`
  não tem campo de âncora. Um campo (`anchors: Vec<Anchor>`) é a mudança
  inteira na camada de dados.
- `crates/tupi-web/src/inspect_api.rs:19-34` — a memória já é servida por HTTP
  (somente leitura): o "cérebro do repo" já tem uma vitrine pronta.
- O harness **já imprime `blake3`** em cada leitura de arquivo/símbolo. O
  primitivo de âncora não precisa ser inventado; só precisa ser *persistido*.
- Prova viva do problema: `.agents/agents/spark/memory.md` cita
  `crates/tupi-app/src/memory.rs` **sem hash** — se esse arquivo mudar amanhã,
  a nota continua "verdadeira" para sempre.

## 🚀 Por que não? (objeções antecipadas)

- **"Âncora em hash quebra a cada commit, vira ruído."** Não quebra: vence. A
  granularidade é símbolo, não arquivo; e vencida ≠ apagada — ela fica visível
  como dívida, que é exatamente o que o time precisa ver.
- **"Memória é só documentação com passos extras."** Documentação não tem
  verificador. Aqui o verificador roda no CI e no `pre-commit` — a diferença é
  a mesma entre comentário e `assert`.
- **"Quem mantém?"** A mesma pessoa que hoje revisa PR: o diff de memória
  aparece junto do diff de código. Custar 0 cliques é o requisito de design.
- **"Marketplace de prompt/workflow já foi descartado (sessão 1)."** Correto —
  e continua descartado. Aqui não se vende personalidade (copiável em markdown);
  vende-se **validade verificada**, que exige o verificador rodando.
- **"Modelos vão resolver isso sozinhos."** Modelo não tem memória durável nem
  interesse em declarar o próprio erro; quem escreve não pode ser o notário.

## 🧪 O Nano-Test (o mais barato possível)

Não escrever código de produto. Só medir a mentira:

1. Pegar 10 bullets da memória deste repositório e anexar o hash do símbolo que
   cada um cita (cópia manual, ~20 min).
2. Um script de 20 linhas que recomputa os hashes e lista o que venceu.
3. Rodar contra `main~20..main` — janela em que este repo mexeu em memória e em
   código ao mesmo tempo.

**Sinal único que importa:** quantas notas dos agentes deste repo já estão
falsas sem ninguém ter percebido. Qualquer número acima de ~30% é o
posicionamento inteiro do produto, e sai como conteúdo ("as memórias dos nossos
agentes estavam N% erradas — e as suas também") antes de existir feature.

Sinal de fracasso, para não me enganar: se o número for ~0%, o problema não
existe e a ideia morre barata — que é o objetivo do nano-teste.

## 💰 O Modelo (esboço, não compromisso)

- **Free:** `memory.md` local com âncoras e `--check` no pre-commit.
- **Pago por time:** `--check` no CI + mesh entre repos (memória de serviço A
  que explica contrato de serviço B) + histórico de "quando isto virou mentira".
- **Pago por ativo:** memória curada de sistemas críticos, licenciada por
  assento de onboarding. O comprador não é o dev, é quem sofre com o ramp-up.
- Efeito de rede honesto: quanto mais repos com memória ancorada, mais valioso
  o clone; mas o formato é aberto, então a defensabilidade é o verificador e o
  histórico, não o arquivo.

## 🦋 Ideias laterais que ficam no bolso (não são esta sessão)

- **Memória como gate de review:** PR que muda símbolo ancorado abre issue
  automática "esta nota ficou vencida?"
- **`tupi memory --blame`:** qual nota mentiu, desde quando, e qual PR a venceu.
- **Benchmark público de staleness por ecossistema** (usa `cached_ratio` da
  telemetria como dado de apoio) — conteúdo de mercado, não produto.
