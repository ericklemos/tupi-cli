//! Fusão entre as palavras transcritas e os trechos de locutor.

/// Uma palavra transcrita, com tempo em centésimos de segundo.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub start_cs: i64,
    pub end_cs: i64,
    pub text: String,
}

/// Um trecho contínuo atribuído a um locutor, em centésimos de segundo.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start_cs: i64,
    pub end_cs: i64,
    pub speaker: i32,
}

/// Uma linha de diálogo: um turno de um locutor.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub speaker: u32,
    pub start_cs: i64,
    pub text: String,
}

/// Agrupa palavras em turnos, atribuindo cada palavra ao locutor do trecho
/// com maior sobreposição temporal.
///
/// Palavra sem sobreposição herda o locutor anterior; no começo do áudio, usa
/// o locutor do primeiro trecho. Os locutores são renumerados por ordem de
/// aparição (1, 2, ...), independentemente dos rótulos da diarização.
pub fn lines_from_words(words: &[Word], spans: &[Span]) -> Vec<Line> {
    let fallback = spans.first().map(|span| span.speaker);
    let mut numbering: Vec<i32> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Option<Line> = None;
    let mut previous: Option<i32> = None;

    for word in words {
        let speaker = match best_span(word, spans) {
            Some(speaker) => speaker,
            None => previous.or(fallback).unwrap_or(0),
        };
        previous = Some(speaker);

        let number = match numbering.iter().position(|known| *known == speaker) {
            Some(index) => index as u32 + 1,
            None => {
                numbering.push(speaker);
                numbering.len() as u32
            }
        };

        match current.as_mut() {
            Some(line) if line.speaker == number => push_word(&mut line.text, &word.text),
            _ => {
                if let Some(line) = current.take() {
                    lines.push(line);
                }
                current = Some(Line {
                    speaker: number,
                    start_cs: word.start_cs,
                    text: word.text.clone(),
                });
            }
        }
    }

    if let Some(line) = current {
        lines.push(line);
    }
    lines
}

fn best_span(word: &Word, spans: &[Span]) -> Option<i32> {
    let mut best: Option<(i64, i64, i32)> = None;
    for span in spans {
        let overlap = word.end_cs.min(span.end_cs) - word.start_cs.max(span.start_cs);
        if overlap <= 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_overlap, best_start, _)) => {
                overlap > best_overlap || (overlap == best_overlap && span.start_cs < best_start)
            }
        };
        if better {
            best = Some((overlap, span.start_cs, span.speaker));
        }
    }
    best.map(|(_, _, speaker)| speaker)
}

fn push_word(buffer: &mut String, word: &str) {
    if buffer.is_empty() {
        buffer.push_str(word);
        return;
    }
    if !starts_attached(word) {
        buffer.push(' ');
    }
    buffer.push_str(word);
}

fn starts_attached(word: &str) -> bool {
    matches!(
        word.chars().next(),
        Some(',' | '.' | '!' | '?' | ';' | ':' | '…' | ')' | ']' | '}')
    )
}

/// Formata o diálogo: uma linha por turno, opcionalmente com o horário de início.
pub fn render(lines: &[Line], timestamps: bool) -> String {
    let mut out = String::new();
    for line in lines {
        if timestamps {
            out.push_str(&format!("[{}] ", clock(line.start_cs)));
        }
        out.push_str(&format!("Voz {}: {}\n", line.speaker, line.text));
    }
    out
}

fn clock(centiseconds: i64) -> String {
    let total = centiseconds.max(0) / 100;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start_cs: i64, end_cs: i64, text: &str) -> Word {
        Word {
            start_cs,
            end_cs,
            text: text.to_string(),
        }
    }

    fn span(start_cs: i64, end_cs: i64, speaker: i32) -> Span {
        Span {
            start_cs,
            end_cs,
            speaker,
        }
    }

    #[test]
    fn a_word_takes_the_speaker_with_the_largest_overlap() {
        let words = [word(0, 10, "um"), word(20, 100, "dois")];
        let spans = [span(0, 30, 0), span(30, 200, 1)];

        let lines = lines_from_words(&words, &spans);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].speaker, 2);
    }

    #[test]
    fn a_new_line_starts_when_the_speaker_changes() {
        let words = [
            word(0, 10, "ola"),
            word(20, 30, "tudo"),
            word(40, 50, "bem"),
        ];
        let spans = [span(0, 15, 0), span(15, 60, 1)];

        let lines = lines_from_words(&words, &spans);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "ola");
        assert_eq!(lines[1].text, "tudo bem");
    }

    #[test]
    fn adjacent_words_of_the_same_speaker_share_one_line() {
        let words = [word(0, 10, "bom"), word(10, 20, "dia"), word(20, 30, "a")];
        let spans = [span(0, 100, 0)];

        let lines = lines_from_words(&words, &spans);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "bom dia a");
    }

    #[test]
    fn a_word_outside_every_span_inherits_the_previous_speaker() {
        let words = [word(0, 10, "a"), word(50, 60, "b")];
        let spans = [span(0, 20, 0)];

        let lines = lines_from_words(&words, &spans);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "a b");
    }

    #[test]
    fn a_word_before_the_first_span_takes_the_first_speaker() {
        let words = [word(0, 10, "a"), word(100, 110, "b")];
        let spans = [span(50, 120, 0), span(120, 200, 1)];

        let lines = lines_from_words(&words, &spans);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "a b");
    }

    #[test]
    fn speakers_are_renumbered_by_first_appearance() {
        let words = [word(0, 10, "a"), word(20, 30, "b"), word(40, 50, "c")];
        let spans = [span(0, 15, 7), span(15, 35, 3), span(35, 60, 7)];

        let lines = lines_from_words(&words, &spans);

        let speakers: Vec<u32> = lines.iter().map(|line| line.speaker).collect();
        assert_eq!(speakers, vec![1, 2, 1]);
    }

    #[test]
    fn render_writes_one_line_per_turn() {
        let lines = [
            Line {
                speaker: 1,
                start_cs: 0,
                text: "oi".to_string(),
            },
            Line {
                speaker: 2,
                start_cs: 100,
                text: "ola".to_string(),
            },
        ];

        assert_eq!(render(&lines, false), "Voz 1: oi\nVoz 2: ola\n");
    }

    #[test]
    fn render_prefixes_the_start_time_when_timestamps_are_on() {
        let lines = [Line {
            speaker: 1,
            start_cs: 372_500,
            text: "oi".to_string(),
        }];

        assert_eq!(render(&lines, true), "[01:02:05] Voz 1: oi\n");
    }

    #[test]
    fn render_of_an_empty_transcript_is_empty() {
        assert_eq!(render(&[], false), "");
    }

    #[test]
    fn render_without_timestamps_has_no_brackets() {
        let lines = [Line {
            speaker: 1,
            start_cs: 0,
            text: "oi".to_string(),
        }];

        assert!(!render(&lines, false).contains('['));
    }
}
