//! Normalização tolerante para busca: caixa, acentos, invisíveis e telefone.

fn deaccent(ch: char) -> char {
    match ch {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        other => other,
    }
}

fn is_invisible(ch: char) -> bool {
    matches!(
        ch as u32,
        0x200B..=0x200F | 0x202A..=0x202E | 0x2060 | 0xFEFF
    )
}

/// Minúsculas, sem acentos, sem invisíveis e com espaços colapsados.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if is_invisible(ch) {
            continue;
        }
        let ch = deaccent(ch).to_ascii_lowercase();
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

fn is_phone_char(ch: char) -> bool {
    ch.is_ascii_digit() || matches!(ch, '+' | '(' | ')' | '-' | '.' | ' ')
}

/// `fold` + telefones reduzidos a dígitos.
pub fn search_text(text: &str) -> String {
    let folded = fold(text);
    let chars: Vec<char> = folded.chars().collect();
    let mut out = String::with_capacity(folded.len());
    let mut i = 0;
    while i < chars.len() {
        if is_phone_char(chars[i]) && chars[i] != ' ' {
            let start = i;
            let mut digits = 0;
            while i < chars.len() && is_phone_char(chars[i]) {
                if chars[i].is_ascii_digit() {
                    digits += 1;
                }
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            if digits >= 10 {
                for ch in run.chars().filter(|ch| ch.is_ascii_digit()) {
                    out.push(ch);
                }
                if i < chars.len() && chars[i].is_ascii_alphanumeric() {
                    out.push(' ');
                }
            } else {
                out.push_str(&run);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.trim().to_string()
}

/// Tokens de busca, na ordem do texto.
pub fn tokens(text: &str) -> Vec<String> {
    search_text(text)
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_removes_case_accents_and_zero_width() {
        assert_eq!(fold("João  SILVA"), "joao silva");
        assert_eq!(fold("JOSÉ\u{200e}"), "jose");
    }

    #[test]
    fn search_text_normalizes_formatted_phones() {
        assert_eq!(search_text("ligue +55 (11) 98765-4321"), "ligue 5511987654321");
    }

    #[test]
    fn tokens_split_on_anything_but_letters_digits_and_phone_marks() {
        assert_eq!(tokens("João, tudo bem?"), vec!["joao", "tudo", "bem"]);
    }
}
