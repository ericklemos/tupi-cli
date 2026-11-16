//! Transcrição local de áudio com separação por locutor.

pub mod asr;
pub mod decode;
pub mod diarize;
pub mod engine;
pub mod merge;
pub mod model;

/// Devolve os argumentos do subcomando `transcribe`, quando ele é o primeiro
/// argumento depois do nome do binário.
pub fn subcommand(args: &[String]) -> Option<&[String]> {
    match args.get(1).map(String::as_str) {
        Some("transcribe") => Some(&args[2..]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn the_subcommand_hands_over_its_arguments() {
        let values = args(&["tupi", "transcribe", "a.ptt", "b.m4a"]);

        assert_eq!(subcommand(&values), Some(&values[2..]));
    }

    #[test]
    fn the_tui_path_without_the_subcommand_is_left_alone() {
        let values = args(&["tupi", "--web"]);

        assert!(subcommand(&values).is_none());
    }

    #[test]
    fn transcribe_elsewhere_is_not_the_subcommand() {
        let values = args(&["tupi", "--model", "transcribe"]);

        assert!(subcommand(&values).is_none());
    }
}
