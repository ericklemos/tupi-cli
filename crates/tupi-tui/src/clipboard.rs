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

/// Sequência OSC 52 que coloca `text` na área de transferência do terminal.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", encode_base64(text.as_bytes()))
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
}
