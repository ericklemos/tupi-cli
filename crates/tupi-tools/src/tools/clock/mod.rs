//! Relógio da sessão: data e hora correntes, em UTC e no fuso local.
//!
//! Sem esta tool o modelo só tem a data do treino à mão e ou inventa uma data
//! ou declara a data "em aberto". O relógio é do processo, não do prompt.

use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

const WEEKDAYS: [&str; 7] = [
    "domingo", "segunda", "terça", "quarta", "quinta", "sexta", "sábado",
];

const SECONDS_PER_DAY: i64 = 86_400;

/// Uma data/hora civil já resolvida (UTC ou no fuso local).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Civil {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl Civil {
    fn date_text(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn time_text(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    fn weekday(&self) -> &'static str {
        weekday_name(days_from_civil(self.year, self.month, self.day))
    }
}

/// Relógio da sessão: a resposta é o instante da chamada, não um valor do prompt.
#[derive(Debug, Default)]
pub struct ClockTool;

impl ClockTool {
    pub fn new() -> Self {
        Self
    }

    fn body(&self) -> String {
        let secs = unix_seconds();
        let utc = civil_from_epoch(secs);
        let mut body = String::new();
        match local_civil(secs) {
            Some(local) => body.push_str(&format!(
                "- local: {} {} · {} ({})\n",
                local.date_text(),
                local.weekday(),
                local.time_text(),
                offset_text(offset_secs(&local, &utc)),
            )),
            None => body.push_str("- local: indisponível neste alvo; use o horário UTC\n"),
        }
        body.push_str(&format!(
            "- UTC: {} {} · {}\n",
            utc.date_text(),
            utc.weekday(),
            utc.time_text(),
        ));
        body.push_str(&format!(
            "- epoch: {secs} segundos desde 1970-01-01T00:00:00Z\n"
        ));
        format!("### clock\n{body}")
    }
}

impl Tool for ClockTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "clock".to_string(),
            description: "Relógio da sessão: data e hora correntes em UTC e no fuso local. \
                          Sem argumentos."
                .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        }
    }

    fn call(&self, _arguments: Value) -> Result<String, ToolError> {
        Ok(self.body())
    }

    fn describe_call(&self, _arguments: &Value) -> String {
        "clock: agora".to_string()
    }

    fn sdd_allowed(&self) -> bool {
        true
    }
}

/// Segundos desde a epoch Unix; negativo se o relógio estiver antes de 1970.
fn unix_seconds() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(error) => -(error.duration().as_secs() as i64),
    }
}

fn civil_from_epoch(secs: i64) -> Civil {
    let days = secs.div_euclid(SECONDS_PER_DAY);
    let rem = secs.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: (rem / 3_600) as u32,
        minute: ((rem % 3_600) / 60) as u32,
        second: (rem % 60) as u32,
    }
}

/// Dias desde 1970-01-01 para a data civil (algoritmo de Howard Hinnant).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 1;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn weekday_name(days: i64) -> &'static str {
    // 1970-01-01 foi uma quinta-feira: o índice 4 fecha a conta.
    let index = (days + 4).rem_euclid(7) as usize;
    WEEKDAYS[index]
}

/// Diferença entre o horário local e o UTC, em segundos.
fn offset_secs(local: &Civil, utc: &Civil) -> i64 {
    let as_secs = |civil: &Civil| {
        days_from_civil(civil.year, civil.month, civil.day) * SECONDS_PER_DAY
            + civil.hour as i64 * 3_600
            + civil.minute as i64 * 60
            + civil.second as i64
    };
    as_secs(local) - as_secs(utc)
}

fn offset_text(offset: i64) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.abs();
    format!("{sign}{:02}:{:02}", abs / 3_600, (abs % 3_600) / 60)
}

#[cfg(unix)]
fn local_civil(secs: i64) -> Option<Civil> {
    let time = secs as libc::time_t;
    // `time_t` pode ser mais estreito que `i64`: fora do range o cast mente.
    if time as i64 != secs {
        return None;
    }
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `tm` é um buffer válido e `localtime_r` só escreve nele.
    let filled = unsafe { libc::localtime_r(&time, &mut tm) };
    if filled.is_null() {
        return None;
    }
    Some(Civil {
        year: tm.tm_year as i64 + 1900,
        month: tm.tm_mon as u32 + 1,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    })
}

#[cfg(not(unix))]
fn local_civil(_secs: i64) -> Option<Civil> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_civil_algorithms_round_trip() {
        for days in [-719_468, -1, 0, 1, 20_240, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn the_epoch_is_a_thursday() {
        let civil = civil_from_epoch(0);
        assert_eq!(civil.date_text(), "1970-01-01");
        assert_eq!(civil.time_text(), "00:00:00");
        assert_eq!(civil.weekday(), "quinta");
    }

    #[test]
    fn a_known_instant_matches_the_calendar() {
        // 2025-06-01T17:32:07Z, um domingo.
        let civil = civil_from_epoch(1_748_799_127);
        assert_eq!(civil.date_text(), "2025-06-01");
        assert_eq!(civil.time_text(), "17:32:07");
        assert_eq!(civil.weekday(), "domingo");
    }

    #[test]
    fn a_moment_before_the_epoch_is_not_dropped() {
        let civil = civil_from_epoch(-1);
        assert_eq!(civil.date_text(), "1969-12-31");
        assert_eq!(civil.time_text(), "23:59:59");
    }

    #[test]
    fn the_offset_is_written_with_sign_and_minutes() {
        assert_eq!(offset_text(0), "+00:00");
        assert_eq!(offset_text(-3 * 3_600), "-03:00");
        assert_eq!(offset_text(5 * 3_600 + 30 * 60), "+05:30");
    }

    #[test]
    fn the_call_reports_local_utc_and_epoch() {
        let out = ClockTool::new().call(serde_json::json!({})).unwrap();
        assert!(out.starts_with("### clock\n"), "{out}");
        assert!(out.contains("- UTC: "), "{out}");
        assert!(out.contains("- epoch: "), "{out}");
        assert!(out.contains("- local: "), "{out}");
    }
}
