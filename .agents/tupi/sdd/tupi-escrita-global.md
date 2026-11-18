<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Escrita agnóstica de linguagem: endereço, guarda e verificação (`tupi-escrita-global`)

**Status:** completed

## Objetivo

Tornar a escrita do tupi tão universal quanto a leitura: todo arquivo de texto editável por endereço barato (âncora literal única ou faixa de linhas), com as mesmas guardas em qualquer linguagem (hash do arquivo, casamento único, verificação pelo verificador do próprio repo) e com parser de linguagem rebaixado a otimização opcional. Hoje o único endereço cirúrgico é `symbol`, disponível só em Rust (crates/tupi-tools/src/tools/edit/params.rs:4-11), enquanto `write` reescreve o arquivo inteiro (crates/tupi-tools/src/tools/write/mod.rs:15-28) e `patch` entra por diff unificado e recusa patch sem hunk `@@` (crates/tupi-tools/src/tools/patch/mod.rs:228-312). Antes de qualquer código, medir por extensão quanto do arquivo um `write` de fato altera e qual endereço é usado (docs/features/tupi-telemetry.md; crates/tupi/src/telemetry_report.rs).

## Restrições

- Nenhum caminho novo de escrita dispensa hash: `expected_hash` continua obrigatório em sobrescrita e toda escrita segue por `paths::resolve_for_write` (crates/tupi-tools/src/support/paths.rs:53-101; docs/decisions/0004-controle-por-tool.md:63-89).
- Toda instrução nova precisa de contrato executável (tool que recusa) ou número em configs/*.ron: nada de "seja econômico" no prompt (docs/decisions/0004-controle-por-tool.md:63-89).
- Não criar escape: sem --force, sem modo permissivo, sem confirmação que libere o que a guarda recusa.
- Parser de linguagem é otimização opcional por linguagem, nunca pré-requisito de escrita: o caminho padrão é texto.
- Resolver por texto só vale com casamento ÚNICO; 0 ou >1 ocorrência é recusa com contagem e trecho, no padrão de recusa que ensina (docs/decisions/0002-shell-poder-e-ruido.md).
- Tasks 1-4 são condicionadas ao resultado da task 0; task 0 não muda código de produção.
- Escopo restrito ao lado da escrita (write, write_many, edit, patch, relocate, verificação); não alterar tools de leitura nem o contrato de lookup/map/graph.
- Prosa e documento seguem o mesmo invariante das tools: âncora `caminho:linha` verificável ou a afirmação não entra.

## Nuances

- Assimetria atual: leitura tem seleção (map/lookup skeleton/lines/text/graph/semantic), orçamento, dedup de entrega e âncora por hash; a escrita tem só arquivo inteiro ou símbolo Rust.
- `edit` exige path+symbol+expected_hash+new_body+signature+budget_tokens e resolve por SpanResolver/AST, reparseando e restaurando o original quando o disco não bate (crates/tupi-tools/src/tools/edit/params.rs:4-11; crates/tupi-tools/src/tools/edit/mod.rs; crates/tupi-tools/src/tools/edit/splice.rs).
- `patch` é o único endereço já agnóstico, mas entra por diff unificado: recusa patch sem `@@`, roda `git apply --check` antes de aplicar, aceita `-R` para reverter e `check_only` para não escrever (crates/tupi-tools/src/tools/patch/mod.rs:228-312; testes em mod.rs:359-382).
- Diff exige aritmética de número de linha, que modelo erra; âncora literal exata é formato de entrada mais confiável e igualmente genérico. Diff segue sendo o bom formato de revisão e rollback.
- `write_many` já é transacional com rollback e teto MAX_WRITES (crates/tupi-tools/src/tools/write_many/mod.rs); `relocate` move/copia com hash e recusa symlink (crates/tupi-tools/src/tools/relocate/mod.rs).
- A doutrina declara edição cirúrgica como caminho normal e `write` inteiro como exceção declarada, mas isso só é verdade em Rust — a regra e o código discordam fora de Rust (docs/decisions/0004-controle-por-tool.md:63-89).
- Verificação pós-escrita não pode depender de parser do tupi: o verificador do repo (cargo fmt --check, tsc --noEmit, ruff, pytest, sqlfluff, markdownlint) é o juiz, chamado por `exec` com timeout e cache (docs/features/tupi-telemetry.md lista só métricas de turno; `exec` já existe com cache).
- Telemetria já grava JSONL por turno e `TurnRow` agrega steps/provider_us/tool_us/re-read/tokens, mas não quebra por tool nem por extensão de arquivo — a medição da task 0 exige essa quebra (crates/tupi/src/telemetry_report.rs; docs/features/tupi-telemetry.md).
- Para documento, a checagem barata é âncora viva: `caminho:linha` conferido contra o blake3 atual do arquivo; âncora morta falha o check em vez de envelhecer em silêncio.

## Tasks

- [x] **T0** Medir a escrita real por tool e por extensão
  - Ler o JSONL de telemetria (crates/tupi-app/src/telemetry.rs) e produzir quebra nova no relatório (crates/tupi/src/telemetry_report.rs): por extensão de arquivo, quantas chamadas de write/write_many/edit/patch, quantas linhas o diff de cada write de fato mudou contra o tamanho do arquivo, e quantas recusas de edit são de hash (StaleHash) e quantas são por 'não é Rust'. Não alterar tools de produção. Saída: tabela por extensão + veredito binário sobre as tasks T1/T2 (paga ou não).
  - Nuance: TurnRow hoje agrega steps/provider_us/tool_us/re-read/tokens; falta quebra por tool e por extensão (docs/features/tupi-telemetry.md).
  - Nuance: O evento ToolFinished já existe no JSONL; o trabalho é de agregação, não de instrumentação nova.
  - Nuance: Critério de leitura: write com diff <10% do arquivo em extensão X é sinal de que o endereço cirúrgico paga naquela extensão.
  - [x] **T0.1** Confirmar os campos disponíveis no JSONL para tool e para o caminho tocado — Lista de campos reais lidos do arquivo de telemetria com um exemplo de linha; nenhum campo presumido.
  - [x] **T0.2** Implementar a agregação por extensão e por tool — Comando do relatório imprime, por extensão, contagem de cada tool de escrita e razão linhas-mudadas/tamanho nos writes.
  - [x] **T0.3** Rodar sobre os turnos já gravados e escrever o veredito — Tabela com números reais e frase explícita 'T1 paga: sim/não' e 'T2 paga: sim/não' com o gatilho citado.
- [x] **T1** Endereço por âncora literal única no edit
  - Aceitar em EditParams o par old/new como alternativa a symbol+new_body (crates/tupi-tools/src/tools/edit/params.rs:4-11). Resolver por casamento literal exato do bloco old no arquivo, exigindo exatamente 1 ocorrência; 0 ocorrências recusa citando que o trecho não existe, >1 recusa citando a contagem e as linhas. expected_hash continua obrigatório e o caminho segue por paths::resolve_for_write. Sem parser envolvido. Nenhum force/escape.
  - Nuance: É a forma genérica do que edit faz em Rust, e cobre md/yaml/toml/py/ts/go/sql.
  - Nuance: A recusa por ambiguidade é contrato, não aviso: é o análogo textual de StaleHash.
  - Nuance: Diff continua sendo formato de revisão; âncora é formato de entrada.
  - [x] **T1.1** Estender EditParams com old/new opcionais e validação de exclusividade — Parâmetro inválido (nem symbol nem old, ou ambos) é recusado com mensagem que diz qual usar; teste cobre os dois erros.
  - [x] **T1.2** Implementar a substituição literal única mantendo o contrato de hash e o restore em divergência — Testes: edição em .md e .yaml com sucesso; âncora inexistente recusa; âncora ambígua recusa com contagem; hash velho recusa sem tocar no disco.
  - [x] **T1.3** Atualizar spec da tool e documentação de convenções — docs e a descrição da tool dizem quando usar old/new, symbol ou write inteiro, sem contradizer docs/decisions/0004-controle-por-tool.md.
- [x] **T2** Endereço por faixa de linhas
  - Aceitar start/end com expectativa do trecho atual (para não editar às cegas) e expected_hash do arquivo. Se o trecho em disco não for o esperado, recusar. Serve para casos em que o texto é ambíguo por natureza (linhas repetidas) e para arquivos grandes onde a âncora é desconfortável.
  - Nuance: Faixa sem expectativa de conteúdo é escrita às cegas: proibida.
  - Nuance: É o caso mais próximo do que patch já faz; a diferença é não exigir diff nem git.
  - [x] **T2.1** Definir parâmetros e regra de conflito com old/new e symbol — Precedência documentada e testada; combinação ambígua recusa com a regra na mensagem.
  - [x] **T2.2** Implementar com verificação do trecho esperado — Testes: faixa correta aplica; faixa deslocada por hash velho recusa; faixa com conteúdo divergente recusa sem escrever.
- [x] **T3** Verificação pós-escrita pelo verificador do repo
  - Permitir declarar, por glob de caminho, o comando de verificação do ecossistema (cargo fmt --check, tsc --noEmit, ruff, pytest, sqlfluff, markdownlint) e rodá-lo via exec depois de aplicar escrita, opt-in. Falha do comando é recusa com a saída colada e o comando citado; a escrita já aplicada é informada, não desfeita silenciosamente. Comando é dado de configuração, não código por linguagem.
  - Nuance: O juiz é o verificador do repo, nunca o parser do tupi.
  - Nuance: exec já tem timeout e cache, e a allowlist de shell não é afetada.
  - Nuance: Opt-in por projeto, para não mudar o comportamento de quem não declarou nada.
  - [x] **T3.1** Definir o formato de configuração por glob e a resolução do comando aplicável — Um arquivo .md e um .ts de exemplo resolvem para comandos distintos; glob sem comando é no-op explícito.
  - [x] **T3.2** Rodar a verificação pós-aplicação e reportar resultado — Teste com comando que falha devolve recusa citando comando e saída; comando ausente não derruba o turno, só avisa.
- [x] **T4** Parser de linguagem como plugin opcional
  - Rebaixar o caminho Rust/AST a um resolver entre vários: se a linguagem tem parser e o modelo pediu symbol, usa; senão cai em old/new ou faixa. Adicionar linguagem passa a ser tabela de suporte, não tool nova. Não ampliar poder de escrita: as guardas são as mesmas.
  - Nuance: Hoje edit resolve por SpanResolver/AST e reparseia (crates/tupi-tools/src/tools/edit/splice.rs).
  - Nuance: Só faz sentido depois de T1/T2 provarem que o endereço textual é o padrão.
  - [x] **T4.1** Extrair a resolução por símbolo atrás de uma interface única de endereço — edit escolhe resolver por dado (symbol, old/new, faixa) sem if de linguagem espalhado; testes existentes de Rust continuam verdes.
  - [x] **T4.2** Documentar como habilitar uma linguagem nova — Passo a passo em docs, com o que é obrigatório (guarda de hash e unicidade) e o que é opcional (parser).
- [x] **T5** Documento com âncora viva
  - Checagem que valida toda âncora caminho:linha em docs/ contra o blake3 atual do arquivo e a existência do trecho, falhando quando a linha morreu. Barato, agnóstico de linguagem e o único controle objetivo de prosa que existe hoje.
  - Nuance: É o mesmo invariante da memória ancorada, aplicado a documento.
  - Nuance: Não julga estilo: julga se a afirmação ainda corresponde ao código.
  - [x] **T5.1** Definir o formato da âncora e o parser mínimo de docs — Âncoras atuais de docs/features são reconhecidas ou declaradas fora de escopo; formato documentado.
  - [x] **T5.2** Implementar o check com lista de âncoras mortas — Rodado neste repo, imprime as âncoras mortas com caminho, linha citada e motivo (arquivo mudou/linha não existe).
