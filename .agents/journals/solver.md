# Solver — journal

## 2026-02-14 - ParseCache crescia um `PathBuf` por parse (leak silencioso)

**Challenge:** `ParseCache` (`crates/tupi-tools/src/support/graph/parse_cache.rs`) limitava bytes, mas o índice de despejo era um `VecDeque<(PathBuf, u64)>` com marcadores preguiçosos: cada `insert` empurrava um item e só havia `pop_front` quando `bytes > max_bytes`. Com folga de orçamento — o caso comum em repo pequeno/médio — o `VecDeque` nunca era varrido, então cada reinserção do mesmo arquivo deixava um marcador morto permanente. O `GraphCache` invalida e reparseia *todos* os arquivos a cada rebuild, então o crescimento é `rebuilds × arquivos` de `PathBuf` para sempre. Bônus ruim: quando o limite finalmente estoura, o laço de despejo varre a pilha inteira de marcadores mortos numa única chamada sob o `write()`, produzindo um pico de latência.

**Pattern Applied:** Índice FIFO em `BTreeMap<u64, PathBuf>` chaveado pela `generation` (estritamente crescente). Reinserir/invalidar remove a chave antiga junto com a entrada, então `order.len() == entries.len()` por invariante — sem marcadores mortos, sem varredura. Despejo = `pop_first()` (O(log n)), e a checagem de geração obsoleta no laço desaparece porque a chave *é* a geração viva.

**Learning:** limite de memória implementado como "índice com lixo preguiçoso + limpeza só sob pressão" vira leak exatamente no regime em que o cache *não* está sob pressão. Prefira estruturas cujo tamanho acompanhe o número de itens vivos (mapa ordenado por sequência) a filas com marcadores mortos.

## 2026-02-14 - Falso positivo: `Notify` + contador NÃO tem lost wakeup

**Challenge:** o laço de `App::cancel` (`crates/tupi-app/src/app.rs`) espera as tools bloqueantes (`spawn_blocking`) terminarem com `loop { let notified = notify.notified(); if count.load(..) == 0 { break } notified.await }`, e o guard notifica com `notify_waiters()` apenas na transição 1→0. Parece o lost wakeup clássico: a futura só se registra no primeiro `poll`, e `notify_waiters()` não guarda permit.

**Pattern Applied:** refiz o laço com `tokio::pin!` + `Notified::enable()` e escrevi um teste que afirmava "futura não habilitada não observa `notify_waiters()`". O teste **falhou**: no tokio, `Notify::notified()` fotografa o contador de chamadas de `notify_waiters` na criação e, no primeiro `poll`, checa se o contador mudou. Qualquer `notify_waiters()` posterior à criação é observado no primeiro poll — a perda só acontece para futuras criadas *depois* do aviso.

**Learning:** antes de "corrigir" um laço `notified()` + contador, lembre que o tokio já fecha a janela via snapshot do contador; o que `Notified::enable()` garante é registro *eager* (usar quando a futura fica parada sem poll, ex. dentro de `select!`). A ordem correta do laço é criar a futura **antes** de checar o contador — que era exatamente a ordem do código original. Reverti a mudança; PR sem bug é ruído.
