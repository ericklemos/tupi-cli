# BugHunter — journal

## LlamaBackend::init é global por processo: dois modelos no mesmo processo quebram

**Bug/Gap:** `LlamaEmbedder::load`, `LlamaGenerator::load` e `LlamaColBert::load`
chamavam `LlamaBackend::init()` cada um. A segunda chamada no mesmo processo
devolve `Unavailable("BackendAlreadyInitialized")`, então carregar embeddings e
geração juntos (ou dois testes de modelo real na mesma suíte) falha por ordem de
execução. A suíte `cargo test -p tupi` reprovava em
`config::tests::real_generation_model_produces_a_line` e passava em
`config::tests::real_embedding_model_loads_with_1024_dims` — puro azar de ordem.

**Learning:** o backend do llama.cpp é estado global do processo, mas estava
guardado como campo possuído em três structs independentes. Manter o dono no
struct garante um `init` por *load*, não por *processo*. Testes que carregam
modelos reais compartilham o processo e por isso escondem/mostram a falha
conforme o escalonamento das threads.

**Action:** `crates/tupi-infer/src/backend.rs` guarda o backend num `OnceLock`
protegido por `Mutex` (dupla checagem) e os três loaders recebem
`&'static LlamaBackend`. Teste determinístico sem gguf:
`backend::tests::the_backend_is_initialized_once_and_reused` e
`concurrent_callers_share_the_same_backend`. Regra para o futuro: qualquer
coisa que seja `init` global do llama.cpp passa por `backend::shared()`, nunca
por `LlamaBackend::init()` direto.

## Depois do fix sobra uma falha que não é de código: LFM3 não carrega neste build

**Bug/Gap:** `LlamaGenerator::load` do gguf `LFM3-1.7B-Q4_K_M.gguf` devolve
`Unavailable("null result from llama cpp")`. O arquivo local tem exatamente
`LFM3_BYTES` e magic `GGUF`, e o modelo de embedding (Q8_0, mesmo código, mesmo
backend) carrega e produz 1024 dims.

**Learning:** sondado duas vezes: (a) com o backend próprio por load
(comportamento antigo, `Box::leak` de um `init` novo) o erro é o mesmo; (b) com
`LlamaModelParams::default().with_n_gpu_layers(0)` também. Ou seja: não é
regressão do backend compartilhado nem falta de VRAM — o llama.cpp linkado não
aceita esse gguf (arquitetura/quantização). Os logs do llama.cpp estão
desligados por `send_logs_to_tracing(..with_logs_enabled(false))`, então a causa
real não chega ao erro: só o `null` genérico.

**Action:** ao investigar falha de carregamento de modelo local, comparar com um
segundo modelo conhecido-bom no mesmo processo e testar o caminho antigo antes
de culpar a mudança. Não enfraquecer o teste que reprova; reportar o artefato.

## Paginação por offset precisa de invariante testada: orçamento zero trava o chamador

**Bug/Gap:** `PeekTool::peek` com `budget_tokens == 0` devolvia página vazia e
`Próximo offset:` idêntico ao offset atual (`Próximo offset: 0.`), porque
`chars_for_budget(0) == 0`. O envelope convida o chamador a seguir o offset, então
o resultado é um loop estéril que queima requests sem entregar um caractere. O
schema JSON declara `minimum: 1` para o campo, mas schema não é validação — o
runtime aceitava zero.

**Learning:** toda tool paginada (`peek`, `read` com `budget_tokens`, envelope de
`support::budget`) tem uma invariante não escrita: *página não-final avança o
offset*. Os testes existentes só usam orçamentos >0, então o caso degenerado era
invisível. O padrão do codebase para orçamento insuficiente é "deixe entrar a
unidade mínima" (`support::budget::select` mantém a seção de maior prioridade
mesmo estourando o teto), não "devolva vazio".

**Action:** qualquer parser de `budget`/`limit`/`offset` precisa de um teste com
o valor 0 e outro com o valor máximo; a asserção deve ser sobre o *progresso*
(novo offset > offset atual ou `Fim do corpo.`), não só sobre o texto da página.
Fix aplicado: `chars_for_budget(..).max(1)` + teste
`peek::tests::a_zero_budget_still_advances_the_offset`.

## 2026-11-16 - Fixture de teste apoiada em artefato gerado no build

**Bug/Gap:** 6 testes de `crates/tupi-tools/src/tools/workflow/mod.rs` reprovavam
porque fixavam os nomes nativos `feature`, `fix-bug` e `review`, que não estão
mais em `configs/workflows/*.ron`. `NATIVE` é gerado por `build.rs` em
`OUT_DIR/native_workflows.rs` (`include!`), então o catálogo "que viaja no
binário" é config de build, não código: o nome sumido não aparece em nenhum
`match`, só no teste que falha.

**Learning:** teste de precedência de camadas (`nativo < global < local`) não
deve escolher o nome nativo por memória; o nome existe só se o `.ron` existir.
Com um nome que não existe, `merge` não tem nativo para sombrear e a asserção
`shadowed == [Native]` falha com `[]` — sintoma que parece bug de precedência e

## 2026-11-15 - Editor de uma linha reaproveitando o wrapper do transcript

**Bug/Gap:** `draw_composer` quebrava `state.input` com `wrap_text`, que é
`text.split_whitespace()`: o espaço digitado no fim do rascunho desaparecia. O
cursor sai da última linha quebrada, então `"oi "` desenhava `"oi"` e o cursor
ficava parado — a tecla espaço só tinha efeito visível quando a letra seguinte
chegava. Espaços repetidos no meio (`"a  b"`) também viravam um só.

**Learning:** o composer é um editor, não texto formatado. Reutilizar a quebra
do transcript (que colapsa espaço de propósito, para markdown) num campo de
digitação transforma "espaço não renderizado" em "cursor não anda", e nenhum
teste de texto pegaria isso: a única asserção capaz de reprovar é sobre a
posição do cursor (`TestBackend::assert_cursor_position`).

**Action:** rascunho passa por `wrap_input` (`crates/tupi-tui/src/run.rs`), que
preserva cada espaço e só decide *onde* quebrar. Testes:
`run::tests::trailing_space_moves_the_composer_cursor` e
`run::tests::repeated_spaces_are_rendered_in_the_composer`. Regra para o
futuro: antes de reusar `wrap_text`/quebra de markdown num campo editável,
perguntar se o espaço é conteúdo — em editor, é.
