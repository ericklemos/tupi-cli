//! Chaves de casamento tolerante: trigramas e fonética pt-BR.

use super::normalize::fold;

/// Todas as janelas de três caracteres do token.
pub fn grams(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 3 {
        return Vec::new();
    }
    chars
        .windows(3)
        .map(|window| window.iter().collect())
        .collect()
}

/// Chave fonética: "joao" e "juao" caem na mesma.
pub fn phonetic(token: &str) -> String {
    let folded = fold(token);
    let chars: Vec<char> = folded.chars().collect();
    let mut out = String::with_capacity(folded.len());
    let mut last: Option<char> = None;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        let mapped = match ch {
            'p' if next == Some('h') => {
                index += 1;
                'f'
            }
            'n' if next == Some('h') => {
                index += 1;
                'n'
            }
            'l' if next == Some('h') => {
                index += 1;
                'l'
            }
            'c' if matches!(next, Some('e' | 'i')) => 's',
            'c' | 'q' => 'k',
            'z' | 'x' => 's',
            'a' | 'e' | 'i' | 'o' | 'u' => {
                if out.is_empty() {
                    'a'
                } else {
                    index += 1;
                    continue;
                }
            }
            other => other,
        };
        if Some(mapped) != last {
            out.push(mapped);
            last = Some(mapped);
        }
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phonetic_collapses_common_pt_br_spellings() {
        assert_eq!(phonetic("joao"), phonetic("juao"));
        assert_eq!(phonetic("marcelo"), phonetic("marselo"));
    }

    #[test]
    fn grams_covers_every_window_of_three() {
        assert_eq!(grams("joao"), vec!["joa", "oao"]);
        assert!(grams("jo").is_empty());
    }
}
