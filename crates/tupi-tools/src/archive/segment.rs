//! Segmentação tolerante: nenhuma linha do acervo pode ser perdida.

/// Grau de certeza do carimbo de tempo de um segmento.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeCertainty {
    /// Carimbo lido do próprio conteúdo.
    Exact,
    /// Data deduzida do nome do arquivo.
    FromName,
    /// Data deduzida do mtime.
    FromMtime,
    /// Sem data conhecida.
    None,
}

/// Origem do texto do segmento.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    Ocr,
    Transcript,
}

/// Unidade de indexação e de citação.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub source: String,
    pub line_start: usize,
    pub line_end: usize,
    pub ts: Option<i64>,
    pub certainty: TimeCertainty,
    pub kind: SegmentKind,
    pub text: String,
}

struct Open {
    line_start: usize,
    line_end: usize,
    ts: Option<i64>,
    text: String,
}

/// Quebra o texto em segmentos. Linha que não casa com carimbo continua o
/// segmento anterior; se não houver anterior, vira segmento próprio.
pub fn segment_file(
    source: &str,
    text: &str,
    fallback: Option<(i64, TimeCertainty)>,
) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut open: Option<Open> = None;
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        if let Some((ts, rest)) = parse_stamp(raw) {
            if let Some(current) = open.take() {
                segments.push(close(source, current, fallback));
            }
            open = Some(Open {
                line_start: line_no,
                line_end: line_no,
                ts: Some(ts),
                text: rest.to_string(),
            });
            continue;
        }
        match open.as_mut() {
            Some(current) => {
                if !current.text.is_empty() {
                    current.text.push('\n');
                }
                current.text.push_str(raw);
                current.line_end = line_no;
            }
            None => {
                open = Some(Open {
                    line_start: line_no,
                    line_end: line_no,
                    ts: None,
                    text: raw.to_string(),
                });
            }
        }
    }
    if let Some(current) = open {
        segments.push(close(source, current, fallback));
    }
    segments
}

fn close(source: &str, open: Open, fallback: Option<(i64, TimeCertainty)>) -> Segment {
    let (ts, certainty) = match open.ts {
        Some(ts) => (Some(ts), TimeCertainty::Exact),
        None => match fallback {
            Some((ts, certainty)) => (Some(ts), certainty),
            None => (None, TimeCertainty::None),
        },
    };
    Segment {
        source: source.to_string(),
        line_start: open.line_start,
        line_end: open.line_end,
        ts,
        certainty,
        kind: SegmentKind::Text,
        text: open.text,
    }
}

fn parse_stamp(line: &str) -> Option<(i64, &str)> {
    if let Some(rest) = line.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        let inner = &rest[..end];
        if let Some(ts) = parse_datetime(inner) {
            return Some((ts, rest[end + 1..].trim_start()));
        }
    }
    if line.len() >= 16 {
        let head = &line[..16];
        if let Some(ts) = parse_datetime(head) {
            let tail = line[16..].trim_start();
            let tail = tail.strip_prefix("- ").unwrap_or(tail);
            return Some((ts, tail));
        }
    }
    None
}

fn parse_datetime(text: &str) -> Option<i64> {
    let cleaned = text.replace(',', " ");
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let date: Vec<i64> = parts[0]
        .split('/')
        .map(|piece| piece.parse::<i64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if date.len() != 3 {
        return None;
    }
    let time: Vec<i64> = parts[1]
        .split(':')
        .map(|piece| piece.parse::<i64>().ok())
        .collect::<Option<Vec<_>>>()?;
    if time.len() < 2 {
        return None;
    }
    let (day, month, year) = (date[0], date[1], date[2]);
    let (hour, minute) = (time[0], time[1]);
    let second = time.get(2).copied().unwrap_or(0);
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[allow(dead_code)]
/// Data civil (AAAA-MM-DD) a partir de dias desde a época.
pub fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_lines_become_a_segment_instead_of_being_dropped() {
        let body = "linha sem data\nalguma coisa solta\n";
        let segments = segment_file("a.txt", body, Some((1_700_000_000, TimeCertainty::FromMtime)));
        let joined: String = segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("solta"), "{joined}");
        assert_eq!(segments[0].certainty, TimeCertainty::FromMtime);
    }

    #[test]
    fn a_datetime_prefix_splits_segments_and_marks_exact_time() {
        let body = "[15/11/2025, 10:00:00] Ana: bom dia\n[15/11/2025, 10:01:00] Bia: oi\n";
        let segments = segment_file("d.txt", body, None);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].certainty, TimeCertainty::Exact);
        assert_eq!(segments[0].line_start, 1);
        assert!(segments[0].text.contains("bom dia"));
    }
}
