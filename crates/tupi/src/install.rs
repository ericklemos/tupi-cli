//! Suporte ao subcomando `tupi install`: descobrir o diretório do executável em
//! execução e declarar esse diretório no PATH do usuário.
//!
//! Tudo aqui é puro e injetável (`get_env`, `exists`, `home`) para que os testes
//! nunca toquem no perfil real do usuário.

use std::fmt;
use std::path::{Path, PathBuf};

/// Falhas ao resolver o alvo de instalação.
#[derive(Debug)]
pub enum InstallError {
    /// O caminho do executável não pôde ser resolvido (inexistente, sem permissão).
    Canonicalize {
        /// Caminho informado, antes da resolução.
        path: PathBuf,
        /// Erro devolvido pelo sistema de arquivos.
        source: std::io::Error,
    },
    /// O caminho resolvido não tem diretório pai para declarar no PATH.
    NoParent {
        /// Caminho já canonicalizado.
        path: PathBuf,
    },
    /// O arquivo de inicialização não pôde ser lido, escrito ou criado.
    Io {
        /// Caminho do arquivo ou do diretório que falhou.
        path: PathBuf,
        /// Erro devolvido pelo sistema de arquivos.
        source: std::io::Error,
    },
    /// O arquivo de inicialização não é UTF-8 válido.
    NotUtf8 {
        /// Caminho do arquivo recusado.
        path: PathBuf,
    },
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::Canonicalize { path, source } => write!(
                f,
                "não foi possível resolver o caminho do executável {}: {source}",
                path.display()
            ),
            InstallError::NoParent { path } => write!(
                f,
                "o executável {} não tem diretório pai para declarar no PATH",
                path.display()
            ),
            InstallError::Io { path, source } => write!(
                f,
                "não foi possível acessar o arquivo de inicialização {}: {source}",
                path.display()
            ),
            InstallError::NotUtf8 { path } => write!(
                f,
                "o arquivo de inicialização {} não está em UTF-8",
                path.display()
            ),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InstallError::Canonicalize { source, .. } => Some(source),
            InstallError::Io { source, .. } => Some(source),
            InstallError::NoParent { .. } | InstallError::NotUtf8 { .. } => None,
        }
    }
}

/// Diretório a declarar no PATH: o pai do executável canonicalizado.
///
/// Canonicalizar resolve links simbólicos, então o diretório declarado é o do
/// arquivo real — funciona tanto em `target/release` quanto em `~/.cargo/bin`
/// sem copiar o binário.
pub fn bin_dir(exe: &Path) -> Result<PathBuf, InstallError> {
    let resolved = exe
        .canonicalize()
        .map_err(|source| InstallError::Canonicalize {
            path: exe.to_path_buf(),
            source,
        })?;

    resolved
        .parent()
        .map(Path::to_path_buf)
        .ok_or(InstallError::NoParent { path: resolved })
}

/// Shell do usuário, que decide tanto o arquivo de inicialização quanto a
/// sintaxe do bloco declarado no PATH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// `~/.zshrc`.
    Zsh,
    /// `~/.bashrc`.
    Bash,
    /// `~/.config/fish/config.fish`.
    Fish,
    /// `~/.profile`, para sh e POSIX genéricos.
    Posix,
    /// Perfil do PowerShell no Windows.
    PowerShell,
}

impl Shell {
    /// Nome canônico do shell, como aparece em `--shell` e no relatório.
    pub fn as_str(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Posix => "posix",
            Shell::PowerShell => "powershell",
        }
    }

    /// Interpreta um nome de shell; `None` para desconhecido (inclusive `auto`).
    pub fn from_name(name: &str) -> Option<Shell> {
        match name.trim().to_ascii_lowercase().as_str() {
            "zsh" => Some(Shell::Zsh),
            "bash" => Some(Shell::Bash),
            "fish" => Some(Shell::Fish),
            "sh" | "dash" | "ash" | "ksh" | "posix" => Some(Shell::Posix),
            "pwsh" | "powershell" | "powershell.exe" => Some(Shell::PowerShell),
            _ => None,
        }
    }
}

/// Descobre o shell do usuário: primeiro pelo basename de `$SHELL`, depois pela
/// existência de `~/.zshrc`, `~/.bashrc` e `~/.profile`. Sem nenhuma pista, cai
/// em [`Shell::Posix`]. Nada é lido do sistema além do que as closures devolvem.
pub fn detect_shell<G, E>(get_env: G, exists: E) -> Shell
where
    G: Fn(&str) -> Option<String>,
    E: Fn(&Path) -> bool,
{
    if let Some(shell) = get_env("SHELL").as_deref().and_then(shell_from_path) {
        return shell;
    }

    if let Some(home) = home_from(&get_env) {
        let candidates = [
            (home.join(".zshrc"), Shell::Zsh),
            (home.join(".bashrc"), Shell::Bash),
            (home.join(".profile"), Shell::Posix),
        ];
        for (path, shell) in candidates {
            if exists(&path) {
                return shell;
            }
        }
    }

    Shell::Posix
}

/// Extrai o shell do caminho completo recebido em `$SHELL`, aceitando sufixos
/// como `zsh-5.9` e a extensão `.exe` do Windows.
fn shell_from_path(value: &str) -> Option<Shell> {
    let name = value
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .trim()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    let name: String = name
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '-')
        .collect();

    Shell::from_name(name.trim_end_matches('-'))
}

/// Diretório pessoal segundo as variáveis disponíveis (`HOME` no Unix,
/// `USERPROFILE` no Windows).
fn home_from<G>(get_env: &G) -> Option<PathBuf>
where
    G: Fn(&str) -> Option<String>,
{
    get_env("HOME")
        .or_else(|| get_env("USERPROFILE"))
        .filter(|home| !home.trim().is_empty())
        .map(PathBuf::from)
}

/// Arquivo de inicialização que o `shell` lê ao abrir uma sessão nova.
///
/// `home` é injetado para que os testes nunca toquem no perfil real do
/// usuário.
pub fn rc_path(shell: Shell, home: &Path) -> PathBuf {
    match shell {
        Shell::Zsh => home.join(".zshrc"),
        Shell::Bash => home.join(".bashrc"),
        Shell::Fish => home.join(".config").join("fish").join("config.fish"),
        Shell::Posix => home.join(".profile"),
        Shell::PowerShell => home
            .join("Documents")
            .join("PowerShell")
            .join("Microsoft.PowerShell_profile.ps1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn the_bin_directory_is_the_parent_of_the_executable() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("tupi");
        std::fs::write(&exe, b"binario").unwrap();

        let expected = dir.path().canonicalize().unwrap();
        assert_eq!(bin_dir(&exe).unwrap(), expected);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_executable_resolves_to_the_real_directory() {
        let real = tempdir().unwrap();
        let exe = real.path().join("tupi");
        std::fs::write(&exe, b"binario").unwrap();

        let link_dir = tempdir().unwrap();
        let link = link_dir.path().join("tupi");
        std::os::unix::fs::symlink(&exe, &link).unwrap();

        let expected = real.path().canonicalize().unwrap();
        assert_eq!(bin_dir(&link).unwrap(), expected);
    }

    #[test]
    fn a_missing_executable_names_the_path_in_the_error() {
        let dir = tempdir().unwrap();
        let exe = dir.path().join("nao-existe");

        let error = bin_dir(&exe).unwrap_err();
        assert!(matches!(error, InstallError::Canonicalize { .. }));
        assert!(
            error.to_string().contains("nao-existe"),
            "mensagem sem o caminho: {error}"
        );
    }

    #[test]
    fn a_root_path_has_no_directory_to_declare() {
        let root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };

        let error = bin_dir(&root).unwrap_err();
        assert!(matches!(error, InstallError::NoParent { .. }));
    }

    fn env_map<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    fn no_files(_: &Path) -> bool {
        false
    }

    fn files(names: &'static [&'static str]) -> impl Fn(&Path) -> bool {
        move |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| names.contains(&name))
        }
    }

    #[test]
    fn the_shell_variable_wins_by_its_basename() {
        let cases = [
            ("/usr/bin/zsh", Shell::Zsh),
            ("/bin/bash", Shell::Bash),
            ("/usr/local/bin/fish", Shell::Fish),
            ("/usr/local/bin/zsh-5.9", Shell::Zsh),
            ("/bin/sh", Shell::Posix),
            ("/bin/dash", Shell::Posix),
        ];

        for (value, expected) in cases {
            let env = [("SHELL", value)];
            assert_eq!(detect_shell(env_map(&env), no_files), expected, "{value}");
        }
    }

    #[test]
    fn the_windows_powershell_is_recognised_from_the_shell_variable() {
        let shell = detect_shell(
            env_map(&[("SHELL", "C:\\Program Files\\PowerShell\\7\\pwsh.exe")]),
            no_files,
        );

        assert_eq!(shell, Shell::PowerShell);
    }

    #[test]
    fn the_shell_is_guessed_from_the_startup_files() {
        let home = env_map(&[("HOME", "/home/ana")]);

        assert_eq!(detect_shell(home, files(&[".zshrc"])), Shell::Zsh);
        assert_eq!(
            detect_shell(env_map(&[("HOME", "/home/ana")]), files(&[".bashrc"])),
            Shell::Bash
        );
        assert_eq!(
            detect_shell(env_map(&[("HOME", "/home/ana")]), files(&[".profile"])),
            Shell::Posix
        );
    }

    #[test]
    fn the_file_fallback_uses_the_home_directory() {
        let seen = std::cell::RefCell::new(Vec::new());
        let shell = detect_shell(env_map(&[("HOME", "/home/ana")]), |path: &Path| {
            seen.borrow_mut().push(path.to_path_buf());
            path.file_name().and_then(|name| name.to_str()) == Some(".bashrc")
        });

        assert_eq!(shell, Shell::Bash);
        assert_eq!(seen.borrow()[0], Path::new("/home/ana").join(".zshrc"));
    }

    #[test]
    fn the_profile_variable_replaces_home_when_it_is_absent() {
        let shell = detect_shell(
            env_map(&[("USERPROFILE", "C:\\Users\\ana")]),
            files(&[".profile"]),
        );

        assert_eq!(shell, Shell::Posix);
    }

    #[test]
    fn an_unknown_or_empty_shell_variable_falls_back_to_the_files() {
        let tcsh = env_map(&[("SHELL", "/bin/tcsh"), ("HOME", "/home/ana")]);
        assert_eq!(detect_shell(tcsh, files(&[".bashrc"])), Shell::Bash);

        let empty = env_map(&[("SHELL", ""), ("HOME", "/home/ana")]);
        assert_eq!(detect_shell(empty, files(&[".profile"])), Shell::Posix);
    }

    #[test]
    fn without_any_clue_the_posix_default_applies() {
        assert_eq!(detect_shell(env_map(&[]), no_files), Shell::Posix);
        assert_eq!(
            detect_shell(env_map(&[("HOME", "/home/ana")]), no_files),
            Shell::Posix
        );
    }

    #[test]
    fn the_unix_startup_files_live_in_the_home_directory() {
        let home = Path::new("/home/ana");

        assert_eq!(rc_path(Shell::Zsh, home), home.join(".zshrc"));
        assert_eq!(rc_path(Shell::Bash, home), home.join(".bashrc"));
        assert_eq!(rc_path(Shell::Posix, home), home.join(".profile"));
    }

    #[test]
    fn the_fish_configuration_sits_under_the_config_directory() {
        let home = Path::new("/home/ana");

        assert_eq!(
            rc_path(Shell::Fish, home),
            Path::new("/home/ana/.config/fish/config.fish")
        );
    }

    #[test]
    fn the_power_shell_profile_sits_under_documents() {
        let home = Path::new("C:\\Users\\ana");

        assert_eq!(
            rc_path(Shell::PowerShell, home),
            home.join("Documents")
                .join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1")
        );
    }

    #[test]
    fn every_unix_rc_path_stays_inside_the_injected_home() {
        let home = PathBuf::from("/tmp/home-injetado");

        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Posix] {
            assert!(rc_path(shell, &home).starts_with(&home), "{shell:?}");
        }
    }

    #[test]
    fn the_markers_are_the_fixed_contract_strings() {
        assert_eq!(MARKER_START, "# >>> tupi install >>>");
        assert_eq!(MARKER_END, "# <<< tupi install <<<");
    }

    #[test]
    fn the_posix_block_exports_the_directory_ahead_of_the_current_path() {
        assert_eq!(
            block(Shell::Posix, Path::new("/home/ana/bin")),
            "# >>> tupi install >>>\nexport PATH=\"/home/ana/bin:$PATH\"\n# <<< tupi install <<<\n"
        );
    }

    #[test]
    fn bash_and_zsh_share_the_posix_export() {
        let dir = Path::new("/home/ana/bin");

        assert_eq!(block(Shell::Zsh, dir), block(Shell::Posix, dir));
        assert_eq!(block(Shell::Bash, dir), block(Shell::Posix, dir));
    }

    #[test]
    fn the_fish_block_uses_set_gx_path() {
        let block = block(Shell::Fish, Path::new("/home/ana/.cargo/bin"));

        assert!(
            block.contains("set -gx PATH \"/home/ana/.cargo/bin\" $PATH"),
            "{block}"
        );
        assert!(!block.contains("export"));
    }

    #[test]
    fn the_power_shell_block_separates_entries_with_a_semicolon() {
        let block = block(Shell::PowerShell, Path::new("C:\\Users\\ana\\bin"));
        let lines: Vec<&str> = block.lines().collect();

        assert_eq!(lines.len(), 3, "{block}");
        assert_eq!(lines[0], MARKER_START);
        assert_eq!(lines[1], "$env:Path = \"C:\\Users\\ana\\bin;$env:Path\"");
        assert_eq!(lines[2], MARKER_END);
    }

    #[test]
    fn every_block_carries_exactly_one_pair_of_markers() {
        let dir = Path::new("/opt/tupi");

        for shell in [
            Shell::Zsh,
            Shell::Bash,
            Shell::Fish,
            Shell::Posix,
            Shell::PowerShell,
        ] {
            let block = block(shell, dir);

            assert_eq!(block.matches(MARKER_START).count(), 1, "{shell:?}");
            assert_eq!(block.matches(MARKER_END).count(), 1, "{shell:?}");
            assert!(block.starts_with(MARKER_START), "{shell:?}");
            assert!(block.trim_end().ends_with(MARKER_END), "{shell:?}");
            assert!(block.ends_with('\n'), "{shell:?}");
        }
    }

    #[test]
    fn a_directory_with_spaces_stays_quoted() {
        let dir = Path::new("/home/ana/Meus Programas/tupi");

        for shell in [
            Shell::Zsh,
            Shell::Bash,
            Shell::Fish,
            Shell::Posix,
            Shell::PowerShell,
        ] {
            let block = block(shell, dir);

            assert!(
                block.contains("\"/home/ana/Meus Programas/tupi"),
                "diretório sem aspas em {shell:?}: {block}"
            );
        }
    }
}

/// Marca que abre o bloco gerado por `tupi install`.
pub const MARKER_START: &str = "# >>> tupi install >>>";

/// Marca que fecha o bloco gerado por `tupi install`.
pub const MARKER_END: &str = "# <<< tupi install <<<";

/// Bloco marcado que declara `dir` no PATH de `shell`, pronto para ser escrito
/// no arquivo de inicialização do usuário.
///
/// O bloco tem sempre três linhas — marca de abertura, declaração e marca de
/// fechamento — e termina com `\n`, para que o trecho entre as marcas possa ser
/// substituído inteiro numa segunda execução sem nunca duplicar o bloco.
pub fn block(shell: Shell, dir: &Path) -> String {
    let dir = dir.display();
    let line = match shell {
        Shell::Zsh | Shell::Bash | Shell::Posix => format!("export PATH=\"{dir}:$PATH\""),
        Shell::Fish => format!("set -gx PATH \"{dir}\" $PATH"),
        Shell::PowerShell => format!("$env:Path = \"{dir};$env:Path\""),
    };

    format!("{MARKER_START}\n{line}\n{MARKER_END}\n")
}

/// O que aconteceu com o arquivo de inicialização do shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// O arquivo não existia e foi criado com o bloco.
    Created,
    /// O arquivo existia e foi alterado (bloco anexado ou substituído).
    Updated,
    /// O arquivo já continha exatamente este bloco; nada foi escrito.
    AlreadyPresent,
}

/// Declara o bloco no arquivo `path`, de forma idempotente.
///
/// Arquivo ausente conta como vazio (e é criado). Se o arquivo já contém os
/// marcadores, só o trecho entre eles é trocado, na mesma posição; as demais
/// linhas são preservadas byte a byte. Sem marcadores, o bloco é anexado ao
/// final. Diretórios pais são criados quando faltam (caso do `fish`).
pub fn apply(path: &Path, block: &str) -> Result<Action, InstallError> {
    let existing = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            // Caminho que não é um arquivo regular (diretório, por exemplo)
            // chega aqui: o erro cita o caminho e nada é escrito.
            return Err(InstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let existed = existing.is_some();
    let content = match existing {
        Some(bytes) => String::from_utf8(bytes).map_err(|_| InstallError::NotUtf8 {
            path: path.to_path_buf(),
        })?,
        None => String::new(),
    };

    let Some(next) = splice(&content, block) else {
        return Ok(Action::AlreadyPresent);
    };

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| InstallError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    if existed {
        // Sonda de escrita sem `truncate`: o arquivo somente leitura é recusado
        // antes de qualquer alteração, preservando o conteúdo original.
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|source| InstallError::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }

    if let Err(source) = std::fs::write(path, next.as_bytes()) {
        if existed {
            // Escrita parcial não deixa o perfil truncado: restaura o original.
            let _ = std::fs::write(path, content.as_bytes());
        }
        return Err(InstallError::Io {
            path: path.to_path_buf(),
            source,
        });
    }

    Ok(if existed {
        Action::Updated
    } else {
        Action::Created
    })
}

/// Conteúdo resultante da declaração do bloco, ou `None` se já está aplicado.
fn splice(content: &str, block: &str) -> Option<String> {
    let Some(start) = content.find(MARKER_START) else {
        return Some(appended(content, block));
    };

    // Fim do trecho ocupado pelo bloco atual: até o fim da linha do marcador
    // final, ou até o fim do arquivo se o marcador final sumiu.
    let end = match content[start..].find(MARKER_END) {
        Some(offset) => {
            let marker_end = start + offset + MARKER_END.len();
            let rest = &content[marker_end..];
            if rest.starts_with("\r\n") {
                marker_end + 2
            } else if rest.starts_with('\n') {
                marker_end + 1
            } else {
                marker_end
            }
        }
        None => content.len(),
    };

    let next = format!("{}{}{}", &content[..start], block, &content[end..]);

    (next != content).then_some(next)
}

/// Conteúdo com o bloco anexado ao final, separado por exatamente um `\n`.
fn appended(content: &str, block: &str) -> String {
    if content.is_empty() {
        return block.to_string();
    }

    let mut next = String::with_capacity(content.len() + block.len() + 1);
    next.push_str(content);
    if !content.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(block);
    next
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    fn zsh_block(dir: &str) -> String {
        block(Shell::Zsh, Path::new(dir))
    }

    #[test]
    fn an_existing_block_is_replaced_in_place() {
        let before = "# perfil do usuário\nexport EDITOR=vim\n";
        let after = "\nalias ll='ls -l'\n";
        let content = format!("{before}{}{after}", zsh_block("/old/bin"));

        let next = splice(&content, &zsh_block("/new/bin")).unwrap();

        assert_eq!(next, format!("{before}{}{after}", zsh_block("/new/bin")));
        assert!(next.starts_with(before), "prefixo alterado: {next:?}");
        assert!(next.ends_with(after), "sufixo alterado: {next:?}");
        assert_eq!(next.matches(MARKER_START).count(), 1);
        assert_eq!(next.matches("/old/bin").count(), 0);
    }

    #[test]
    fn the_replacement_keeps_the_position_of_the_block() {
        let content = format!("a\n{}\nb\n", zsh_block("/old/bin"));
        let next = splice(&content, &zsh_block("/new/bin")).unwrap();

        assert_eq!(next, format!("a\n{}\nb\n", zsh_block("/new/bin")));
    }

    #[test]
    fn an_identical_block_is_not_rewritten() {
        let content = format!("# topo\n{}", zsh_block("/opt/tupi"));

        assert_eq!(splice(&content, &zsh_block("/opt/tupi")), None);
    }

    #[test]
    fn a_missing_block_is_appended_after_a_single_newline() {
        let content = "export EDITOR=vim";

        let next = splice(content, &zsh_block("/opt/tupi")).unwrap();

        assert_eq!(
            next,
            format!("export EDITOR=vim\n{}", zsh_block("/opt/tupi"))
        );
    }

    #[test]
    fn a_file_ending_with_a_newline_gains_no_blank_line() {
        let content = "export EDITOR=vim\n";

        let next = splice(content, &zsh_block("/opt/tupi")).unwrap();

        assert_eq!(
            next,
            format!("export EDITOR=vim\n{}", zsh_block("/opt/tupi"))
        );
        assert!(!next.contains("\n\n"));
    }

    #[test]
    fn an_empty_file_receives_only_the_block() {
        assert_eq!(
            splice("", &zsh_block("/opt/tupi")),
            Some(zsh_block("/opt/tupi"))
        );
    }

    #[test]
    fn an_unterminated_block_is_replaced_to_the_end_of_the_file() {
        let content = format!("antes\n{}\nexport RUIM=1\n", MARKER_START);

        let next = splice(&content, &zsh_block("/opt/tupi")).unwrap();

        assert_eq!(next, format!("antes\n{}", zsh_block("/opt/tupi")));
    }

    fn read_text(path: &Path) -> String {
        String::from_utf8(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn the_second_run_with_the_same_directory_is_a_no_op() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        let block = zsh_block("/opt/tupi");

        assert!(matches!(apply(&path, &block).unwrap(), Action::Created));
        let first = std::fs::read(&path).unwrap();
        assert_eq!(first, block.as_bytes());

        assert!(matches!(
            apply(&path, &block).unwrap(),
            Action::AlreadyPresent
        ));
        assert_eq!(std::fs::read(&path).unwrap(), first);
        assert_eq!(read_text(&path).matches(MARKER_START).count(), 1);
    }

    #[test]
    fn a_second_run_over_an_existing_profile_keeps_the_content_untouched() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Bash, home.path());
        std::fs::write(&path, "export EDITOR=vim\n").unwrap();
        let block = zsh_block("/opt/tupi");

        assert!(matches!(apply(&path, &block).unwrap(), Action::Updated));
        let first = std::fs::read(&path).unwrap();
        assert_eq!(read_text(&path), format!("export EDITOR=vim\n{block}"));

        assert!(matches!(
            apply(&path, &block).unwrap(),
            Action::AlreadyPresent
        ));
        assert_eq!(std::fs::read(&path).unwrap(), first);
        let text = read_text(&path);
        assert_eq!(text.matches(MARKER_START).count(), 1);
        assert_eq!(text.matches(MARKER_END).count(), 1);
    }

    #[test]
    fn every_user_line_survives_the_single_appended_block() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        let own = "# meu perfil\nexport EDITOR=vim\n\nalias ll='ls -l'\nexport PAGER=less";
        std::fs::write(&path, own).unwrap();
        let block = zsh_block("/opt/tupi");

        assert!(matches!(apply(&path, &block).unwrap(), Action::Updated));

        let text = read_text(&path);
        // Nenhuma linha própria pode ser perdida ou reordenada.
        assert!(text.starts_with(own), "linhas alteradas: {text:?}");
        assert_eq!(text, format!("{own}\n{block}"));

        // O bloco aparece uma única vez.
        assert_eq!(text.matches(MARKER_START).count(), 1);
        assert_eq!(text.matches(MARKER_END).count(), 1);
        assert!(matches!(
            apply(&path, &block).unwrap(),
            Action::AlreadyPresent
        ));
        assert_eq!(read_text(&path), text);
    }

    #[test]
    fn a_replacement_keeps_every_user_line_and_a_single_block() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Bash, home.path());
        let before = "# topo do perfil\nexport EDITOR=vim\n";
        let after = "\nalias ll='ls -l'\nexport PAGER=less\n";
        let old = zsh_block("/usr/local/bin");
        std::fs::write(&path, format!("{before}{old}{after}")).unwrap();
        let block = zsh_block("/opt/tupi/bin");

        assert!(matches!(apply(&path, &block).unwrap(), Action::Updated));

        let text = read_text(&path);
        assert_eq!(text, format!("{before}{block}{after}"));
        assert_eq!(text.matches(MARKER_START).count(), 1);
        assert_eq!(text.matches(MARKER_END).count(), 1);
        assert!(!text.contains("/usr/local/bin"));
    }

    #[test]
    fn a_second_run_creates_the_fish_directory_only_when_missing() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Fish, home.path());
        let block = block(Shell::Fish, Path::new("/opt/tupi/bin"));

        assert!(matches!(apply(&path, &block).unwrap(), Action::Created));
        assert!(path.exists());
        let first = std::fs::read(&path).unwrap();

        assert!(matches!(
            apply(&path, &block).unwrap(),
            Action::AlreadyPresent
        ));
        assert_eq!(std::fs::read(&path).unwrap(), first);
    }
}

/// Erros de t2.3: bytes inválidos, diretório no lugar do arquivo e falta de
/// permissão precisam citar o caminho e nunca truncar o arquivo original.
#[cfg(test)]
mod error_tests {
    use super::*;

    fn zsh_block(dir: &str) -> String {
        block(Shell::Zsh, Path::new(dir))
    }

    #[test]
    fn invalid_bytes_are_refused_with_the_path_and_the_file_kept() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        let original = b"# perfil do usuario\n\xff\xfe\n".to_vec();
        std::fs::write(&path, &original).unwrap();

        let error = apply(&path, &zsh_block("/opt/tupi")).unwrap_err();

        assert!(matches!(error, InstallError::NotUtf8 { .. }));
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "mensagem sem o caminho: {error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn a_directory_in_place_of_the_profile_is_refused_with_the_path() {
        use std::error::Error as _;

        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        std::fs::create_dir_all(&path).unwrap();

        let error = apply(&path, &zsh_block("/opt/tupi")).unwrap_err();

        assert!(matches!(error, InstallError::Io { .. }));
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "mensagem sem o caminho: {error}"
        );
        assert!(
            error.source().is_some(),
            "sem a causa do sistema de arquivos"
        );
        assert!(path.is_dir(), "o diretório não deveria ter sido tocado");
    }

    #[test]
    fn a_blocked_parent_directory_names_the_path_and_writes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let parent = home.path().join("config");
        let original = b"nao sou um diretorio\n".to_vec();
        std::fs::write(&parent, &original).unwrap();
        let path = parent.join("config.fish");

        let error = apply(&path, &block(Shell::Fish, Path::new("/opt/tupi/bin"))).unwrap_err();

        let text = error.to_string();
        assert!(
            text.contains(&path.display().to_string())
                || text.contains(&parent.display().to_string()),
            "mensagem sem o caminho: {text}"
        );
        assert!(!path.exists(), "nada deveria ser criado");
        assert_eq!(std::fs::read(&parent).unwrap(), original);
    }

    /// Remove os bits de escrita e devolve `false` quando o processo os ignora
    /// (root): aí não existe falha de permissão a verificar.
    #[cfg(unix)]
    fn deny_write(path: &Path) -> bool {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).unwrap();
        std::fs::OpenOptions::new().write(true).open(path).is_err()
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_profile_is_refused_with_the_path_and_kept_whole() {
        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        let original = b"export EDITOR=vim\n".to_vec();
        std::fs::write(&path, &original).unwrap();

        if !deny_write(&path) {
            return; // root ignora o bit de escrita
        }

        let error = apply(&path, &zsh_block("/opt/tupi")).unwrap_err();

        assert!(matches!(error, InstallError::Io { .. }));
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "mensagem sem o caminho: {error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_profile_is_refused_with_the_path_and_kept_whole() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Fish, home.path());
        let original = b"set -gx EDITOR vim\n".to_vec();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &original).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        if std::fs::read(&path).is_ok() {
            // root ignora o modo do arquivo.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            return;
        }

        let error = apply(&path, &zsh_block("/opt/tupi")).unwrap_err();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(matches!(error, InstallError::Io { .. }));
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "mensagem sem o caminho: {error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

/// Opções de linha de comando do subcomando `tupi install`.
#[derive(Debug)]
pub struct Options {
    /// `--print`: mostra o que seria escrito e não toca em nenhum arquivo.
    pub print: bool,
    /// `--shell`: força o shell; ausente ou `auto` deixa a detecção automática.
    pub shell: Option<Shell>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            print: false,
            shell: None,
        }
    }
}

/// Interpreta as flags do subcomando; flag desconhecida é erro que a cita.
pub fn parse_options(args: &[String]) -> anyhow::Result<Options> {
    const USAGE: &str = "--print, --shell <auto|bash|zsh|fish|posix|powershell>";

    let mut options = Options::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--print" => options.print = true,
            "--shell" => {
                let value = rest
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--shell exige um valor ({USAGE})"))?;
                options.shell = parse_shell(value)?;
            }
            other => match other.strip_prefix("--shell=") {
                Some(value) => options.shell = parse_shell(value)?,
                None => anyhow::bail!("flag desconhecida `{other}` ({USAGE})"),
            },
        }
    }
    Ok(options)
}

fn parse_shell(value: &str) -> anyhow::Result<Option<Shell>> {
    if value.trim().eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    Shell::from_name(value).map(Some).ok_or_else(|| {
        anyhow::anyhow!(
            "shell desconhecido `{value}` (use auto, bash, zsh, fish, posix ou powershell)"
        )
    })
}

/// O que o subcomando faria, resolvido apenas com leituras.
pub struct Plan {
    /// Shell cujo bloco marcado será escrito.
    pub shell: Shell,
    /// Diretório do executável, já canonicalizado.
    pub dir: std::path::PathBuf,
    /// Arquivo de inicialização do usuário.
    pub path: std::path::PathBuf,
    /// Ação que a escrita produziria (ou que já está em vigor).
    pub action: Action,
    /// O diretório já estava no `PATH` deste ambiente antes da escrita.
    pub in_path: bool,
}

impl Plan {
    /// Bloco marcado que representa este plano.
    pub fn block(&self) -> String {
        block(self.shell, &self.dir)
    }
}

impl Action {
    /// Rótulo estável da ação no relatório.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Created => "created",
            Action::Updated => "updated",
            Action::AlreadyPresent => "already-present",
        }
    }
}

/// Resolve shell, diretório e arquivo de inicialização sem escrever nada.
pub fn plan<G, E>(
    options: &Options,
    exe: &std::path::Path,
    get_env: G,
    exists: E,
) -> anyhow::Result<Plan>
where
    G: Fn(&str) -> Option<String>,
    E: Fn(&std::path::Path) -> bool,
{
    let dir = bin_dir(exe)?;
    let shell = match options.shell {
        Some(shell) => shell,
        None => detect_shell(&get_env, &exists),
    };
    let home = home_from(&get_env).ok_or_else(|| {
        anyhow::anyhow!("HOME (ou USERPROFILE) não está definido: não há onde declarar o PATH")
    })?;
    let path = rc_path(shell, &home);
    let action = probe(&path, &block(shell, &dir))?;
    // O bloco é gravado mesmo quando o diretório já está no `PATH` deste
    // ambiente: o efeito só vale em shells novos, então a declaração no
    // arquivo de inicialização continua sendo a que persiste.
    let in_path = declared_in_path(&dir, shell, &get_env);
    Ok(Plan {
        shell,
        dir,
        path,
        action,
        in_path,
    })
}

/// Ação que `apply` tomaria neste arquivo, sem escrever: apenas lê.
fn probe(path: &std::path::Path, block: &str) -> anyhow::Result<Action> {
    let content = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| InstallError::NotUtf8 {
            path: path.to_path_buf(),
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Action::Created),
        Err(source) => {
            return Err(InstallError::Io {
                path: path.to_path_buf(),
                source,
            }
            .into());
        }
    };

    Ok(match splice(&content, block) {
        None => Action::AlreadyPresent,
        Some(_) => Action::Updated,
    })
}

/// Relatório do `--print`: caminho, ação e bloco, sem escrever nada.
/// Prévia do `--print` no idioma resolvido por `output_lang`.
pub fn preview_in(plan: &Plan, lang: crate::lang::Lang) -> String {
    // O `PATH` do ambiente já conter o diretório não dispensa o bloco: o
    // efeito só vale em shells novos, então o dry run também avisa o fato.
    let note = if plan.in_path {
        match lang {
            crate::lang::Lang::PtBr => format!(
                "observação: {} já estava no PATH deste ambiente; o bloco seria gravado de qualquer forma\n",
                plan.dir.display()
            ),
            crate::lang::Lang::En => format!(
                "note: {} was already in this environment's PATH; the block would be written anyway\n",
                plan.dir.display()
            ),
        }
    } else {
        String::new()
    };

    match lang {
        crate::lang::Lang::PtBr => format!(
            "tupi install --print (dry run): nenhum arquivo foi criado ou alterado\n\
             shell: {}\n\
             diretório: {}\n\
             arquivo: {}\n\
             ação: {}\n\
             {note}\
             bloco que seria escrito neste arquivo:\n\
             {}",
            plan.shell.as_str(),
            plan.dir.display(),
            plan.path.display(),
            action_label(plan.action, lang),
            plan.block(),
        ),
        crate::lang::Lang::En => format!(
            "tupi install --print (dry run): no file was created or changed\n\
             shell: {}\n\
             directory: {}\n\
             file: {}\n\
             action: {}\n\
             {note}\
             block that would be written to this file:\n\
             {}",
            plan.shell.as_str(),
            plan.dir.display(),
            plan.path.display(),
            action_label(plan.action, lang),
            plan.block(),
        ),
    }
}

#[cfg(test)]
pub fn preview(plan: &Plan) -> String {
    preview_in(plan, crate::lang::Lang::En)
}

/// Relatório do comando depois de escrever o bloco.
/// Idioma do relatório: só o `lang` explícito de `ai.project.json`.
///
/// Ausente, ilegível ou inválido cai em inglês. `ProjectConfig::lang` não serve
/// aqui porque completa o campo ausente com o locale do sistema.
pub fn output_lang(root: &Path) -> crate::lang::Lang {
    crate::project_config::read_project(root)
        .ok()
        .and_then(|config| config.lang)
        .unwrap_or(crate::lang::Lang::En)
}

/// Relatório final no idioma resolvido por `output_lang`.
pub fn report_in(plan: &Plan, action: Action, lang: crate::lang::Lang) -> String {
    // Quando o diretório já está no `PATH` do ambiente, a ação sobre o arquivo
    // não muda (o bloco é gravado assim mesmo) e o relatório diz o fato.
    let note = if plan.in_path {
        match lang {
            crate::lang::Lang::PtBr => format!(
                "observação: {} já estava no PATH deste ambiente; o bloco é gravado de qualquer forma\n",
                plan.dir.display()
            ),
            crate::lang::Lang::En => format!(
                "note: {} was already in this environment's PATH; the block is written anyway\n",
                plan.dir.display()
            ),
        }
    } else {
        String::new()
    };

    match lang {
        crate::lang::Lang::PtBr => format!(
            "tupi install: diretório declarado no PATH\n\
             diretório: {}\n\
             arquivo: {}\n\
             ação: {}\n\
             {note}\
             reabra o shell (um terminal novo) para o PATH novo valer",
            plan.dir.display(),
            plan.path.display(),
            action_label(action, lang),
        ),
        crate::lang::Lang::En => format!(
            "tupi install: directory declared in PATH\n\
             directory: {}\n\
             file: {}\n\
             action: {}\n\
             {note}\
             restart your shell (open a new terminal) so the new PATH takes effect",
            plan.dir.display(),
            plan.path.display(),
            action_label(action, lang),
        ),
    }
}

/// Rótulo da ação no idioma pedido.
fn action_label(action: Action, lang: crate::lang::Lang) -> &'static str {
    match (lang, action) {
        (crate::lang::Lang::En, _) => action.as_str(),
        (crate::lang::Lang::PtBr, Action::Created) => "criado",
        (crate::lang::Lang::PtBr, Action::Updated) => "atualizado",
        (crate::lang::Lang::PtBr, Action::AlreadyPresent) => "já-presente",
    }
}

#[cfg(test)]
pub fn report(plan: &Plan, action: Action) -> String {
    report_in(plan, action, crate::lang::Lang::En)
}

/// Executa o subcomando: com `--print` só relata, sem escrever nada.
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let options = parse_options(args)?;
    // Idioma da saída: `lang` explícito do projeto, nunca o locale do sistema.
    let lang = std::env::current_dir()
        .map(|root| output_lang(&root))
        .unwrap_or(crate::lang::Lang::En);
    let exe = std::env::current_exe()?;
    let plan = plan(
        &options,
        &exe,
        |key| std::env::var(key).ok(),
        |path: &std::path::Path| path.exists(),
    )?;

    if options.print {
        print!("{}", preview_in(&plan, lang));
        return Ok(());
    }

    let action = apply(&plan.path, &plan.block())?;
    println!("{}", report_in(&plan, action, lang));
    Ok(())
}

#[cfg(test)]
mod print_tests {
    use super::*;

    fn home_env(home: &std::path::Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| (key == "HOME").then(|| home.display().to_string())
    }

    fn binary(dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    fn dry_run(shell: Shell, home: &std::path::Path, bin: &std::path::Path) -> Plan {
        let options = Options {
            print: true,
            shell: Some(shell),
        };
        plan(
            &options,
            &binary(bin),
            home_env(home),
            |path: &std::path::Path| path.exists(),
        )
        .unwrap()
    }

    #[test]
    fn the_dry_run_creates_no_file_and_prints_the_block_and_the_path() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = dry_run(Shell::Zsh, home.path(), bin.path());

        let out = preview(&plan);
        assert!(
            out.contains(&block(Shell::Zsh, &plan.dir)),
            "saída sem o bloco: {out}"
        );
        assert!(
            out.contains(&plan.path.display().to_string()),
            "saída sem o caminho: {out}"
        );
        assert!(out.contains("action: created"), "saída sem a ação: {out}");

        assert!(
            !plan.path.exists(),
            "o --print criou {}",
            plan.path.display()
        );
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "o --print escreveu em {}",
            home.path().display()
        );
    }

    #[test]
    fn the_dry_run_leaves_an_existing_profile_untouched() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let path = rc_path(Shell::Zsh, home.path());
        let original = "export EDITOR=vim\n";
        std::fs::write(&path, original).unwrap();

        let plan = dry_run(Shell::Zsh, home.path(), bin.path());
        let out = preview(&plan);

        assert!(out.contains("action: updated"), "saída sem a ação: {out}");
        assert!(out.contains(MARKER_START), "saída sem o bloco: {out}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn the_dry_run_does_not_create_the_configuration_directory_of_fish() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = dry_run(Shell::Fish, home.path(), bin.path());

        assert!(
            preview(&plan).contains("set -gx PATH"),
            "saída sem a sintaxe do fish"
        );
        assert!(
            !plan.path.exists(),
            "o --print criou {}",
            plan.path.display()
        );
        assert!(
            !home.path().join(".config").exists(),
            "o --print criou o diretório de configuração do fish"
        );
    }

    #[test]
    fn an_already_declared_directory_is_reported_without_rewriting() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let first = dry_run(Shell::Zsh, home.path(), bin.path());
        apply(&first.path, &first.block()).unwrap();
        let before = std::fs::read(&first.path).unwrap();

        let again = dry_run(Shell::Zsh, home.path(), bin.path());
        let out = preview(&again);

        assert!(
            out.contains("action: already-present"),
            "saída sem a ação: {out}"
        );
        assert!(out.contains(MARKER_START), "saída sem o bloco: {out}");
        assert_eq!(std::fs::read(&again.path).unwrap(), before);
    }

    #[test]
    fn the_print_flag_is_recognised_and_an_unknown_flag_is_refused() {
        let args = vec![
            "--print".to_string(),
            "--shell".to_string(),
            "auto".to_string(),
        ];
        let options = parse_options(&args).unwrap();
        assert!(options.print);
        assert!(options.shell.is_none());

        let error = parse_options(&["--nada".to_string()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("--nada"), "mensagem sem a flag: {error}");
    }
}

#[cfg(test)]
mod shell_override_tests {
    use super::*;

    /// `HOME` injetado e `SHELL` apontando para o zsh: sem `--shell` a
    /// detecção escolheria o `~/.zshrc`.
    fn env_with_zsh(home: &std::path::Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| match key {
            "HOME" => Some(home.display().to_string()),
            "SHELL" => Some("/usr/bin/zsh".to_string()),
            _ => None,
        }
    }

    fn binary(dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    fn forced(name: &str, home: &std::path::Path, bin: &std::path::Path) -> Plan {
        let args = vec![
            "--print".to_string(),
            "--shell".to_string(),
            name.to_string(),
        ];
        let options = parse_options(&args).unwrap();
        plan(
            &options,
            &binary(bin),
            env_with_zsh(home),
            |path: &std::path::Path| path.exists(),
        )
        .unwrap()
    }

    #[test]
    fn an_invalid_shell_is_refused_citing_every_accepted_value() {
        for args in [
            vec!["--shell".to_string(), "tcsh".to_string()],
            vec!["--shell=tcsh".to_string()],
        ] {
            let error = parse_options(&args).unwrap_err().to_string();
            assert!(error.contains("tcsh"), "mensagem sem o valor: {error}");
            for accepted in ["auto", "bash", "zsh", "fish", "posix", "powershell"] {
                assert!(
                    error.contains(accepted),
                    "mensagem sem `{accepted}`: {error}"
                );
            }
        }
    }

    #[test]
    fn an_empty_value_after_the_equals_sign_is_refused_too() {
        let error = parse_options(&["--shell=".to_string()])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("desconhecido"),
            "mensagem inesperada: {error}"
        );
    }

    #[test]
    fn a_forced_bash_wins_over_the_zsh_detection() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = forced("bash", home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "bash");
        assert_eq!(plan.path, home.path().join(".bashrc"));
        assert!(
            plan.block().contains("export PATH="),
            "bloco sem a sintaxe POSIX: {}",
            plan.block()
        );
        assert!(preview(&plan).contains("shell: bash"));

        apply(&plan.path, &plan.block()).unwrap();
        assert!(
            plan.path.exists(),
            "nada foi escrito em {}",
            plan.path.display()
        );
        assert!(
            !home.path().join(".zshrc").exists(),
            "o zsh foi tocado apesar do --shell bash"
        );
    }

    #[test]
    fn a_forced_fish_writes_the_fish_configuration_with_its_own_syntax() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = forced("fish", home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "fish");
        assert_eq!(
            plan.path,
            home.path().join(".config").join("fish").join("config.fish")
        );
        assert!(
            plan.block().contains("set -gx PATH"),
            "bloco sem a sintaxe do fish: {}",
            plan.block()
        );

        apply(&plan.path, &plan.block()).unwrap();
        assert!(
            plan.path.exists(),
            "nada foi escrito em {}",
            plan.path.display()
        );
    }

    #[test]
    fn a_forced_powershell_uses_the_profile_and_the_semicolon_separator() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = forced("powershell", home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "powershell");
        assert_eq!(
            plan.path,
            home.path()
                .join("Documents")
                .join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1")
        );
        let block = plan.block();
        assert!(
            block.contains("$env:Path = "),
            "bloco sem o PowerShell: {block}"
        );
        assert!(
            block.contains(&format!("{};$env:Path", plan.dir.display())),
            "separador errado no bloco: {block}"
        );
    }

    #[test]
    fn a_forced_posix_uses_the_profile_and_the_names_are_case_insensitive() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = forced("POSIX", home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "posix");
        assert_eq!(plan.path, home.path().join(".profile"));
        assert!(
            plan.block().contains("export PATH="),
            "bloco sem a sintaxe POSIX: {}",
            plan.block()
        );
    }

    #[test]
    fn auto_keeps_the_detection_in_charge() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = forced("auto", home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "zsh");
        assert_eq!(plan.path, home.path().join(".zshrc"));
    }
}

#[cfg(test)]
mod output_lang_tests {
    use super::*;

    fn project(root: &Path, body: &str) {
        std::fs::write(root.join("ai.project.json"), body).unwrap();
    }

    fn sample(root: &Path) -> Plan {
        Plan {
            shell: Shell::Zsh,
            dir: root.join("bin"),
            path: root.join(".zshrc"),
            action: Action::Created,
            in_path: false,
        }
    }

    #[test]
    fn the_explicit_project_language_is_used() {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), r#"{"name":"projeto","lang":"pt-BR"}"#);

        let lang = output_lang(dir.path());
        assert_eq!(lang, crate::lang::Lang::PtBr);

        let plan = sample(dir.path());
        let out = report_in(&plan, Action::Created, lang);
        assert!(out.contains("diretório: "), "relatório sem o rótulo: {out}");
        assert!(out.contains("ação: criado"), "ação não traduzida: {out}");
        assert!(
            out.contains("reabra o shell"),
            "saída sem o aviso de reabrir o shell: {out}"
        );
        assert!(
            out.contains(&plan.path.display().to_string()),
            "saída sem o arquivo: {out}"
        );

        let preview = preview_in(&plan, lang);
        assert!(
            preview.contains("nenhum arquivo foi criado ou alterado"),
            "prévia em inglês: {preview}"
        );
        assert!(
            preview.contains(&plan.block()),
            "prévia sem o bloco: {preview}"
        );
    }

    #[test]
    fn without_the_field_the_output_is_english() {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), r#"{"name":"projeto"}"#);

        // `ProjectConfig::lang` completaria o campo com o locale do sistema;
        // `output_lang` só lê o que está escrito no arquivo.
        assert_eq!(output_lang(dir.path()), crate::lang::Lang::En);

        let plan = sample(dir.path());
        let out = report(&plan, Action::Created);
        assert!(out.contains("directory: "), "relatório sem o rótulo: {out}");
        assert!(out.contains("action: created"), "ação diferente: {out}");
        assert!(
            out.contains("restart your shell"),
            "saída sem o aviso de reabrir o shell: {out}"
        );
        assert_eq!(preview(&plan), preview_in(&plan, crate::lang::Lang::En));
    }

    #[test]
    fn an_explicit_english_stays_english() {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), r#"{"name":"projeto","lang":"en"}"#);

        assert_eq!(output_lang(dir.path()), crate::lang::Lang::En);
    }

    #[test]
    fn a_missing_or_invalid_project_file_falls_back_to_english() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(output_lang(dir.path()), crate::lang::Lang::En);

        project(dir.path(), "nao e json");
        assert_eq!(output_lang(dir.path()), crate::lang::Lang::En);

        project(dir.path(), r#"{"name":"projeto","lang":"klingon"}"#);
        assert_eq!(output_lang(dir.path()), crate::lang::Lang::En);
    }

    #[test]
    fn every_action_label_follows_the_language() {
        let dir = tempfile::tempdir().unwrap();
        let plan = sample(dir.path());
        let portuguese = [
            (Action::Created, "criado"),
            (Action::Updated, "atualizado"),
            (Action::AlreadyPresent, "já-presente"),
        ];
        for (action, label) in portuguese {
            let out = report_in(&plan, action, crate::lang::Lang::PtBr);
            assert!(
                out.contains(&format!("ação: {label}")),
                "ação {label}: {out}"
            );
            let out = report(&plan, action);
            assert!(
                out.contains(&format!("action: {}", action.as_str())),
                "ação em inglês: {out}"
            );
        }
    }
}

/// Cenário de uso normal: `tupi install` num home sem nenhum arquivo de
/// inicialização. `HOME`/`SHELL` vêm da closure, nunca do ambiente real.
#[cfg(test)]
mod fresh_home_tests {
    use super::*;
    use std::path::Path;

    /// `bin_dir` canonicaliza, então o executável precisa existir de fato.
    fn binary(dir: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    fn bash_env(home: &Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| match key {
            "HOME" => Some(home.display().to_string()),
            "SHELL" => Some("/bin/bash".to_string()),
            _ => None,
        }
    }

    fn plan_for(home: &Path, bin: &Path) -> Plan {
        plan(
            &Options::default(),
            &binary(bin),
            bash_env(home),
            |path: &Path| path.exists(),
        )
        .unwrap()
    }

    #[test]
    fn a_fresh_home_gains_the_bash_profile_with_the_binary_directory() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let profile = home.path().join(".bashrc");
        assert!(!profile.exists(), "o cenário exige um home sem .bashrc");

        let plan = plan_for(home.path(), bin.path());
        assert_eq!(plan.shell.as_str(), "bash");
        assert_eq!(plan.path, profile);
        assert_eq!(plan.dir, bin.path().canonicalize().unwrap());
        assert!(matches!(plan.action, Action::Created));

        assert!(matches!(
            apply(&plan.path, &plan.block()).unwrap(),
            Action::Created
        ));

        let content = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(content, plan.block(), "o arquivo novo é só o bloco");
        assert_eq!(content.matches(MARKER_START).count(), 1);
        assert_eq!(content.matches(MARKER_END).count(), 1);

        let declared = plan.dir.display().to_string();
        let line = content
            .lines()
            .find(|line| line.starts_with("export PATH="))
            .expect("o bloco não exporta o PATH");
        let entries: Vec<&str> = line
            .trim_start_matches("export PATH=")
            .trim_matches('"')
            .split(':')
            .collect();
        assert!(
            entries.contains(&declared.as_str()),
            "o PATH declarado não contém {declared}: {line}"
        );
        assert_eq!(entries.first().copied(), Some(declared.as_str()));
    }
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Executável de mentira dentro de um diretório temporário: o `home`
    /// injetado isola o teste do perfil real do usuário.
    fn binary(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    /// Só o `HOME` injetado responde, no padrão de `data_dir`.
    fn home_env(home: &Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| (key == "HOME").then(|| home.display().to_string())
    }

    fn plan_for(shell: Shell, home: &Path, bin: &Path) -> Plan {
        plan(
            &Options {
                print: false,
                shell: Some(shell),
            },
            &binary(bin),
            home_env(home),
            |path: &Path| path.exists(),
        )
        .unwrap()
    }

    fn marker_counts(text: &str) -> (usize, usize) {
        (
            text.matches(MARKER_START).count(),
            text.matches(MARKER_END).count(),
        )
    }

    /// A sequência real do comando (`plan` + `apply`) duas vezes seguidas em
    /// cada shell deixa exatamente um par de marcadores e nada é reescrito.
    #[test]
    fn two_consecutive_runs_keep_a_single_marker_pair_in_every_shell() {
        for name in ["bash", "zsh", "fish", "posix", "powershell"] {
            let shell = Shell::from_name(name).unwrap();
            let home = tempfile::tempdir().unwrap();
            let bin = tempfile::tempdir().unwrap();

            let first = plan_for(shell, home.path(), bin.path());
            let created = apply(&first.path, &first.block()).unwrap();
            assert!(
                matches!(created, Action::Created | Action::Updated),
                "{name}: a primeira execução não escreveu o bloco"
            );
            let after_first = std::fs::read(&first.path).unwrap();

            let second = plan_for(shell, home.path(), bin.path());
            assert_eq!(second.path, first.path, "{name}: o arquivo mudou");
            assert!(
                matches!(second.action, Action::AlreadyPresent),
                "{name}: a segunda execução não foi reconhecida como no-op"
            );
            assert!(
                matches!(
                    apply(&second.path, &second.block()).unwrap(),
                    Action::AlreadyPresent
                ),
                "{name}: a segunda execução reescreveu o arquivo"
            );
            assert_eq!(
                std::fs::read(&second.path).unwrap(),
                after_first,
                "{name}: os bytes mudaram na segunda execução"
            );
            let text = std::fs::read_to_string(&second.path).unwrap();
            assert_eq!(
                marker_counts(&text),
                (1, 1),
                "{name}: marcadores duplicados ou ausentes:\n{text}"
            );
            assert_eq!(text, second.block(), "{name}: conteúdo final inesperado");
        }
    }

    /// Se o executável muda de diretório entre as execuções, o bloco é
    /// substituído no lugar: nada é anexado e nunca há dois blocos.
    #[test]
    fn two_runs_with_a_moved_binary_replace_the_block_in_place() {
        let home = tempfile::tempdir().unwrap();
        let old_bin = tempfile::tempdir().unwrap();
        let new_bin = tempfile::tempdir().unwrap();
        let profile = home.path().join(".zshrc");
        std::fs::write(&profile, "export EDITOR=vim\n").unwrap();

        let first = plan_for(Shell::Zsh, home.path(), old_bin.path());
        assert!(matches!(
            apply(&first.path, &first.block()).unwrap(),
            Action::Updated
        ));

        let second = plan_for(Shell::Zsh, home.path(), new_bin.path());
        assert_eq!(second.path, profile);
        assert_eq!(second.dir, new_bin.path().canonicalize().unwrap());
        assert!(matches!(
            apply(&second.path, &second.block()).unwrap(),
            Action::Updated
        ));

        let text = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(
            marker_counts(&text),
            (1, 1),
            "marcadores duplicados:\n{text}"
        );
        assert!(
            text.starts_with("export EDITOR=vim\n"),
            "linha do usuário perdida:\n{text}"
        );
        let old_dir = first.dir.display().to_string();
        let new_dir = second.dir.display().to_string();
        assert!(
            !text.contains(&old_dir),
            "PATH antigo ficou para trás:\n{text}"
        );
        assert!(text.contains(&new_dir), "PATH novo ausente:\n{text}");
        assert_eq!(marker_counts(&text), (1, 1), "marcadores duplicados");
    }
}

#[cfg(test)]
mod windows_line_endings_tests {
    use super::*;
    use std::path::Path;

    /// `bin_dir` canonicaliza, então o executável precisa existir de fato.
    fn binary(dir: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    fn plan_for(home: &Path, bin: &Path) -> Plan {
        plan(
            &Options::default(),
            &binary(bin),
            |key: &str| (key == "HOME").then(|| home.display().to_string()),
            |path: &Path| path.exists(),
        )
        .unwrap()
    }

    fn assert_one_pair_of_markers(text: &str, block: &str, caso: &str) {
        assert_eq!(
            text.matches(MARKER_START).count(),
            1,
            "marcador inicial duplicado ({caso}): {text:?}"
        );
        assert_eq!(
            text.matches(MARKER_END).count(),
            1,
            "marcador final duplicado ({caso}): {text:?}"
        );
        assert_eq!(
            text.matches(block).count(),
            1,
            "bloco duplicado ({caso}): {text:?}"
        );
    }

    #[test]
    fn a_crlf_profile_keeps_a_single_marker_pair_over_two_runs() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_for(home.path(), bin.path());
        let profile = plan.path.clone();
        let own = "# perfil\r\nexport EDITOR=vim\r\n";
        std::fs::write(&profile, own).unwrap();
        let block = plan.block();

        assert!(matches!(apply(&profile, &block).unwrap(), Action::Updated));
        let after_first = std::fs::read_to_string(&profile).unwrap();
        assert!(
            after_first.starts_with(own),
            "linha do usuário perdida: {after_first:?}"
        );
        assert_one_pair_of_markers(&after_first, &block, "primeira execução");

        assert!(matches!(
            apply(&profile, &block).unwrap(),
            Action::AlreadyPresent
        ));
        let after_second = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(
            after_second, after_first,
            "a segunda execução reescreveu o perfil"
        );
        assert_one_pair_of_markers(&after_second, &block, "segunda execução");
    }

    #[test]
    fn a_profile_without_trailing_newline_keeps_a_single_marker_pair() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_for(home.path(), bin.path());
        let profile = plan.path.clone();
        std::fs::write(&profile, "export EDITOR=vim").unwrap();
        let block = plan.block();

        assert!(matches!(apply(&profile, &block).unwrap(), Action::Updated));
        let after_first = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(after_first, format!("export EDITOR=vim\n{block}"));
        assert_one_pair_of_markers(&after_first, &block, "primeira execução");

        assert!(matches!(
            apply(&profile, &block).unwrap(),
            Action::AlreadyPresent
        ));
        let after_second = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(
            after_second, after_first,
            "a segunda execução reescreveu o perfil"
        );
        assert_one_pair_of_markers(&after_second, &block, "segunda execução");
    }
}

/// Separador entre entradas do `PATH`: `;` no PowerShell, `:` nos demais.
pub fn path_separator(shell: Shell) -> char {
    match shell {
        Shell::PowerShell => ';',
        Shell::Zsh | Shell::Bash | Shell::Fish | Shell::Posix => ':',
    }
}

/// Entradas do `PATH` deste ambiente, sem vazios e sem a barra final.
pub fn path_entries<G>(shell: Shell, get_env: &G) -> Vec<PathBuf>
where
    G: Fn(&str) -> Option<String>,
{
    let Some(value) = get_env("PATH") else {
        return Vec::new();
    };

    value
        .split(path_separator(shell))
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| PathBuf::from(normalized_entry(entry)))
        .collect()
}

/// `/usr/bin/` e `/usr/bin` são o mesmo diretório; `/` continua `/`.
fn normalized_entry(entry: &str) -> &str {
    let trimmed = entry.trim();
    let stripped = trimmed.trim_end_matches(|c| c == '/' || c == '\\');

    if stripped.is_empty() {
        trimmed
    } else {
        stripped
    }
}

/// O diretório já está declarado no `PATH` deste ambiente?
///
/// Canonicaliza dos dois lados quando possível, para que um `PATH` com
/// symlink case com o diretório real do executável.
pub fn declared_in_path<G>(dir: &Path, shell: Shell, get_env: &G) -> bool
where
    G: Fn(&str) -> Option<String>,
{
    let wanted = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());

    path_entries(shell, get_env)
        .iter()
        .any(|entry| entry.canonicalize().unwrap_or_else(|_| entry.clone()) == wanted)
}

/// O diretório já estar no `PATH` do ambiente não dispensa o bloco marcado: o
/// efeito só vale em shells novos. Estes testes provam a escrita e o relatório.
#[cfg(test)]
mod env_path_note_tests {
    use super::*;

    fn shell() -> Shell {
        if cfg!(windows) {
            Shell::PowerShell
        } else {
            Shell::Zsh
        }
    }

    fn other_dir() -> &'static str {
        if cfg!(windows) {
            "C:\\Windows"
        } else {
            "/usr/bin"
        }
    }

    fn binary(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    /// `HOME` e `PATH` injetados por closure: o perfil real nunca é tocado.
    fn env(home: &Path, path: &str) -> impl Fn(&str) -> Option<String> {
        let home = home.display().to_string();
        let path = path.to_string();
        move |key| match key {
            "HOME" => Some(home.clone()),
            "PATH" => Some(path.clone()),
            _ => None,
        }
    }

    fn plan_with(print: bool, home: &Path, bin: &Path, path_env: &str) -> Plan {
        let options = Options {
            print,
            shell: Some(shell()),
        };
        plan(
            &options,
            &binary(bin),
            env(home, path_env),
            |path: &Path| path.exists(),
        )
        .unwrap()
    }

    fn in_path_env(bin: &Path) -> String {
        format!(
            "{}{}{}",
            bin.display(),
            path_separator(shell()),
            other_dir()
        )
    }

    #[test]
    fn a_directory_already_in_the_environment_path_is_still_written() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let path_env = in_path_env(bin.path());

        let plan = plan_with(false, home.path(), bin.path(), &path_env);
        assert!(
            plan.in_path,
            "PATH injetado não foi reconhecido: {path_env}"
        );

        let action = apply(&plan.path, &plan.block()).unwrap();
        assert_eq!(
            action.as_str(),
            "created",
            "nada foi gravado em {}",
            plan.path.display()
        );

        let written = std::fs::read_to_string(&plan.path).unwrap();
        assert_eq!(
            written.matches(MARKER_START).count(),
            1,
            "bloco ausente: {written}"
        );
        assert_eq!(
            written.matches(MARKER_END).count(),
            1,
            "bloco ausente: {written}"
        );
        assert!(
            written.contains(&plan.dir.display().to_string()),
            "sem o diretório no bloco: {written}"
        );

        let again = plan_with(false, home.path(), bin.path(), &path_env);
        assert!(
            again.in_path,
            "o PATH do ambiente sumiu na segunda execução"
        );
        assert_eq!(
            apply(&again.path, &again.block()).unwrap().as_str(),
            "already-present",
            "a segunda execução não foi no-op"
        );
    }

    #[test]
    fn the_report_mentions_a_directory_already_in_the_environment_path() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_with(false, home.path(), bin.path(), &in_path_env(bin.path()));
        let dir = plan.dir.display().to_string();

        let en = report_in(&plan, Action::Created, crate::lang::Lang::En);
        assert!(
            en.contains("already in this environment's PATH"),
            "relatório sem a observação: {en}"
        );
        assert!(en.contains(&dir), "relatório sem o diretório: {en}");
        assert!(en.contains("action: created"), "relatório sem a ação: {en}");

        let pt = report_in(&plan, Action::Created, crate::lang::Lang::PtBr);
        assert!(
            pt.contains("já estava no PATH"),
            "relatório sem a observação: {pt}"
        );
        assert!(pt.contains(&dir), "relatório sem o diretório: {pt}");
    }

    #[test]
    fn a_directory_absent_from_the_environment_path_gains_no_note() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_with(false, home.path(), bin.path(), other_dir());

        assert!(!plan.in_path, "PATH do ambiente reconhecido sem estar lá");

        let en = report_in(&plan, Action::Created, crate::lang::Lang::En);
        assert!(
            !en.contains("already in this environment's PATH"),
            "observação indevida: {en}"
        );
        let pt = report_in(&plan, Action::Created, crate::lang::Lang::PtBr);
        assert!(
            !pt.contains("já estava no PATH"),
            "observação indevida: {pt}"
        );

        assert_eq!(
            apply(&plan.path, &plan.block()).unwrap().as_str(),
            "created",
            "o bloco deixou de ser gravado"
        );
    }

    #[test]
    fn the_dry_run_reports_the_environment_path_without_writing() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_with(true, home.path(), bin.path(), &in_path_env(bin.path()));

        let out = preview_in(&plan, crate::lang::Lang::En);
        assert!(
            out.contains("would be written anyway"),
            "dry run sem a observação: {out}"
        );
        assert!(
            !plan.path.exists(),
            "o dry run criou {}",
            plan.path.display()
        );
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "o dry run escreveu em {}",
            home.path().display()
        );
    }
}

/// Subtarefa t4.4: o diretório já presente no `PATH` do ambiente.
#[cfg(test)]
mod path_present_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// `bin_dir` canonicaliza, então o executável precisa existir de fato.
    fn binary(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    /// `HOME`, `SHELL` e `PATH` injetados: nada do ambiente real é lido.
    fn env_with_path<'a>(home: &'a Path, path: &'a str) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| match key {
            "HOME" => Some(home.display().to_string()),
            "SHELL" => Some("/bin/bash".to_string()),
            "PATH" => Some(path.to_string()),
            _ => None,
        }
    }

    fn plan_for(shell: Shell, home: &Path, bin: &Path, path: &str) -> Plan {
        plan(
            &Options {
                print: false,
                shell: Some(shell),
            },
            &binary(bin),
            env_with_path(home, path),
            |candidate: &Path| candidate.exists(),
        )
        .unwrap()
    }

    /// O aceite: com o diretório já no `PATH` do ambiente, o bloco é gravado
    /// de qualquer forma e a saída reporta o fato.
    #[test]
    fn the_block_is_written_and_reported_when_the_directory_is_already_in_the_path() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let declared = bin.path().canonicalize().unwrap();
        let environment_path = format!("/usr/bin:{}", declared.display());
        let declared = declared.display().to_string();

        let plan = plan_for(Shell::Bash, home.path(), bin.path(), &environment_path);
        assert!(plan.in_path, "o diretório do executável está no PATH");
        assert!(
            matches!(plan.action, Action::Created),
            "o arquivo ainda não existia"
        );

        let action = apply(&plan.path, &plan.block()).unwrap();
        assert!(
            matches!(action, Action::Created),
            "o bloco não foi gravado mesmo com o diretório já no PATH"
        );

        let content = std::fs::read_to_string(&plan.path).unwrap();
        assert_eq!(content, plan.block(), "o arquivo novo é só o bloco");
        assert_eq!(content.matches(MARKER_START).count(), 1);
        assert_eq!(content.matches(MARKER_END).count(), 1);
        assert!(content.contains(&declared), "o bloco não cita {declared}");

        let report = report_in(&plan, action, crate::lang::Lang::En);
        assert!(
            report.contains("already in this environment's PATH"),
            "a saída não reporta o fato: {report}"
        );
        assert!(
            report.contains(&declared),
            "a saída não cita o diretório: {report}"
        );

        let pt_br = report_in(&plan, action, crate::lang::Lang::PtBr);
        assert!(
            pt_br.contains("já estava no PATH deste ambiente"),
            "a nota não foi traduzida: {pt_br}"
        );
    }

    /// A segunda execução continua sendo no-op e continua reportando o fato.
    #[test]
    fn the_no_op_second_run_still_reports_the_directory_was_already_in_the_path() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let declared = bin.path().canonicalize().unwrap();
        let environment_path = format!("/usr/bin:{}:/bin", declared.display());

        let first = plan_for(Shell::Zsh, home.path(), bin.path(), &environment_path);
        apply(&first.path, &first.block()).unwrap();
        let saved = std::fs::read(&first.path).unwrap();

        let second = plan_for(Shell::Zsh, home.path(), bin.path(), &environment_path);
        assert!(second.in_path, "o diretório continua no PATH");
        assert!(
            matches!(second.action, Action::AlreadyPresent),
            "a segunda execução não é no-op"
        );
        let action = apply(&second.path, &second.block()).unwrap();
        assert!(matches!(action, Action::AlreadyPresent));
        assert_eq!(
            std::fs::read(&second.path).unwrap(),
            saved,
            "o arquivo mudou na segunda execução"
        );

        let report = report_in(&second, action, crate::lang::Lang::En);
        assert!(report.contains("already-present"), "ação errada: {report}");
        assert!(
            report.contains("already in this environment's PATH"),
            "o no-op escondeu o fato: {report}"
        );
    }

    /// Fora do `PATH` do ambiente, nenhuma nota aparece na saída.
    #[test]
    fn a_directory_outside_the_environment_path_carries_no_note() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();

        let plan = plan_for(Shell::Bash, home.path(), bin.path(), "/usr/bin:/bin");
        assert!(!plan.in_path, "o diretório não está no PATH injetado");

        let action = apply(&plan.path, &plan.block()).unwrap();
        assert_eq!(std::fs::read_to_string(&plan.path).unwrap(), plan.block());

        let report = report_in(&plan, action, crate::lang::Lang::En);
        assert!(
            !report.contains("already in this environment's PATH"),
            "nota indevida fora do PATH: {report}"
        );
        let preview = preview_in(&plan, crate::lang::Lang::En);
        assert!(
            !preview.contains("already in this environment's PATH"),
            "nota indevida no dry run: {preview}"
        );
    }

    /// O separador das entradas segue o shell: `:` no Unix, `;` no PowerShell.
    #[test]
    fn the_entries_follow_the_separator_of_the_shell() {
        let posix = env_with_path(Path::new("/home/teste"), "/usr/bin:/opt/x");
        assert!(declared_in_path(Path::new("/opt/x"), Shell::Posix, &posix));
        assert!(!declared_in_path(Path::new("/opt"), Shell::Posix, &posix));

        let windows = env_with_path(Path::new("C:\\Users\\teste"), "C:\\Windows;C:\\tools\\tupi");
        assert!(declared_in_path(
            Path::new("C:\\tools\\tupi"),
            Shell::PowerShell,
            &windows
        ));

        // A barra final não cria um diretório diferente.
        let trailing = env_with_path(Path::new("/home/teste"), "/opt/x/");
        assert!(declared_in_path(
            Path::new("/opt/x"),
            Shell::Posix,
            &trailing
        ));
    }
}

/// Subtarefa t4.5: as variantes de shell, com o caminho do arquivo de
/// inicialização e a sintaxe de cada uma: POSIX em `~/.profile`, fish em
/// `~/.config/fish/config.fish` e PowerShell no perfil em `Documents`.
#[cfg(test)]
mod shell_variant_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// `bin_dir` canonicaliza, então o executável precisa existir de fato.
    fn binary(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("tupi");
        std::fs::write(&exe, b"binario").unwrap();
        exe
    }

    /// `HOME` injetado e `SHELL` apontando para o bash: a variante testada
    /// vem sempre do `--shell` explícito, nunca da detecção.
    fn env(home: &Path) -> impl Fn(&str) -> Option<String> + '_ {
        move |key| match key {
            "HOME" => Some(home.display().to_string()),
            "SHELL" => Some("/bin/bash".to_string()),
            _ => None,
        }
    }

    fn plan_for(shell: Shell, home: &Path, bin: &Path) -> Plan {
        plan(
            &Options {
                print: false,
                shell: Some(shell),
            },
            &binary(bin),
            env(home),
            |candidate: &Path| candidate.exists(),
        )
        .unwrap()
    }

    /// POSIX genérico: `~/.profile` com `export` e `:` entre as entradas.
    #[test]
    fn the_posix_variant_writes_dot_profile_with_the_export_syntax() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let plan = plan_for(Shell::Posix, home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "posix");
        assert_eq!(plan.path, home.path().join(".profile"), "arquivo errado");
        let dir = plan.dir.display().to_string();
        assert_eq!(
            plan.block(),
            format!("{MARKER_START}\nexport PATH=\"{dir}:$PATH\"\n{MARKER_END}\n")
        );

        assert!(matches!(
            apply(&plan.path, &plan.block()).unwrap(),
            Action::Created
        ));
        assert_eq!(
            std::fs::read_to_string(&plan.path).unwrap(),
            plan.block(),
            "o arquivo novo é só o bloco"
        );
        assert!(
            matches!(
                plan_for(Shell::Posix, home.path(), bin.path()).action,
                Action::AlreadyPresent
            ),
            "a segunda execução não foi no-op"
        );
    }

    /// fish: o `apply` cria `~/.config/fish` e a linha usa `set -gx`, com
    /// espaço entre as entradas em vez de `:`.
    #[test]
    fn the_fish_variant_creates_the_config_directory_and_uses_set_gx() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let config = home.path().join(".config").join("fish");
        assert!(!config.exists(), "o cenário exige o diretório ausente");

        let plan = plan_for(Shell::Fish, home.path(), bin.path());
        assert_eq!(plan.shell.as_str(), "fish");
        assert_eq!(plan.path, config.join("config.fish"));
        assert!(
            !config.exists(),
            "o plano criou {} sem escrever",
            config.display()
        );
        let dir = plan.dir.display().to_string();
        assert_eq!(
            plan.block(),
            format!("{MARKER_START}\nset -gx PATH \"{dir}\" $PATH\n{MARKER_END}\n")
        );

        assert!(matches!(
            apply(&plan.path, &plan.block()).unwrap(),
            Action::Created
        ));
        assert!(config.is_dir(), "o diretório do fish não foi criado");
        assert_eq!(
            std::fs::read_to_string(&plan.path).unwrap(),
            plan.block(),
            "o arquivo novo é só o bloco"
        );
        assert!(
            matches!(
                plan_for(Shell::Fish, home.path(), bin.path()).action,
                Action::AlreadyPresent
            ),
            "a segunda execução não foi no-op"
        );
    }

    /// PowerShell: o perfil em `Documents/PowerShell` e `;` entre as entradas.
    #[test]
    fn the_power_shell_variant_targets_the_profile_and_joins_with_a_semicolon() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let profile = home
            .path()
            .join("Documents")
            .join("PowerShell")
            .join("Microsoft.PowerShell_profile.ps1");
        let plan = plan_for(Shell::PowerShell, home.path(), bin.path());

        assert_eq!(plan.shell.as_str(), "powershell");
        assert_eq!(plan.path, profile);
        assert_eq!(path_separator(Shell::PowerShell), ';');
        let dir = plan.dir.display().to_string();
        assert_eq!(
            plan.block(),
            format!("{MARKER_START}\n$env:Path = \"{dir};$env:Path\"\n{MARKER_END}\n")
        );

        assert!(matches!(
            apply(&plan.path, &plan.block()).unwrap(),
            Action::Created
        ));
        assert_eq!(
            std::fs::read_to_string(&plan.path).unwrap(),
            plan.block(),
            "o arquivo novo é só o bloco"
        );
        assert!(
            matches!(
                plan_for(Shell::PowerShell, home.path(), bin.path()).action,
                Action::AlreadyPresent
            ),
            "a segunda execução não foi no-op"
        );
    }

    /// O dry run de cada variante nomeia o seu arquivo e a sua sintaxe, sem
    /// criar nada.
    #[test]
    fn every_variant_previews_its_own_file_and_syntax() {
        let home = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();

        for (shell, syntax) in [
            (Shell::Posix, "export PATH="),
            (Shell::Fish, "set -gx PATH"),
            (Shell::PowerShell, "$env:Path = "),
        ] {
            let plan = plan_for(shell, home.path(), bin.path());
            let preview = preview_in(&plan, crate::lang::Lang::En);
            let name = shell.as_str();
            assert!(
                preview.contains(&format!("shell: {name}")),
                "{name}: prévia sem o shell: {preview}"
            );
            assert!(
                preview.contains(&format!("file: {}", plan.path.display())),
                "{name}: prévia sem o arquivo: {preview}"
            );
            assert!(
                preview.contains(syntax),
                "{name}: prévia sem a sintaxe `{syntax}`: {preview}"
            );
            assert!(
                !plan.path.exists(),
                "{name}: a prévia criou {}",
                plan.path.display()
            );
        }
    }
}
