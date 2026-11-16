# Modelo de decisão `d1` — artefato e custo

O `d1` (LiquidAI `d1-2B`) é o modelo local de decisão: responde em **um único
forward pass** (`noul`, `choice`, `score`), sem gerar tokens. Por isso ele não
é `Generator` nem `Embedder` e não é consumido nesta entrega — nenhuma tool,
prompt ou fluxo de documentação muda de comportamento. O que existe é a
metadata verificada e o download sob demanda em `tupi-infer`
(`crates/tupi-infer/src/lib.rs:53-56`, `download.rs:65-70`).

## Artefato canônico

| Campo | Valor |
|---|---|
| Repositório | `LiquidAI/d1-2B-GGUF` |
| Arquivo | `d1-2B-Q4_K_M.gguf` |
| Quantização | Q4_K_M |
| Tamanho | 1.099.629.888 bytes (~1,1 GB; ~1,02 GiB) |
| SHA-256 | `adca5789ad30ae6027e8e936749bb9f19673097492ab26fa1c5b69643ca50b4b` |
| Destino local | `~/.tupi/models/LiquidAI/d1-2B-GGUF/d1-2B-Q4_K_M.gguf` |

O destino é `models_dir(base, repo)` (`crates/tupi-infer/src/cache.rs:3-5`):
`base/models/<repo>/<arquivo>`, com `base = ~/.tupi` — o mesmo layout dos
outros modelos locais.

## Download sob demanda

O GGUF **nunca** é baixado no startup. Ele só aparece quando alguém chama
`ensure_decider_model(base, fetcher)`:

1. se o arquivo já existe e o SHA-256 confere, o caminho é devolvido sem rede;
2. senão, o download vai para `<arquivo>.part`
   (`https://huggingface.co/<repo>/resolve/main/<arquivo>`);
3. o tamanho exato é conferido (falha → `InferError::Size`, `.part` removido);
4. o SHA-256 é conferido (falha → `InferError::Checksum`, `.part` removido);
5. só depois o `.part` é renomeado para o nome final.

Um download rejeitado não deixa arquivo parcial com o nome canônico, e um
arquivo presente mas com hash divergente é apagado e baixado de novo. A
lógica é a mesma de `ensure_model` (`crates/tupi-infer/src/download.rs:72-103`)
usada pelo embedder e pelo gerador.

## Custo e limites

- **Disco**: ~1,1 GB por instalação que baixar o decisor; nada é baixado se
  ninguém chamar `ensure_decider_model`.
- **Rede**: uma transferência completa de ~1,1 GB na primeira vez; depois, só a
  leitura do arquivo para verificar o hash.
- **Build**: o `d1` não muda o binário — `local-models` continua opcional e o
  llama.cpp do nosso fork sobe em CPU por padrão (GPU só sob feature).
- **Batch**: uma questão inteira precisa caber num batch (`-b 8192`), o que
  exige `n_ctx`/batch maior que o dos carregamentos atuais — 2048 fixo em
  `llama.rs:37-51` e `llama_generate.rs:39-44`; o ajuste vive no carregador do
  decisor, não nos dois carregamentos existentes.

O teste de download usa `StubFetcher` (`download.rs:124-215`): nenhuma suíte
toca a rede nem exige o GGUF em disco. O smoke real só roda quando o arquivo já
está presente.
