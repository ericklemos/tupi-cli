use std::sync::{Mutex, OnceLock};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Codifica bytes em base64 padrão, com preenchimento.
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (first << 16) | (second << 8) | third;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(triple >> 6) as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[triple as usize & 0x3f] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

struct Helper {
    program: &'static str,
    args: &'static [&'static str],
}

#[cfg(not(target_os = "macos"))]
const COPY_HELPERS: &[Helper] = &[
    Helper {
        program: "wl-copy",
        args: &[],
    },
    Helper {
        program: "xclip",
        args: &["-selection", "clipboard"],
    },
    Helper {
        program: "xsel",
        args: &["--clipboard", "--input"],
    },
];

#[cfg(not(target_os = "macos"))]
const PASTE_HELPERS: &[Helper] = &[
    Helper {
        program: "wl-paste",
        args: &["--no-newline"],
    },
    Helper {
        program: "xclip",
        args: &["-selection", "clipboard", "-o"],
    },
    Helper {
        program: "xsel",
        args: &["--clipboard", "--output"],
    },
];

#[cfg(target_os = "macos")]
const COPY_HELPERS: &[Helper] = &[Helper {
    program: "pbcopy",
    args: &[],
}];

#[cfg(target_os = "macos")]
const PASTE_HELPERS: &[Helper] = &[Helper {
    program: "pbpaste",
    args: &[],
}];

/// De onde o texto copiado saiu de fato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRoute {
    /// Chegou ao clipboard do sistema (helper externo ou arboard).
    Clipboard,
    /// Só o escape OSC 52 foi emitido: o terminal pode ignorá-lo.
    Terminal,
}

/// Aviso do rodapé. Só anuncia "copiado" quando o sistema assumiu o conteúdo;
/// escrever o escape OSC 52 não prova que o terminal o aceitou.
pub fn copy_notice(route: CopyRoute) -> &'static str {
    match route {
        CopyRoute::Clipboard => "copiado",
        CopyRoute::Terminal => "copiado via OSC 52",
    }
}

/// Entrega `text` ao clipboard do sistema: primeiro um helper externo, depois
/// o clipboard nativo (arboard) — o tupi não depende de binário instalado.
pub fn copy_to_system(text: &str) -> bool {
    copy_via_helper(text) || copy_via_native(text)
}

fn copy_via_helper(text: &str) -> bool {
    use std::io::Write;

    for helper in COPY_HELPERS {
        let Ok(mut child) = std::process::Command::new(helper.program)
            .args(helper.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
        if written && child.wait().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}

/// Instância viva do clipboard nativo. No X11/Wayland quem responde às
/// requisições de colagem é este processo: precisa durar enquanto o tupi roda.
fn native() -> &'static Mutex<Option<arboard::Clipboard>> {
    static CLIPBOARD: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    CLIPBOARD.get_or_init(|| Mutex::new(arboard::Clipboard::new().ok()))
}

fn copy_via_native(text: &str) -> bool {
    let Ok(mut slot) = native().lock() else {
        return false;
    };
    slot.as_mut()
        .is_some_and(|clipboard| clipboard.set_text(text.to_string()).is_ok())
}

fn paste_via_native() -> Option<String> {
    let mut slot = native().lock().ok()?;
    slot.as_mut()?.get_text().ok()
}

/// Lê o clipboard do sistema: primeiro o caminho nativo, depois os helpers.
pub fn paste_from_system() -> Option<String> {
    paste_via_native()
        .filter(|text| !text.is_empty())
        .or_else(paste_via_helper)
}

fn paste_via_helper() -> Option<String> {
    for helper in PASTE_HELPERS {
        let Ok(output) = std::process::Command::new(helper.program)
            .args(helper.args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if output.status.success() {
            return Some(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }
    None
}

/// Sequência OSC 52 que coloca `text` na área de transferência do terminal.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", encode_base64(text.as_bytes()))
}

#[cfg(test)]
mod helper_tests {
    use super::{COPY_HELPERS, PASTE_HELPERS};

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn wayland_helpers_come_before_x11() {
        assert_eq!(COPY_HELPERS[0].program, "wl-copy");
        assert_eq!(PASTE_HELPERS[0].program, "wl-paste");
        assert_eq!(PASTE_HELPERS[1].program, "xclip");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_to_four_chars() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"a"), "YQ==");
        assert_eq!(encode_base64(b"ab"), "YWI=");
        assert_eq!(encode_base64(b"abc"), "YWJj");
    }

    #[test]
    fn osc52_wraps_base64_in_the_clipboard_sequence() {
        assert_eq!(osc52("oi"), "\x1b]52;c;b2k=\x07");
    }

    #[test]
    fn base64_encodes_utf8_bytes() {
        assert_eq!(osc52("✓"), "\x1b]52;c;4pyT\x07");
    }

    #[test]
    fn paste_normalizes_crlf_and_cr_to_lf() {
        assert_eq!(normalize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn escape_only_copy_does_not_claim_the_system_clipboard() {
        assert_eq!(copy_notice(CopyRoute::Clipboard), "copiado");
        assert_ne!(copy_notice(CopyRoute::Terminal), "copiado");
    }
}
