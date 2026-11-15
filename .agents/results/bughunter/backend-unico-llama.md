# BugHunter — backend único do llama.cpp (modelos locais)

## 💡 O que

`crates/tupi-infer/src/backend.rs` (novo): `shared()` devolve um único
`&'static LlamaBackend` por processo, via `OnceLock` + `Mutex` de dupla
checagem. `LlamaEmbedder`, `LlamaGenerator` e `LlamaColBert` deixam de possuir um
`LlamaBackend` e passam a guardar a referência estática; os três `load` usam
`crate::backend::shared()` em vez de `LlamaBackend::init()`.

## 🎯 Por que

`LlamaBackend::init()` é global por processo. Com um `init` por `load`, o
segundo modelo carregado no mesmo processo morre com
`Unavailable("BackendAlreadyInitialized")`. Isso quebrava o app quando
embeddings de docs e geração de resumos rodam juntos, e quebrava a suíte por
ordem de execução: antes do fix, `cargo test -p tupi` dava

```
73 testes: 72 ok, 1 FAILED
config::tests::real_generation_model_produces_a_line
  → modelo carrega: Unavailable("BackendAlreadyInitialized")
config::tests::real_embedding_model_loads_with_1024_dims
  → ok   (ganhou a corrida do init)
```

## ✅ Verificação

```
cargo test -p tupi --bin tupi
```
O erro `BackendAlreadyInitialized` desaparece e os dois testes de modelo real
passam a conviver no mesmo processo (o de embedding segue ok e continua
produzindo 1024 dims).

Testes novos, determinísticos e sem gguf (rodam com
`--features local-models`, a mesma gate dos testes de modelo real):
`backend::tests::the_backend_is_initialized_once_and_reused` (mesmo ponteiro nas
duas chamadas) e `backend::tests::concurrent_callers_share_the_same_backend`
(quatro threads, um único endereço — falha se o `init` for feito fora do mutex).

## ⚠️ Falha remanescente, não causada por esta mudança

`config::tests::real_generation_model_produces_a_line` continua reprovando neste
ambiente, agora em outro ponto:

```
modelo carrega: Unavailable("null result from llama cpp")
```

Evidências de que é pré-existente e independente do fix:

- o gguf local tem `1_099_627_808` bytes, exatamente `LFM3_BYTES`, e magic
  `GGUF`;
- o modelo de embedding (379 MB, Q8_0) carrega pelo mesmo `shared()` e embeda;
- revertendo `shared()` ao comportamento antigo (um `init` novo por load, num
  processo que só roda esse teste) o erro é idêntico — logo não é
  `BackendAlreadyInitialized` mascarando, nem concorrência;
- `LlamaModelParams::default().with_n_gpu_layers(0)` também devolve o mesmo
  `null`, então não é offload/VRAM.

Conclusão: o llama.cpp linkado não carrega o `LFM3-1.7B-Q4_K_M.gguf`
(arquitetura/quantização), e os logs do llama.cpp estão desligados
(`with_logs_enabled(false)`), então a causa real não aparece na mensagem. O
teste não foi ignorado nem enfraquecido — fica registrado como defeito próprio.

## Arquivos tocados

- `crates/tupi-infer/src/backend.rs` (novo, com 2 testes)
- `crates/tupi-infer/src/lib.rs` (declara `mod backend;` sob
  `#[cfg(feature = "local-models")]`)
- `crates/tupi-infer/src/llama.rs`, `llama_generate.rs`, `llama_colbert.rs`
  (campo `&'static LlamaBackend`, `load` via `shared()`, `self.backend` em
  `new_context`)
