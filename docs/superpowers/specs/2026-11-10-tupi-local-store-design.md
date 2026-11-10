# Tupi — Base local do app (redb)

**Data:** 2026-11-10
**Status:** aguardando aprovação
**Relacionado:** `2026-11-10-tupi-cli-design.md` (v1 do chat)

## Intenção e por quê

O Tupi nasce sem estado em disco: sessão em memória e catálogo de modelos buscado
na rede a cada `/models`. Isso custa latência, deixa o seletor vazio quando a
DeepSeek está fora e não dá base para o que vem depois (histórico, último
modelo/provedor, preferências).

Esta entrega cria a base local do aplicativo: um banco embutido **redb** em
`{home}/.tupi/tupi.redb`, com schema versionado desde a primeira versão. O
primeiro uso é cachear o catálogo de modelos; o desenho deixa espaço explícito
para as próximas tabelas, sem prometê-las agora.

## Objetivos

- Banco em `{home}/.tupi/tupi.redb`:
  - Linux/macOS: `$HOME/.tupi/tupi.redb`
  - Windows: `%USERPROFILE%\.tupi\tupi.redb`
  - sem `HOME` nem `USERPROFILE`: `.` (mesma queda de `state_dir()`)
- Novo crate `tupi-store`, dependendo só de `tupi-domain` e `redb`; kernel e TUI
  continuam sem saber que existe disco.
- Schema versionado em uma tabela `meta` (`schema_version`), com migração
  explícita e testada.
- `/models` responde do cache quando ele está fresco (TTL de 24 h), sem tocar na
  rede; quando vence, busca no provedor, grava e devolve.
- Falha de disco nunca quebra o chat: o app roda com um store vazio em memória e
  registra o motivo em log.

## Não-objetivos (v1 desta base)

- Persistir histórico de sessão (continua em memória).
- Persistir último modelo/provedor, tema, preferências.
- Sincronização entre máquinas, criptografia em repouso, retenção/limpeza.
- Múltiplos processos escrevendo ao mesmo tempo.

## Anti-objetivos

- A TUI ou o kernel importando `redb` ou conhecendo caminhos de arquivo.
- Chamar o banco dentro do loop de eventos da TUI e bloquear desenho/teclado.
- O cache virar fonte de verdade: ele é cópia do catálogo do provedor, nunca
  configuração editável à mão.
- Abrir o banco no caminho de desenho: a abertura acontece no composition root,
  antes da TUI.

## Restrições

- Rust 1.99, edition 2021, workspace existente.
- `redb` puro Rust, um arquivo, sem build C — escolhido por compilar igual em
  Linux, macOS e Windows.
- redb é síncrono: leituras/escritas rodam em `tokio::task::spawn_blocking`,
  nunca direto no handler assíncrono nem no loop da TUI.
- Sem dependência nova no binário para resolver o home: mesma abordagem de
  `config::from_env`, com função que recebe o getter de ambiente para ser
  testável.

## Abordagens consideradas

1. **redb (escolhida).** Rust puro, ACID, MVCC, um arquivo, API key-value com
   tabelas tipadas. Schema versionado por `meta` + migrações nossas. Cobre
   catálogo, histórico e preferências com folga. Sem SQL nem ferramentas de
   inspeção prontas.
2. **RocksDB.** LSM para escrita massiva e datasets grandes; o custo é build C++
   e toolchain por plataforma, especialmente no Windows. Desempenho que não
   vamos usar.
3. **SQLite (`rusqlite` com `bundled`).** Forte em consultas relacionais e
   migrações maduras, mas ainda é build C e mais um SQL para manter. Rejeitada
   porque o volume e o formato dos dados são key-value.

Decisão do autor: redb, mais simples.

## Arquitetura

```text
tupi        (bin)   composition root: resolve {home}, abre o Store, injeta no App
  |
tupi-app            orquestra; recebe Option<Arc<Store>>; spawn_blocking para I/O
  |
tupi-store          Store::open, catálogo, TTL, migrações
  |
tupi-domain         tipos puros (nenhum I/O)
```

- `tupi-store` não depende de `tupi-kernel` nem de `tupi-app`.
- `App::new` continua existindo sem store, para os testes atuais; a composição usa
  uma variante que recebe o store.
- Abertura: `Store::open(path)` cria diretório e arquivo se faltarem. Qualquer
  erro (permissão, arquivo travado por outra instância, corrupção) é logado como
  `warn` e a composição segue sem store.
- Como só existe uma instância de TUI, não há coordenação entre processos; com
  dois Tupis abertos, o segundo não abre o banco (redb trava o arquivo) e roda
  sem cache.

## Schema e migrações

Tabelas redb da versão 1:

| Tabela | Chave | Valor | Uso |
|---|---|---|---|
| `meta` | `&str` | JSON (`serde_json::Value`) | `schema_version`, metadados futuros |
| `models` | `&str` (provider id) | JSON `CachedModels` | catálogo por provedor |

```rust
pub struct CachedModels {
    pub models: Vec<String>,
    pub fetched_at_unix: u64, // segundos
}

impl CachedModels {
    /// `now` vem do chamador; o store nunca lê o relógio sozinho.
    pub fn is_fresh(&self, now_unix: u64, ttl_secs: u64) -> bool {
        now_unix.saturating_sub(self.fetched_at_unix) < ttl_secs
    }
}
```

Relógio para trás (`now < fetched_at`) cai no `saturating_sub` e conta como
fresco, evitando refetch em loop.

Migração: ao abrir, ler `meta.schema_version`.

- ausente → gravar `1` e criar as tabelas;
- igual à versão atual → seguir;
- menor → aplicar migrações em ordem (v1 não tem nenhuma ainda);
- maior que o binário conhece → não escrever nada; abrir como "sem store" e
  avisar em log (evita corromper dados de uma versão mais nova).

A chave do catálogo é o id do provedor. Se o `base_url` virar configurável, a
chave precisa incluir o endpoint; fica registrado para quando acontecer.

## Fluxo do `/models`

1. Handler recebe `Command::RefreshModels`.
2. Em `spawn_blocking`, o app chama `store.models(provider_id)` e avalia
   `is_fresh(now_unix, 24 * 3600)`; `now` vem de `SystemTime`.
3. Cache fresco: publica `Event::ModelsLoaded` e encerra, sem rede.
4. Cache ausente ou vencido: busca `provider.list_models()` como hoje.
   - sucesso → grava o cache (best-effort; falha de escrita só loga) e publica
     `ModelsLoaded`;
   - falha com cache vencido disponível → publica o cache e loga o erro (o
     usuário vê o catálogo salvo, não um erro);
   - falha sem cache → `ModelsFailed` como hoje.
5. A TUI não muda além do que já foi feito: mostra "carregando modelos…" e abre o
   overlay ao receber o evento. Só o evento `ModelsLoaded` existe por comando,
   então o overlay nunca é reaberto/reordenado por um segundo resultado.

TTL é constante nesta versão (`24 h`), sem configuração. Um comando explícito de
"atualizar catálogo" fica para quando houver demanda.

Escritas concorrentes (dois `RefreshModels` em voo) são best-effort: redb aceita
uma transação de escrita por vez; a que perder é logada e ignorada.

## Erros

- `StoreError` com `thiserror` em `tupi-store` (I/O, redb, JSON, versão).
- O app nunca propaga erro de store para o usuário: no máximo um `tracing::warn`
  e a operação segue sem cache.
- Catálogo corrompido é tratado como ausente e sobrescrito na próxima escrita.

## Testes

- `tupi-store`: round-trip de `CachedModels`; `is_fresh` fresco/vencido/relógio
  para trás; abertura em diretório inexistente; arquivo inválido → erro sem
  panic; `schema_version` maior → store indisponível sem escrita.
- `tupi-app`: com cache fresco, `RefreshModels` não chama o provedor e publica o
  cache; cache vencido chama o provedor e regrava; provedor falhando com cache
  vencido publica o cache.
- Integração (reusa o provider pendente): `RefreshModels` continua não
  bloqueando o handler.
- Resolução do caminho: testes com `get_env` injetado cobrindo HOME,
  USERPROFILE e ausência dos dois.

## Decisões deixadas ao implementador

- Nome do arquivo (`tupi.redb`) e do diretório (`.tupi`).
- Serialização do valor com `serde_json` (já no workspace).
- Se a leitura do cache acontece no mesmo `spawn_blocking` da escrita.
- Estrutura interna do `Store` (guardar `Database` redb e recriar tabelas por
  operação).
