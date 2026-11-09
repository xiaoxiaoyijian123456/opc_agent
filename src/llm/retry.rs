use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use std::time::Duration;

const MAX_RETRY_DELAY_SECONDS: u64 = 30;

pub fn is_retriable_status(status: reqwest::StatusCode) -> bool {
    let code = status.as_u16();
    code == 429 || (500..=599).contains(&code)
}

pub fn is_network_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_request()
}

pub fn retry_delay(attempt: u32, headers: &HeaderMap) -> Duration {
    let exponential = 1u64
        .checked_shl(attempt.saturating_sub(1))
        .unwrap_or(MAX_RETRY_DELAY_SECONDS)
        .min(MAX_RETRY_DELAY_SECONDS);
    let requested = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
        .unwrap_or(0);
    Duration::from_secs(exponential.max(requested).min(MAX_RETRY_DELAY_SECONDS))
}

fn parse_retry_after(value: &str) -> Option<u64> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(seconds);
    }
    let date = DateTime::parse_from_rfc2822(value).ok()?;
    let seconds = date.with_timezone(&Utc).timestamp() - Utc::now().timestamp();
    Some(seconds.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_respected_and_capped() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "60".parse().unwrap());
        assert_eq!(retry_delay(1, &headers), Duration::from_secs(30));
    }

    #[test]
    fn exponential_backoff_is_bounded() {
        assert_eq!(retry_delay(2, &HeaderMap::new()), Duration::from_secs(2));
        assert_eq!(retry_delay(10, &HeaderMap::new()), Duration::from_secs(30));
    }
}
