# Escrita por extensão (task 0) — o veredito de T1/T2

Medição da task 0 do plano de escrita agnóstica: rodar sobre os turnos **já
gravados** e decidir se o endereço cirúrgico agnóstico paga. Nenhuma
instrumentação nova — o relatório junta eventos que o JSONL já tem
(`ToolStarted.arguments` × `ToolFinished.ok/error`, `crates/tupi-app/src/telemetry.rs:525-549`).

Reprodução (sonda `crates/tupi-app/tests/write_baseline.rs`, escreve
`target/write-baseline.txt`):

```text
cargo test -p tupi-app --test write_baseline -- --nocapture
TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl cargo test -p tupi-app --test write_baseline -- --nocapture
```

Fonte: `.agents/tupi/debug/telemetry.jsonl`, 11.972 linhas, 0 ilegíveis,
3.018 chamadas de tool. Tools de escrita: `patch` 434 · `edit` 118 ·
`write` 65 · `write_many` 12 · `relocate` 0.

## Chamadas por extensão × tool

`patch` não tem `path` no argumento (o caminho mora no diff), então cai em
"(sem path)"; `edit` sem path é 1 de 118 e vem de argumento ilegível.

| extensão | chamadas | write | write_many | edit | patch | ok | falhas |
| --- | --- | --- | --- | --- | --- | --- | --- |
| (sem path — `patch`) | 1.893 | 1 | 1 | 0 | 434 | 1.578 | 315 |
| `.rs` | 781 | 30 | 9 | 117 | 0 | 727 | 54 |
| (sem extensão) | 199 | 9 | 2 | 1 | 0 | 197 | 2 |
| `.md` | 81 | 16 | 11 | 0 | 0 | 72 | 9 |
| `.toml` | 43 | 6 | 10 | 0 | 0 | 43 | 0 |
| `.ron` | 14 | 3 | 0 | 0 | 0 | 14 | 0 |
| `.txt` / `.json` / `.jsonl` / `.lock` | 28 | 0 | 0 | 0 | 0 | 27 | 1 |

Leitura: **117 das 118 chamadas de `edit` são `.rs`** — o único endereço
cirúrgico que existe é o símbolo Rust (`crates/tupi-tools/src/tools/edit/params.rs:4-11`).
Fora de Rust não há `edit`: `.md` (16 `write` + 11 `write_many`), `.toml`
(6 + 10) e `.ron` (3) só têm arquivo inteiro, e `patch` (434) é o único
endereço agnóstico — pago com 315 recusas (72,6%), todas de endereço
(`corrupt patch at line N` e `patch failed: <arquivo>:<linha>`), nenhuma de hash.

## Quanto o `write` de fato muda

Diff real entre versões consecutivas do mesmo path no mesmo run (LCS por linha,
conteúdo não truncado):

| extensão | writes comparados | linhas mudadas | linhas do arquivo | razão | cirúrgicos (<10%) |
| --- | --- | --- | --- | --- | --- |
| `.rs` | 10 | 632 | 1.255 | 50,4% | 1 |
| `.md` | 3 | 108 | 262 | 41,2% | 0 |
| `.toml` | 3 | 21 | 115 | 18,3% | 0 |
| `.ron` | 1 | 21 | 73 | 28,8% | 0 |

O critério declarado ("write com diff <10% do arquivo em extensão X ⇒ o endereço
cirúrgico paga ali") aparece **1 vez em 17 writes comparáveis (5,9%)**. Esse
número mede o *efeito*, não a causa: o modelo só reescreve 41–50% de um arquivo
inteiro porque não tem âncora barata fora de Rust, e o endereço que sobra
(`patch`) recusa por aritmética de linha em 72,6% das tentativas. A extensão com
endereço (`​.rs`) é a única com `edit`, e lá a recusa é de conteúdo (10 símbolos
não encontrados, 5 hash), não de formato.

## Veredito

**T1 paga: sim** — gatilho: 117 de 118 `edit` são `.rs`, 0 `edit` em `.md`/`.toml`/`.ron`,
enquanto `.md` gasta 16 `write` + 11 `write_many` e `.toml` 6 + 10 em arquivo
inteiro; e o único endereço fora de Rust, `patch`, recusa 315 de 434 (72,6%) por
"corrupt patch"/"patch failed". Sem âncora literal única/faixa de linhas, a
escrita fora de Rust é arquivo inteiro ou diff que o modelo erra.

**T2 paga: sim** — gatilho: 5 recusas `StaleHash` em 118 `edit` (guarda usada de
verdade, `crates/tupi-tools/src/tools/edit/mod.rs:62-74`) e **0** recusas de hash
em `write`/`write_many`, contra 315 recusas de endereço em `patch`. A guarda de
hash já é uniforme entre linguagens e não é o gargalo; uniformizá-la no endereço
novo custa nada e é o que impede o caminho novo de dispensar hash
(`docs/decisions/0004-controle-por-tool.md:63-89`).

Ressalva declarada: o critério estrito de leitura (write com <10% de diff) dá
**1 de 17** — se a decisão fosse lida só por ele, o veredito de T1 seria "não".
Ele não separa "não há demanda" de "não há endereço"; os 434 `patch` com 72,6%
de recusa separam.
