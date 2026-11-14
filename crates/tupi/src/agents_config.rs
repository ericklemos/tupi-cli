use std::path::{Path, PathBuf};

pub const MAX_AGENTS_BYTES: usize = 32 * 1024;

pub fn agents_path(root: &Path) -> PathBuf {
    root.join("AGENTS.md")
}

pub fn load_agents_section(root: &Path) -> Option<String> {
    const SECTION_HEADER: &str = "## Instruções do projeto (AGENTS.md)\n\n";
    const TRUNCATION_NOTICE: &str = "\n\n[AGENTS.md truncado ao limite de 32 KiB.]";

    let path = agents_path(root);
    let raw = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %error, "falha ao ler AGENTS.md; ignorando");
            }
            return None;
        }
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.len() <= MAX_AGENTS_BYTES {
        return Some(format!("{SECTION_HEADER}{trimmed}"));
    }

    let mut end = MAX_AGENTS_BYTES;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    let slice = trimmed[..end].trim_end();
    Some(format!("{SECTION_HEADER}{slice}{TRUNCATION_NOTICE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_agents_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_agents_section(dir.path()), None);
    }

    #[test]
    fn whitespace_only_agents_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(agents_path(dir.path()), "   \n\r\n\t  \n").unwrap();
        assert_eq!(load_agents_section(dir.path()), None);
    }

    #[test]
    fn valid_agents_file_formats_section_with_trimmed_body() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(agents_path(dir.path()), "\nUse cargo test.\n\n").unwrap();
        let section = load_agents_section(dir.path()).expect("seção presente");
        assert_eq!(
            section,
            "## Instruções do projeto (AGENTS.md)\n\nUse cargo test."
        );
    }

    #[test]
    fn invalid_utf8_agents_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(agents_path(dir.path()), [0xff, 0xfe, 0xfd]).unwrap();
        assert_eq!(load_agents_section(dir.path()), None);
    }

    #[test]
    fn unreadable_or_directory_agents_path_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(agents_path(dir.path())).unwrap();
        assert_eq!(load_agents_section(dir.path()), None);
    }

    #[test]
    fn exact_limit_agents_file_is_not_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let body = "a".repeat(MAX_AGENTS_BYTES);
        std::fs::write(agents_path(dir.path()), &body).unwrap();
        let section = load_agents_section(dir.path()).expect("seção presente");
        assert_eq!(
            section,
            format!("## Instruções do projeto (AGENTS.md)\n\n{body}")
        );
        assert!(!section.contains("truncado"));
    }

    #[test]
    fn oversized_utf8_agents_file_truncates_on_char_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = "a".repeat(MAX_AGENTS_BYTES - 1);
        body.push('á');
        body.push_str("fim");
        std::fs::write(agents_path(dir.path()), &body).unwrap();

        let section = load_agents_section(dir.path()).expect("seção presente");
        assert!(section.starts_with("## Instruções do projeto (AGENTS.md)\n\n"));
        assert!(section.contains("truncado"));
        assert!(!section.contains("fim"));
        let prefix_len = "## Instruções do projeto (AGENTS.md)\n\n".len();
        let notice_start = section.rfind("\n\n[").expect("aviso de truncamento");
        assert!(notice_start - prefix_len <= MAX_AGENTS_BYTES);
    }
}
