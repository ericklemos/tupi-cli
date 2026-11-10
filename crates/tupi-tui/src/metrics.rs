use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyMetrics {
    pub started: Instant,
    pub first_token: Option<Duration>,
    pub finished: Option<Duration>,
    pub estimated_tokens: u64,
    pub remote_tokens: Option<u64>,
}

impl ReplyMetrics {
    pub fn start(started: Instant) -> Self {
        Self {
            started,
            first_token: None,
            finished: None,
            estimated_tokens: 0,
            remote_tokens: None,
        }
    }

    pub fn mark_first_token(&mut self, at: Instant) {
        if self.first_token.is_none() {
            self.first_token = Some(at.saturating_duration_since(self.started));
        }
    }

    pub fn add_estimated(&mut self, tokens: u64) {
        self.estimated_tokens += tokens;
    }

    pub fn set_remote(&mut self, tokens: u64) {
        self.remote_tokens = Some(tokens);
    }

    pub fn finish(&mut self, at: Instant) {
        if self.finished.is_none() {
            self.finished = Some(at.saturating_duration_since(self.started));
        }
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        self.finished
            .unwrap_or_else(|| now.saturating_duration_since(self.started))
    }

    pub fn header(&self, now: Instant, compact: bool) -> String {
        let mut parts = vec![format_duration(self.elapsed(now))];

        if !compact
            && self.finished.is_some()
            && let Some(first) = self.first_token
        {
            parts.push(format!("1º {}", format_duration(first)));
        }

        if let Some(from) = self.first_token {
            let to = self
                .finished
                .unwrap_or_else(|| now.saturating_duration_since(self.started));

            if let Some(estimated) = rate(self.estimated_tokens, from, to) {
                if self.finished.is_some() {
                    match self.remote_tokens.and_then(|remote| rate(remote, from, to)) {
                        Some(remote) if compact => parts.push(format!("≈{estimated}→{remote} t/s")),
                        Some(remote) => parts.push(format!("≈{estimated} → {remote} tok/s")),
                        None if compact => parts.push(format!("≈{estimated} t/s")),
                        None => parts.push(format!("≈{estimated} tok/s (est.)")),
                    }
                } else {
                    parts.push(format!("≈{estimated} t/s"));
                }
            }
        }

        parts.join(" · ")
    }
}

fn rate(tokens: u64, from: Duration, to: Duration) -> Option<u64> {
    let window = to.checked_sub(from)?;
    if window < Duration::from_millis(1) {
        return None;
    }
    Some((tokens as f64 / window.as_secs_f64()).round() as u64)
}

pub fn estimate_tokens(text: &str) -> u64 {
    text.split_whitespace()
        .map(|word| {
            let chars = word.chars().count() as u64;
            chars.div_ceil(4).max(1)
        })
        .sum()
}

pub fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let secs = duration.as_secs();
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{:.1}s", duration.as_secs_f64()).replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn estimate_counts_words_in_quarter_chunks() {
        assert_eq!(estimate_tokens("olá, mundo"), 3);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
    }

    #[test]
    fn duration_switches_from_seconds_to_minutes() {
        assert_eq!(format_duration(Duration::from_millis(12_400)), "12,4s");
        assert_eq!(format_duration(Duration::from_secs(63)), "1m03s");
    }

    #[test]
    fn header_shows_estimate_and_remote_after_finish() {
        let t0 = Instant::now();
        let mut metrics = ReplyMetrics::start(t0);
        metrics.add_estimated(340);
        metrics.mark_first_token(t0 + Duration::from_secs(1));
        metrics.finish(t0 + Duration::from_secs(11));
        metrics.set_remote(320);

        assert_eq!(
            metrics.header(t0 + Duration::from_secs(11), false),
            "11,0s · 1º 1,0s · ≈34 → 32 tok/s"
        );
    }

    #[test]
    fn header_without_usage_uses_estimate() {
        let t0 = Instant::now();
        let mut metrics = ReplyMetrics::start(t0);
        metrics.add_estimated(340);
        metrics.mark_first_token(t0 + Duration::from_secs(1));
        metrics.finish(t0 + Duration::from_secs(11));

        assert_eq!(
            metrics.header(t0 + Duration::from_secs(11), false),
            "11,0s · 1º 1,0s · ≈34 tok/s (est.)"
        );
    }

    #[test]
    fn compact_header_uses_arrows_without_labels() {
        let t0 = Instant::now();
        let mut metrics = ReplyMetrics::start(t0);
        metrics.add_estimated(340);
        metrics.mark_first_token(t0 + Duration::from_secs(1));
        metrics.finish(t0 + Duration::from_secs(11));
        metrics.set_remote(320);

        assert_eq!(
            metrics.header(t0 + Duration::from_secs(11), true),
            "11,0s · ≈34→32 t/s"
        );
    }

    #[test]
    fn header_is_live_while_streaming() {
        let t0 = Instant::now();
        let mut metrics = ReplyMetrics::start(t0);
        metrics.add_estimated(96);
        metrics.mark_first_token(t0 + Duration::from_millis(200));

        assert_eq!(
            metrics.header(t0 + Duration::from_millis(3_200), false),
            "3,2s · ≈32 t/s"
        );
    }

    #[test]
    fn rate_is_omitted_for_a_very_short_window() {
        let t0 = Instant::now();
        let mut metrics = ReplyMetrics::start(t0);
        metrics.mark_first_token(t0 + Duration::from_secs(1));
        metrics.finish(t0 + Duration::from_secs(1));

        assert!(
            !metrics
                .header(t0 + Duration::from_secs(1), false)
                .contains("t/s")
        );
    }
}
