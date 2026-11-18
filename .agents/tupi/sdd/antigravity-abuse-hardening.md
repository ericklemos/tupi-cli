<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Antigravity: alinhar cliente ao Hub oficial e cortar tráfego que vira ban (`antigravity-abuse-hardening`)

**Status:** closed

## Objetivo

Eliminar as divergências entre o provider Antigravity do tupi e o cliente Hub oficial (referência: plugin opencode-antigravity-auth), cobrindo de ponta a ponta: erro/limite visível (não engolido), parser e auth resilientes, backoff/cooldown por conta, identidade de requisição (system instruction, session/conversation id, requestId estável), versão/CL dinâmica com gate mínimo, cache e filtragem de modelos, e schema de ferramentas robusto. Objetivo de saída: nenhuma chamada nossa carrega fingerprint que o Hub não carregue, e nenhum erro do Google vira retry silencioso. Fontes: crates/tupi-providers/src/antigravity.rs:112-202,204-334,345-405,417-419,425-456,498-516,518-557,612-640,750-793,795-836,1104-1189 (nosso código); plugin de referência: src/constants.ts (getAntigravityHeaders, getAntigravityUserAgent, ANTIGRAVITY_MINIMUM_VERSION, ANTIGRAVITY_SCOPES), src/plugin/request.ts, README.

## Restrições

- Não mexer no conjunto de headers de generate/loadCodeAssist/fetchAvailableModels: o Hub manda só User-Agent (antigravity.rs:813-821, 1117-1122); X-Goog-Api-Client e Client-Metadata existem só no onboardUser (antigravity.rs:1157) — igual à referência.
- Não alterar client_id, redirect_uri nem os 5 escopos OAuth (antigravity.rs:19-22, 962-981): já idênticos à referência.
- Nunca apontar para o host de produção cloudcode-pa: só daily-cloudcode-pa.googleapis.com (antigravity.rs:23; referência: getEndpointFallbacks não faz fallback entre hosts).
- Nenhum retry infinito: backoff tem teto e o erro final precisa subir como ProviderError visível ao usuário.
- Manter a assinatura pública build_body/resolve_api_model (há testes em antigravity.rs:1318-1506 que dependem dela).
- Leitura do manifest de versão precisa de timeout curto e fallback offline-first (2.24.0), nunca bloquear o boot.
- Fora de escopo: provar que o ban para. O gatilho é política não documentada do Google; aqui só se remove a divergência de comportamento e volume.
- Dados de conta: não gravar o arquivo do plugin; importar por cópia, nunca compartilhar refresh token vivo com outro cliente.

## Nuances

- UA do Hub: antigravity/hub/<versao> (aidev_client; os_type=darwin; arch=arm64; cl=<changelist>) com changelists conhecidos 2.23.0=998057262, 2.23.1=998881661, 2.24.0=1000335682; versão resolvida em runtime do manifest, fallback 2.24.0, mínimo aceito 2.23.0 (referência src/constants.ts; nosso valor congelado em antigravity.rs:25-28).
- O Hub injeta uma system instruction fixa ('You are Antigravity...') com role=user em todo request de estilo antigravity; nós não injetamos nada (antigravity.rs:204-334).
- Onboard: UA = UA do Hub + ' google-api-nodejs-client/10.3.0' e X-Goog-Api-Client gl-node/22.21.1 (antigravity.rs:25-28,1152-1162) — já conforme.
- Envelope do request tem project, model, request, requestType='agent', userAgent='antigravity', requestId (antigravity.rs:326-333); hoje requestId = 'agent-'+blake3(nanos:contador), único por chamada (antigravity.rs:77-86), e não há conversationId/sessionId.
- skip_thought_signature_validator é recurso suportado do Google, mas a referência o usa só como fallback de cache miss (signature-store por sessão/modelo/projeto); nós colocamos em todo primeiro functionCall (antigravity.rs:132-136).
- Erro dentro do SSE não é detectado: extract_chunk ignora {error:...} (antigravity.rs:345-405) e stream_chat só checa status HTTP inicial (antigravity.rs:832-836).
- Payload não-JSON em qualquer evento vira ProviderError::Protocol e encerra o stream (antigravity.rs:349-351).
- Refresh só por relógio local: is_expired = now+60 >= expires_at (antigravity.rs:417-419), chamado em ensure_auth (antigravity.rs:612-640); não existe caminho de refresh após 401.
- Project id: loadCodeAssist -> onboardUser (até 4 tentativas, sleep 2s) -> default 'rising-fact-p41fc' (antigravity.rs:1104-1189); o default é a mesma constante da referência.
- Listagem de modelos: fetchAvailableModels a cada chamada, fallback fixo SUPPORTED_MODELS quando falha (antigravity.rs:750-793, 33-57), sem cache (referência cacheia ~1h) e sem filtrar entitlement real.
- Referência anexa sufixo ':antigravity' ao id do modelo no estilo antigravity (constants.ts: endpoint de produção é 'used for models without :antigravity suffix'); nosso resolve_api_model nunca anexa (antigravity.rs:90-110) — hipótese a validar em teste real.
- Thinking: referência resolve thinkingConfig por tier e aplica orçamento máximo para Claude + header anthropic-beta: interleaved-thinking-2025-05-14; nós só emitimos generationConfig.maxOutputTokens quando request.max_tokens existe (antigravity.rs:305-310).
- Contas: load_from_dirs cai em ~/.config/opencode/antigravity-accounts.json e pega só accounts[activeIndex] com access_token vazio e expires_at 0 (antigravity.rs:425-456,498-516); a referência mantém N contas com rotação, cooldown e quota por conta.
- Schema de ferramentas: referência usa placeholder para schema vazio, hardening de instrução e fixToolResponseGrouping/validateAndFixClaudeToolPairing; nós só removemos chaves não suportadas (antigravity.rs:112-202).
- Escopos OAuth, headers de generate e project default foram checados e já estão conformes — não são causa de divergência.

## Tasks

- [x] **T1** Erro do Google visível: ramo {error:…} no SSE e no HTTP
  - Hoje erro dentro do stream é engolido (antigravity.rs:345-405) e só o status HTTP inicial é checado (antigravity.rs:832-836). Fonte: nuances da spec sobre extract_chunk/stream_chat; referência: src/plugin/recovery.ts (detectErrorType).
  - Nuance: Payload de erro do Cloud Code vem como {"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":…}} ou embrulhado em response/candidates=NONE; preservar o corpo cru em last_payload para diagnóstico.
  - [x] **T1.1** Adicionar variante/mapeamento de erro tipado no ProviderError (RateLimited com retry_after, QuotaExhausted com reset, InvalidArgument) em extract_chunk — extract_chunk devolve Err com status e corpo originais quando o payload traz {error:{code,status,message}}; teste unitário cobre 429/400/500
  - [x] **T1.2** stream_chat encerra o stream com Err ao receber erro mid-stream, sem emitir Usage/ToolCalls depois — teste com SSE contendo texto + bloco de erro garante que o consumidor recebe Text e depois Err, nunca término limpo
  - [x] **T1.3** Guardar o último payload de erro e expor via last_payload — last_payload passa a conter o envelope do último request com erro; teste verifica leitura
- [x] **T2** Parser SSE tolerante: linha não-JSON não mata o turno
  - antigravity.rs:349-351 devolve Protocol em qualquer payload não-JSON; keepalive/linha parcial derruba o turno. Fonte: nuance 'Payload não-JSON em qualquer evento vira ProviderError::Protocol'.
  - [x] **T2.1** Tratar payload não-JSON como evento ignorável (keepalive), mantendo contador de lixo para diagnóstico — stream com 3 keepalives + 2 eventos válidos entrega os 2 eventos e termina Ok
  - [x] **T2.2** Garantir que SseParser jamais entrega evento incompleto e drene o buffer final ao fechar — teste de sse.rs com evento cortado no meio de duas leituras + flush final
  - [x] **T2.3** Teste de regressão end-to-end do stream com lixo intercalado e erro no fim — teste em antigravity.rs cobre keepalive, evento truncado e erro final na mesma sessão
- [x] **T3** Auth resiliente: refresh em 401 e skew com jitter
  - is_expired = now+60 >= expires_at (antigravity.rs:417-419) e ensure_auth sem caminho de refresh após 401 (antigravity.rs:612-640).
  - [x] **T3.1** ensure_auth(force: bool) e janela de refresh com skew (5min) + jitter — token com validade a 4min do fim é renovado antes do request; teste com relógio injetado
  - [x] **T3.2** stream_chat e list_models refazem auth uma única vez em 401 e repetem a chamada — mock 401 seguido de 200 produz resposta única e sem duplicar eventos para o consumidor
  - [x] **T3.3** invalid_grant/invalid_client viram erro claro de relogin (não loop de refresh) — erro de refresh não é retentado; mensagem instrui /login do provider e teste garante 1 única tentativa
- [x] **T4** 429/RESOURCE_EXHAUSTED: backoff com teto e cooldown por conta
  - Repo não tem nenhuma ocorrência de 429 e não há cooldown por conta; referência mantém rotação + resfriamento (src/plugin/accounts.ts). Combinar com T1/T8.
  - Nuance: Honrar Retry-After quando presente; sem Retry-After usar backoff exponencial 2s/4s/8s com teto de 3 tentativas.
  - Nuance: Cooldown precisa sobreviver a restart (persistir junto das credenciais no redb).
  - [x] **T4.1** Camada de retry tipada no provider respeitando Retry-After e teto de tentativas — 3 respostas 429 em sequência produzem 3 tentativas e um Err final; teste controla o tempo virtual
  - [x] **T4.2** Cooldown por conta persistido com reset_at, consultado antes de qualquer request — conta em cooldown não gera request; teste reinicia o provider e o cooldown continua ativo
  - [x] **T4.3** Superfície no app: mensagem de quota com horário de reset e sem retry automático do turno — teste em tupi-app garante que erro de quota não dispara nova chamada ao provider
- [x] **T5** Thinking config e Claude: generationConfig correto e anthropic-beta
  - build_body só emite maxOutputTokens quando request.max_tokens existe (antigravity.rs:305-310); modelos -high/-xhigh e claude-*-thinking exigem orçamento. Referência: resolveThinkingConfig/CLAUDE_THINKING_MAX_OUTPUT_TOKENS e header interleaved-thinking-2025-05-14.
  - [x] **T5.1** Resolver tier do modelo para thinkingConfig.budget dentro de generationConfig — build_body para tier high/medium/low produz budgets distintos e previsíveis; testes em antigravity.rs
  - [x] **T5.2** Modelos Claude thinking: maxOutputTokens mínimo garantido e header anthropic-beta adicionado — teste verifica header presente e maxOutputTokens no piso definido
  - [x] **T5.3** Não regredir request sem tools e sem max_tokens — teste body_omits_max_output_tokens_when_unset continua passando e thinkingConfig ausente para modelo não-thinking
- [x] **T6** Identidade de requisição: system instruction, sessão e requestId estável
  - build_body não injeta a system instruction do Antigravity (antigravity.rs:204-334) e requestId é único por chamada (antigravity.rs:77-86); referência injeta ANTIGRAVITY_SYSTEM_INSTRUCTION com role=user e mantém chaves por sessão/modelo/projeto.
  - [x] **T6.1** Injetar ANTIGRAVITY_SYSTEM_INSTRUCTION no systemInstruction (role=user) preservando as system messages do tupi — teste body_preserves_multiple_system_messages_in_order continua válido com prefixo da instrução e ordem preservada
  - [x] **T6.2** requestId e sessionId estáveis por conversa, derivados de seed da sessão do tupi; conversationId no envelope — duas chamadas da mesma sessão compartilham sessionId e mudam de request só por turno; teste com sessão nova gera novo seed
  - [x] **T6.3** Sentinela skip_thought_signature_validator passa a ser fallback: store de signature por sessão/modelo e injeção só quando não houver assinatura — teste com assinatura em cache não emite a sentinela; sem cache emite
- [x] **T7** Versão/CL dinâmica, gate mínimo e modelos com cache + sufixo
  - UA congelado em 2.24.0/cl=1000335682 (antigravity.rs:25-28); list_models sem cache e fallback fixo (antigravity.rs:750-793); resolve_api_model não anexa sufixo (antigravity.rs:90-110). Referência: getAntigravityUserAgent + ANTIGRAVITY_HUB_CHANGELISTS + ANTIGRAVITY_MINIMUM_VERSION (2.23.0).
  - Nuance: Se o manifest falhar, usar 2.24.0/cl=1000335682 e não bloquear o boot.
  - Nuance: Sufixo ':antigravity' é hipótese: validar em teste real antes de ligar por padrão.
  - [x] **T7.1** Resolver versão do Hub em runtime (manifest com timeout curto) e montar o UA com CL; recusar versão < 2.23.0 — teste com manifest mockado gera UA com versão e cl corretos; falha de rede cai no fallback em menos de 500ms
  - [x] **T7.2** Cachear fetchAvailableModels (~1h) e filtrar SUPPORTED_MODELS pelos modelos realmente retornados — duas chamadas de list_models em sequência geram um só request HTTP; conta sem entitlement não vê modelo bloqueado na lista
  - [x] **T7.3** Avaliar e, se confirmado em teste real, anexar ':antigravity' ao id do modelo — resolve_api_model continua estável para os ids sem sufixo e teste documenta o resultado do experimento antes de habilitar
- [x] **T8** Contas: import por cópia, rotação e cooldown
  - from_opencode_json_str pega só accounts[activeIndex] com access_token vazio e expires_at 0 (antigravity.rs:425-456,498-516), compartilhando o refresh token com o plugin aberto. Referência: accounts.ts com N contas, rotação e quota.
  - [x] **T8.1** Importar todas as contas habilitadas do arquivo do plugin por cópia para o store do tupi (sem escrever no arquivo alheio) — importação de arquivo com 3 contas cria 3 registros locais e não modifica o arquivo de origem (hash antes/depois)
  - [x] **T8.2** Seleção de conta por rotação, pulando contas em cooldown ou sem entitlement — teste com 2 contas, primeira em cooldown, seleciona a segunda; com todas em cooldown devolve erro de quota agregado
  - [x] **T8.3** Refresh só da conta selecionada, com lock, sem refresh concorrente duplicado — duas tasks concorrentes disparam um único refresh e ambas usam o token novo
- [x] **T9** Hardening de schema de ferramentas e pairing
  - clean_gemini_schema só remove chaves não suportadas (antigravity.rs:112-202); ferramenta sem properties ou tool-result órfão gera INVALID_ARGUMENT em série. Referência: EMPTY_SCHEMA_PLACEHOLDER_*, injectParameterSignatures, fixToolResponseGrouping, validateAndFixClaudeToolPairing.
  - [x] **T9.1** Placeholder para schema vazio/objeto sem properties e normalização de functionDeclarations — tool com schema {} produz declaração válida e teste cobre array sem items
  - [x] **T9.2** Agrupar tool responses e remover tool result órfão antes de enviar — teste com 2 tool results do mesmo turno gera um único turno user com ambas as parts; órfão não quebra o request
  - [x] **T9.3** Pairing de functionCall/functionResponse por id inválido é corrigido em vez de propagado — teste com id divergente produz par válido e request serializável
- [ ] **T10** Observabilidade e guarda de regressão do fingerprint
  - Alinhamento não pode regredir silenciosamente: headers, escopos, UA e envelope viram contrato testado.
  - [x] **T10.1** Teste-contrato do conjunto de headers de generate/loadCodeAssist/onboardUser (UA-only fora do onboard) — teste falha se alguém adicionar X-Goog-Api-Client ou Client-Metadata em generate
  - [ ] **T10.2** Contadores de 429/cooldown expostos em telemetria local e no /status do provider — após 2 respostas 429 o status mostra contagem e conta em cooldown
