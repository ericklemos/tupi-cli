# 🤨 Skeptic: o cache do `exec` nunca acerta — a chave inclui o log da própria sessão

**Data:** 2026-11-16 · **Alvo:** `crates/tupi-tools/src/tools/exec/`, `crates/tupi-tools/src/lib.rs`
**Veredito:** o cache anunciado na descrição da tool (`spec.description`: "com erros agrupados e cache") e o "cache persistente" do `ToolsOptions` não existem. Não é lentidão: é promessa falsa com custo por chamada.

## 💥 A Catástrofe

Sequência exata, sem hipótese:

1. `ExecTool::call` monta a chave como `args.join(" ") + fingerprint(root, 5_000)` (`exec/mod.rs:175-176`).
2. `fingerprint_with` percorre **todo** o workspace e faz blake3 do **conteúdo** de cada arquivo; só ficam de fora `target/`, `.git/`, `.brain/` e `.agents/tupi/` (`exec/render.rs:56-137`, exclusão em `:88`).
3. `.tupi/` **não** está na lista. Tupi anexa a `.tupi/sessions/<id>/log.jsonl` a **cada** interação de tool. Medido nesta sessão: 139.534 → 141.168 bytes entre duas chamadas consecutivas, 4 s de intervalo.
4. Logo, a chave muda antes de qualquer segunda chamada de `exec` poder reutilizar algo. Cache morto por construção.

Verificado ao vivo: quatro `exec kind=check package=tupi-kernel` idênticos, **zero** ocorrências de `em cache há`; cada um reportou `0.1s` (o cargo rodou de fato). Isso fecha a dúvida #10 do registro: não é o tempo de vida do processo — há um `ExecTool` por registry (`crates/tupi-tools/src/lib.rs:293`) — é arquivo sob o root mudando entre chamadas.

## 🕵️ O Ponto Cego

- **O caminho de acerto não tem teste.** O único teste de cache (`exec/tests.rs:177`) entrega uma idade pronta para `sections_for`; nada chama `call` duas vezes e exige `em cache há`. Por isso um cache 100 % miss passou verde: o renderizador é testado, o cache não.
- **O custo é invisível por desenho.** `sections_for` reporta `raw.duration`, a duração do processo cargo (`exec/diagnostics.rs:71-93`). O walk + hash de 803 inodes / 25,2 MB acontece antes e não aparece em lugar nenhum.
- **Exclusões erradas nos dois sentidos.** Faltam `.tupi/` (log de sessão, SDD, locks), `.idea/` e `.agents/results/`+`docs/` (este relatório, ao ser gravado, invalida a chave de novo). Sobram justificativas: nada disso muda um build.
- **Duas outras camadas já nasceram mortas.** `ToolsOptions::{exec_cache, web_cache}` são documentados como "cache persistente" (`lib.rs:57-60`), default `true` (`:89-90`), forçados `false` em `hermetic` (`:109-110`) e **lidos em nenhum lugar** — grep só acha a declaração e os dois construtores.

## 💸 O Custo

- Por chamada: walk completo + blake3 de ~25 MB antes do cargo, sem retorno nenhum. O modelo paga em latência e não recebe economia de token alguma.
- Por decisão de arquitetura: um sinal de configuração que mente. Quem desliga `exec_cache` (ou confia no default `true`) acredita controlar um cache que não existe. Isso é a mesma classe de falha do risco #8 (ponteiro `repeat="allow"`): schema/doc afirmando o que o código não faz.
- Confiança: a descrição da tool é o contrato que o modelo lê para escolher entre repetir ou não uma verificação. Ela promete um comportamento que não ocorre.

## 🛡️ O Ultimato

Antes de "consertar" o cache, responda:

1. **Qual é a chave correta?** Precisa ser `(args, insumos de build)` — código-fonte, manifests, fixtures, `Cargo.lock` — e não "todo byte sob o root". Ou se exclui o estado do próprio agente (`.tupi/`, `.idea/`, `docs/`, `.agents/results/`), ou se aceita que a feature é inútil.
2. **O que a chave ignora e o cargo lê?** Só o *nome* de `RUSTUP_TOOLCHAIN` entra (`render.rs:39-45`), nunca a versão resolvida: `rustup update` troca o compilador e a chave não muda. Ficam fora também `~/.cargo/config.toml`, `CARGO_HOME`, `CARGO_NET_OFFLINE`, `SQLX_OFFLINE`, `DATABASE_URL`, `CC`/`CFLAGS`, `PATH`.
3. **Teste de acerto antes de qualquer otimização.** Um teste que instancia `ExecTool`, chama `call` duas vezes e exige `em cache há`, mais um que grava `.tupi/sessions/**/log.jsonl` entre as duas e exige **miss**.
4. **Mate os flags mortos ou ligue-os.** `exec_cache`/`web_cache` como estão são documentação falsa; um `debug_assert!`/consumo real é o mínimo.

**Consequência de ignorar:** hoje o defeito é só desperdício. No dia em que alguém excluir `.tupi/` da chave — a correção óbvia — o cache passa a acertar e o item 2 vira **verde velho**: um `ok` cacheado sobrevivendo a `rustup update` ou a `CARGO_NET_OFFLINE=1`. Os dois defeitos se cancelam; corrigir um ativa o outro.

## Como me provar errado

- `exec kind=check package=tupi-kernel` duas vezes com `em cache há` na segunda ⇒ a chave não está sendo poluída (não consegui: 4 tentativas, 0 acertos).
- Teste de integração que chame `call` duas vezes no mesmo `ExecTool` e observe hit ⇒ o mecanismo é outro.
- `grep -rn exec_cache crates` devolvendo um consumidor além de `lib.rs:58/89/109` ⇒ o flag não é morto.

## Revisão (malícia, escala, estado)

- **Malícia:** um repositório com um arquivo que muda a cada segundo (log, lock, cache de IDE) desliga o cache de qualquer agente — negação de serviço silenciosa e sem erro.
- **Escala:** acima de 5.000 arquivos ou 64 MB o `fingerprint` devolve `None` e o cache desliga **calado** (`render.rs:99-103`); o modelo continua lendo "cache" na descrição.
- **Estado:** o cache é um único `Mutex<Option<Cached>>` em RAM: reiniciar o processo perde tudo, e o flag que promete persistência não toca disco.
