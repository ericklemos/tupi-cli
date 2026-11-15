# Journal — The Spark 💡

Só ideias radicais e pivots. Nada de log de rotina.

## Sem data registrada (sessão 1) - Tupi Attest: o recibo do código escrito por IA

> Nenhuma tool desta sessão expõe o relógio; a data fica em aberto em vez de
> inventada.

**The "Crazy" Idea:** O Tupi não vende o chat: vende o **recibo**. Para cada
afirmação de um PR gerado por agente, um arquivo de KBs mostra o que foi
provado com hash/tool e o que é **palpite**. `DeliveryLedger`
(`crates/tupi-tools/src/support/ledger.rs:104-267`) passa de otimizador de
tokens a notário: mesmo dado, outro mercado.

**The Hidden Potential:** Todo o supply chain de software já tem attestation de
*como foi construído* (SLSA, in-toto, sigstore) e **nenhuma** de *como foi
escrito*. O comprador (segurança, plataforma, auditoria) já tem orçamento e já
exige selo. E o Tupi é provider-agnóstico por arquitetura — quem escreve não
pode ser o próprio notário. Inverte o modelo: de assinatura de assistente para
infraestrutura de confiança, com todo CLI de IA como parceiro, não concorrente.

**First Step:** `tupi attest --range origin/main..HEAD` rodando em 3 PRs reais
deste repo, em modo replay do ledger (hash registrado vs. hash atual). Sinal
único que importa: alguém pedir o recibo no template de PR sem eu pedir.

## Sem data registrada (sessão 2) - Memória que vence: o hash como prazo de validade

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea:** Memória de IA deveria **expirar**. Cada nota de
`.agents/**/memory.md` carrega o `blake3` dos símbolos de que depende; quando o
código muda, a nota não fica silenciosamente obsoleta — ela **vence** e
`tupi memory --check` falha o build. Conhecimento deixa de ser documentação e
vira **teste**. `Evidence` (`crates/tupi-app/src/memory.rs:55-60`) já registra
quais tools rodaram, mas joga fora *o que* foi lido: o hash que a tool já
imprime nunca é persistido.

**The Hidden Potential:** Todo mundo faz RAG que sempre devolve algo — ou seja,
falha em silêncio. Ninguém entrega memória **falsificável**. E o comprador
inverte em relação à sessão 1: atestation de PR vende para segurança; memória
ancorada vende para quem sofre com ramp-up — onboarding de sistema crítico como
ativo licenciável, com o sênior curando o cérebro do repo. Formato aberto
(markdown + hash) para adoção; verificador e histórico como defensabilidade.

**First Step:** ancorar à mão 10 bullets da memória deste repo + script de 20
linhas recomputando os hashes, rodando em `main~20..main`. O número que importa:
quantas notas dos agentes já estão falsas. Acima de ~30% = o produto inteiro; ~0%
= ideia morta barata. Ver `.agents/results/spark/expiring-memory.md`.

## Sem data registrada (sessão 3) - Tupi Delta: pagar pelo que mudou, não pelo que existe

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea:** O custo do agente deve seguir o **churn**, não o tamanho do
repo nem o comprimento da sessão. A poda de contexto existente
(`shrink_old_results`, `crates/tupi-domain/src/context.rs:284-338`) colapsa
resultados **por idade** num ponteiro fixo e por isso precisa ser conservadora —
ela corta token destruindo evidência. Inverter: podar **por prova**. Só colapsa
quando o hash da fonte não mudou, e o ponteiro vira recibo com tool, caminho,
hash e handle. O recibo da sessão 1 deixa de ser auditoria e passa a ser a
**chave de cache**: confiança e economia viram a mesma feature.

**The Hidden Potential:** Muda a função de custo, não a constante. Hoje o
comprador recusa agente em CI de monorepo por imprevisibilidade da conta; "custo
por mudança de código" é linha de orçamento, "custo por sessão em repo grande" é
susto. E o mesmo mecanismo resolve o cold start: sessão nova não relê, verifica
as notas ancoradas (sessão 2) — intrassessão e entressessão com o mesmo remédio.

**First Step:** `tupi cost --session <id>`: somar `estimate_tokens` dos
resultados de tool cujo hash de fonte ainda bate + corpos marcados `Seen` pelo
`DeliveryLedger`. Um número só: quanto do prompt é reenvio de bytes já provados.
`>40%` = produto; `~0%` = a economia está em turnos, não em tokens. Detalhes em
`.agents/results/spark/delta-cost.md`.

## Sem data registrada (sessão 4) - A Escada de Inteligência: comprar QI por veredito

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea:** O Tupi não escolhe *um* cérebro para a sessão — ele desce e
sobe uma **escada**. Um driver barato (ou um 7B local do `tupi-infer`) segura a
sessão; os modelos mais caros do mundo existem como **tool** (`escalate`), chamados
só onde a resposta é falsificável. `App::provider()`
(`crates/tupi-app/src/app.rs:645-653`) devolve um provider ativo enquanto o
`ProviderRegistry` (`crates/tupi-kernel/src/registry.rs:11-32`) guarda todos: a
feature inteira mora na distância entre essas duas linhas. E `judge` fecha o
circuito: N candidatos, `assert`/`exec`/`impact` matam os errados — inteligência
comprada com prova de qual sobreviveu.

**The Hidden Potential:** Todo mundo roteia modelo por preço; ninguém roteia por
**veredito**, porque ninguém tem falsificadores. O Tupi tem — e tem o ledger. Isso
transforma custo de IA de "conta assustadora por sessão em monorepo" em **linha de
orçamento por chamada cirúrgica**, comprada por plataforma/FinOps, não por dev. A
métrica nova é *bugs pegos por dólar*, e ela não existe em SWE-bench nenhum.

**First Step:** vinte perguntas verificáveis sobre este repo, rodadas no modelo
barato e no caro; medir divergência com o caro certo. ≥20% = prêmio na escada;
~0% = ideia morta a custo zero. Ver `.agents/results/spark/intelligence-ladder.md`.

## Sem data registrada (sessão 5) - Tupi Space: o repo com córtex, não com chat

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea:** O repo baixa, verifica e carrega um
`LFM2.5-Embedding-350M` e um `LFM3-1.7B`
(`crates/tupi-infer/src/download.rs:34-62`) — e o único consumidor é a tool
`docs`, que embeda **a pasta de documentação** para busca híbrida
(`docs/search.rs:17-162`) e resume cada arquivo numa linha
(`docs/index.rs:516-591`). Enquanto isso a tool `search`, porta de entrada de
toda sessão, é lexical pura (`search/query.rs`, `LexicalRanker`). Inverter: o
espaço vetorial é **do repo**, é cache incremental por hash, e é **grátis** —
`search`, ranking do `map`, retrieval da memória e roteamento de agente
consultam o córtex local antes de gastar um token.

**The Hidden Potential:** Duas coisas que só existem quando a descoberta é local:
(1) **custo de descoberta zero** — o modelo caro passa a receber o problema já
reduzido, o que torna a Escada da sessão 4 economicamente óbvia; (2) **venda para
quem não pode mandar código a terceiro** — banco, governo, defesa, saúde compram
"binário + GGUF com sha256", não assinatura de SaaS. E o espaço pode ser servido
por MCP: todo CLI de IA vira cliente da camada de significado, não concorrente.

**First Step:** `tupi space build` sobre 2.244 símbolos + docs + bullets de
memória, depois 20 perguntas em português **sem identificador**, medindo recall@5
semântico vs. lexical, com a rede desligada. Sinais: semântico ganha em ≥40% →
paga; ≥3 gêmeos semânticos reais (`space twins`) → produto de review;
empate → morte barata em CPU, zero API. Ver
`.agents/results/spark/repo-vector-space.md`.

## Sem data registrada (sessão 5, anexo) - Lint semântico: o editor que conhece a intenção

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea:** A pergunta era "isso vale para uma RTX 5090?". A resposta
honesta é que hoje o Tupi **não usa a GPU** (`llama-cpp-2` sem feature de
aceleração em `crates/tupi-infer/Cargo.toml`, modelo carregado com
`LlamaModelParams::default()` em `crates/tupi-infer/src/llama.rs:28`). Mas com
GPU, `space twins` deixa de ser relatório e vira **rabisco no editor**: pairwise
total de 2.244² ≈ 2,5M cossenos de 1024 dims ≈ 5 GFLOP, ou seja **milissegundos**.
Aí o produto não é "um agente que busca", é um **compilador de intenção**: o
editor te avisa em tempo real que aquelas 40 linhas já existem três arquivos
adiante com outro nome.

**The Hidden Potential:** Todo mundo já viu bug nascer de copy-paste esquecido —
e nenhum linter pega, porque exige olhar o repo **inteiro** de graça e por
sentido, não por sintaxe. É a ideia desta série que mais ganha com hardware: em
CPU viraria batch noturno e morreria de tédio; em GPU é feedback de digitação.
Troca a tese de "cache por hash" por "indexação como efeito colateral" — o hash
sobrevive, mas muda de papel: de economia para invalidação.

**First Step:** as duas linhas que destravam tudo — feature `cuda` + `n_gpu_layers`.
Depois refazer `space build` e olhar o relógio: **repo inteiro em < 5 s** = o item
de lint contínuo vira o produto principal. Ver item 8.3 de
`.agents/results/spark/idea-backlog.md`.

## Sem data registrada (sessão 5, correção) - O espaço é fosso, não cano

> Sem relógio exposto pelas tools desta sessão; data em aberto de novo.

**The "Crazy" Idea (que morreu):** publicar o espaço vetorial por MCP, para que
Cursor e Claude consultem o significado do repo sem receber o código. O usuário
matou: "não quero dar nada para o Cursor ou o Claude". E ele está certo — o plano
da Attest de virar cano vale para *recibo* (é rede: ganha valor com volume), não
para *conhecimento*. Servir o córtex a um CLI concorrente é treinar o cliente a
não precisar de nós.

**The Hidden Potential (da recusa):** a restrição **melhora** a tese. O espaço é
