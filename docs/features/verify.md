# Verificação da escrita: comando declarado por glob e âncora viva

Depois de a escrita agnóstica existir (`docs/conventions.md`, "Escrita:
endereço, guarda e verificação"), a pergunta deixa de ser *onde* escrever e
passa a ser *quem julga o que foi escrito*. A resposta tem duas peças, ambas em
`crates/tupi-tools/src/verify.rs`: o comando do verificador do repo declarado por
glob de caminho e a checagem de âncora `caminho:linha` em documento. Nenhuma
delas usa parser do tupi — as duas leem bytes, contam linhas e comparam hash.

## Comando por glob: `configs/verify.ron`

Arquivo opcional. Ausente ou com `rules: []` é **opt-in desligado**: nenhuma
verificação roda e nada muda para quem não declarou nada
(`read_verify`/`load_verify`, `crates/tupi-tools/src/verify.rs:168-186`).

```ron
(
    rules: [
        (glob: "**/*.md", program: "markdownlint", args: ["--quiet"]),
        (glob: "**/*.ts", program: "tsc", args: ["--noEmit"], timeout_secs: 120),
    ],
)
```

- **O comando é dado, não código por linguagem**: acrescentar um ecossistema é
  acrescentar uma regra (`VerifyRule`, `:43-54`); não existe `match` de extensão
  em Rust.
- **Resolução**: a primeira regra que casa vence (`VerifyConfig::resolve`,
  `:91-94`); glob sem `/` casa o nome do arquivo em qualquer profundidade, glob
  com `/` casa o caminho relativo à raiz (`matches`, `:208-228`).
- **Caminho sem regra é no-op explícito**, não erro (`resolve` devolve `None`).
- **Config inválida falha alto e com o caminho**: regra sem comando, glob vazio,
  `timeout_secs` zero e arquivo quebrado têm variante própria em
  `VerifyConfigError` e mensagem que cita o arquivo (`:104-131`); teto padrão de
  tempo é `DEFAULT_TIMEOUT_SECS` (120 s).
- **Falha do comando é recusa**, não aviso: a recusa cita o comando e cola a
  saída (`VerifyReport::refusal`, `crates/tupi-tools/src/verify/run.rs:77-112`);
  **comando ausente só avisa** (`warnings`, `:115-126`), porque derrubar o turno
  por falta de `markdownlint` seria barrar a escrita por ambiente, não por
  conteúdo.
- **Uma execução por comando distinto**, mesmo que vários arquivos escritos
  casem a mesma regra (`run_verification`, `:151-173`) — o verificador do repo é
  caro e o custo é por comando, não por arquivo.
- Nada aqui muda a allowlist do `shell`: o comando vem da configuração do
  projeto, não do modelo.

## Âncora viva em documento: `caminho:linha` (e `@hash`)

A prosa que cita código carrega o único controle objetivo que ela tem: o
endereço (`crates/tupi-tools/src/verify/anchors.rs:1-12`).

- **Formato**: `caminho:linha`, `caminho:12-34` (a âncora é a primeira linha) e
  `caminho:linha@blake3` com prefixo hex opcional (`parse_anchors`, `:149-199`).
  Sem `@hash` a âncora prova que a **linha existe hoje**; com `@hash` prova que o
  **arquivo é o mesmo** (`verify`, `:202-225`).
- **Veredito por âncora**: viva, `arquivo não existe`, `arquivo ilegível`,
  `linha não existe (arquivo tem N linha(s))` ou `arquivo mudou (blake3 citado
  X, atual Y)` (`Reason::describe`, `:38-52`); âncora morta vem com o documento e
  a linha onde foi citada (`DeadAnchor`, `:56-63`).
- **Varredura**: `check_files` recebe a lista de documentos e `check` varre
  `docs/**/*.md` (`:107-142`); o relatório só é limpo quando não há âncora morta
  (`Report::is_clean`, `:73-75`).
- **Não julga estilo**, só se a afirmação ainda corresponde ao arquivo citado: é
  o mesmo invariante da memória ancorada aplicado a documento.

## Status hoje (o que falta para isto valer no turno)

O contrato existe e está testado, mas **nenhuma tool de escrita chama
`run_verification` e nenhum subcomando expõe `anchors::check`**: fora dos testes
do próprio módulo, `crate::verify` não tem chamador. Na prática, hoje:

- a verificação pós-escrita continua sendo um passo explícito via `exec`
  (`cargo fmt --check`, `tsc --noEmit`, `ruff`, `pytest`, `sqlfluff`,
  `markdownlint`), com timeout e cache do `exec`;
- a âncora de documento é convenção de prosa checada à mão, não gate de CI.

Ligar as duas é trabalho de fiação, não de contrato: o ponto de chamada é o
caminho único de escrita (`crates/tupi-tools/src/support/paths.rs:53-101`) e um
subcomando de manutenção despachado no `main` antes de env/provider, como
`telemetry` e `install`.

## Provas

- `crates/tupi-tools/src/verify.rs:231-350` — md e ts resolvem para comandos
  distintos, caminho sem glob é no-op, primeira regra vence, regra sem comando e
  glob vazio recusados, arquivo ausente significa nenhuma verificação.
- `crates/tupi-tools/src/verify/run.rs:233-346` — falha vira recusa com comando e
  saída, comando ausente só avisa, um comando roda uma vez para vários arquivos,
  nada declarado é no-op explícito.
- `crates/tupi-tools/src/verify/anchors.rs:278-372` — `caminho:linha` reconhecido
  e palavra solta não, `@hash` carrega o declarado, e o check nomeia arquivo
  ausente, linha morta e arquivo mudado.
