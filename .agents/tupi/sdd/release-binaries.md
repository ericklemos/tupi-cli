<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Fluxo de release de binários via tags (GitHub Actions) (`release-binaries`)

**Status:** closed

## Objetivo

Publicar binários do `tupi` em GitHub Releases quando uma tag `vX.Y.Z` for criada, compilando em runners nativos para exatamente três alvos — Linux genérico x86_64 (`x86_64-unknown-linux-gnu`), Windows x64 (`x86_64-pc-windows-msvc`) e macOS Apple Silicon (`aarch64-apple-darwin`) — empacotados com `SHA256SUMS` por um workflow novo em `.github/workflows/release.yml`.

## Restrições

- Escopo de arquiteturas fechado pelo usuário em 3 alvos: Linux genérico x86_64, Windows x86_64 e macOS Apple Silicon (aarch64). Sem musl, sem i686, sem armv7, sem Windows ARM64, sem macOS Intel.
- Nada de `.github/` existe hoje: o workflow parte do zero; não há CI para reutilizar.
- Sem segredos de runtime: o build não usa `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY` nem `.env`.
- Não commitar artefatos no repositório; tudo vai para a Release.
- Build com `--release --locked` e toolchain fixada.
- Sem assinatura/notarização (macOS sai sem notarização; Gatekeeper exige bypass do usuário).
- Só binário compactado + `SHA256SUMS`; sem instaladores (.msi/.deb/AppImage/Homebrew) nesta versão.
- Gate de publicação = build da matriz + empacotamento; não se publica se algum alvo falhar (`fail-fast: false`, job de release só roda com todos verdes).
- Binário `tupi --version` fica FORA do escopo desta spec (parse manual em `crates/tupi/src/main.rs:465`); registrar como follow-up.
- Pré-requisito conhecido: o workspace não compila hoje (erros em `crates/tupi-providers/src/antigravity.rs`); o pipeline falhará até isso ser corrigido, e corrigir está fora do escopo.

## Nuances

- Alvos definitivos: `x86_64-unknown-linux-gnu` (runner `ubuntu-22.04`, glibc 2.35 para compatibilidade ampla), `x86_64-pc-windows-msvc` (runner `windows-latest`, nativo, sem cross), `aarch64-apple-darwin` (runner `macos-14` ou superior, nativo Apple Silicon). Nenhum alvo é cross-compilado: todos são construídos em runner nativo da arquitetura, o que elimina toolchain C cruzada para `tree-sitter*`.
- `x86_64-unknown-linux-gnu` é o 'Linux genérico x64': depende de glibc >= 2.35 no destino; documentar isso no README e não prometer distros muito antigas.
- macOS Apple Silicon é `aarch64-apple-darwin` e NÃO roda em Macs Intel; não publicar artefato macOS Intel.
- Workspace com 9 crates; binário único `tupi` (`crates/tupi/Cargo.toml:8`), versão herdada de `[workspace.package] version` (`Cargo.toml:14`, hoje `0.1.0`).
- Dependências nativas compiladas no próprio runner: `tree-sitter`, `tree-sitter-rust`, `tree-sitter-kotlin-ng` (C via `cc`) e `reqwest` com `rustls` (`Cargo.toml:33`) — em runner nativo x64 é caminho já suportado; em Apple Silicon o `cc` do Xcode resolve.
- `crates/tupi-tools/src/proc.rs:106-127` tem variantes `cfg(unix)`/`cfg(not(unix))` para `isolate`/`kill_tree` (`libc::kill` só no unix), então o alvo Windows é buildável sem patch.
- Rótulo de release: `sha256-sum` via `sha256sum` no Linux e `shasum -a 256` no macOS; gerar `SHA256SUMS` agregado em job Linux após baixar todos os artefatos.
- Nome do artefato: `tupi-v<versão>-<target>.tar.gz` para linux/mac (bit +x preservado) e `tupi-v<versão>-<target>.zip` para windows, contendo o binário `tupi`(`.exe`) + `README.md` + `LICENSE`.
- Decisão de versão: o job de guarda FALHA quando `vX.Y.Z` da tag difere de `[workspace.package] version`; a mensagem deve mandar fazer o bump no `Cargo.toml` (nenhuma sobrescrita silenciosa de versão no build).
- Tag com sufixo (`v1.0.0-rc.1`, `-beta`) produz Release marcada `prerelease: true`; tag fora do padrão `vX.Y.Z[-sufixo]` aborta o workflow.
- Release exige `permissions: contents: write` e usa `GITHUB_TOKEN`; `concurrency` por tag evita publicação duplicada e `gh release`/`action-gh-release` precisa ser idempotente em re-run.
- Risco residual: sem assinatura, o `.exe` dispara SmartScreen e o binário macOS precisa de `xattr -d com.apple.quarantine`; documentar no README em vez de resolver.

## Tasks

- [ ] **T1** Esqueleto do workflow de release em .github/workflows/release.yml
  - Criar `.github/workflows/release.yml` do zero: `on.push.tags: ["v*.*.*"]`, `workflow_dispatch` para ensaio manual (com input de tag), `permissions: contents: write`, `concurrency: group: release-${{ github.ref }}, cancel-in-progress: false`, e `env` com `CARGO_TERM_COLOR: always`, `CARGO_INCREMENTAL: 0`.
  - [ ] **T1.1** Trigger por tag e permissões — YAML válido; push de tag `v0.1.0` dispara o workflow; push em branch não dispara.
  - [ ] **T1.2** Concurrency e rejeição de tag inválida — Duas execuções para a mesma tag ficam serializadas; job de validação aborta em tag fora de `vX.Y.Z[-sufixo]`.
- [ ] **T2** Job `validate`: tag semver x versão do workspace
  - Job `validate` em `ubuntu-22.04`: checar padrão da tag via shell/regex, extrair `[workspace.package] version` do `Cargo.toml` raiz (`Cargo.toml:14`) e comparar com a tag. Em divergência, falhar com mensagem explícita pedindo bump do `Cargo.toml`. Expor `version` e `prerelease` como `outputs` consumidos pela matriz e pela release.
  - [ ] **T2.1** Guarda de versão tag x workspace — Tag `v0.1.0` casa com workspace `0.1.0` e prossegue; tag `v1.0.0` com workspace `0.1.0` falha citando o bump; `outputs.version` e `outputs.prerelease` preenchidos.
  - [ ] **T2.2** Classificação de pré-release — `v1.0.0-rc.1` gera `prerelease=true`; `v1.0.0` gera `false`.
- [ ] **T4** Matriz de build: linux x64, windows x64, mac arm64
  - Matriz exata de 3 entradas, todas nativas, sem cross: `{os: ubuntu-22.04, target: x86_64-unknown-linux-gnu, ext: tar.gz}`, `{os: windows-latest, target: x86_64-pc-windows-msvc, ext: zip}`, `{os: macos-14, target: aarch64-apple-darwin, ext: tar.gz}`. `fail-fast: false`, `timeout-minutes: 60`. Passos: checkout (com `fetch-depth: 1`), `dtolnay/rust-toolchain@stable` com `targets: <target>` (necessário só para nomear o diretório de saída, já que o host é nativo), `Swatinem/rust-cache` com `key: ${{ matrix.target }}`, `cargo build -p tupi --release --locked --target ${{ matrix.target }}`.
  - [ ] **T4.1** Matriz de 3 alvos nativos — Três jobs verdes; cada um produz `target/<target>/release/tupi` (Linux/macOS) ou `tupi.exe` (Windows).
  - [ ] **T4.2** macOS Apple Silicon sem cross — macos-14 roda preparado e produz binário arm64 (`file` reporta `Mach-O 64-bit arm64`); nenhum passo tentasse cross para Intel.
  - [ ] **T4.3** Cache e tempo de execução — Cache por target ativo; build dentro do timeout em execução limpa.
  - [ ] **T4.4** Gate de qualidade mínimo (sem suíte, que hoje está vermelha) — `cargo test --workspace --locked` fora do gate; o gate é o próprio `cargo build --release` de cada alvo.
- [ ] **T5** Empacotamento e artefatos intermediários
  - Por alvo, empacotar `tupi`(`.exe`) + `README.md` + `LICENSE` em `tupi-v<version>-<target>.tar.gz` (unix, com bit +x) ou `.zip` (windows) usando `tar`/`shasum` e `Compress-Archive`/".  Upload com `actions/upload-artifact` (`name: dist-<target>`, `if-no-files-found: error`, retenção curta).
  - [ ] **T5.1** Empacotamento por plataforma — Nome segue `tupi-v<version>-<target>.<ext>`; `.tar.gz` preserva executável; `.zip` abre com `tupi.exe` na raiz.
  - [ ] **T5.2** Artefato intermediário por alvo — Cada job publica um `dist-<target>` contendo exatamente um arquivo compactado.
- [ ] **T6** Publicação da Release com SHA256SUMS
  - Job `release` em `ubuntu-22.04` com `needs: [validate, build]`, baixando todos os `dist-*` (`merge-multiple: true`), gerando `SHA256SUMS` com `sha256sum *` e criando/atualizando a Release da tag com `gh release create --verify-tag` (ou `softprops/action-gh-release`, com `generate_release_notes: true`, `prerelease` vindo de T2.2). Re-run precisa sobrescrever assets (`--clobber`) sem duplicar.
  - [ ] **T6.1** Publicação idempotente da Release — Release da tag tem exatamente 3 arquivos + `SHA256SUMS`; `sha256sum -c SHA256SUMS` passa em diretório baixado manualmente.
  - [ ] **T6.2** Notas e pré-release — Notas geradas automaticamente; flag prerelease coerente com `outputs.prerelease`.
- [ ] **T7** Documentação de download e proteção de plataforma
  - README ganha seção de instalação por plataforma: alvos publicados, nomes dos artefatos, verificação com `sha256sum -c`, aviso de glibc >= 2.35 no Linux, aviso de Gatekeeper (`xattr -d com.apple.quarantine`) no macOS não notarizado e SmartScreen no Windows. Registrar o follow-up `tupi --version` (`crates/tupi/src/main.rs:465`) e o bloqueio conhecido de compilação em `crates/tupi-providers/src/antigravity.rs`.
  - [ ] **T7.1** Documentação de download no README — README lista os 3 alvos, formato de nome, comando de verificação de checksum e os três avisos de segurança do SO.
  - [ ] **T7.2** Registro de riscos e follow-ups — Bloqueio de compilação e follow-up de `--version` registrados na spec/README, sem alteração de código nesta iniciativa.
