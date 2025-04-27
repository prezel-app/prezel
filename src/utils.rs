use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const LOWERCASE_PLUS_NUMBERS: [char; 30] = [
    '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'u', 'v', 'w', 'x', 'y', 'z',
];

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub(crate) fn now_in_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub(crate) trait PlusHttps {
    fn plus_https(&self) -> Self;
}

impl PlusHttps for String {
    fn plus_https(&self) -> Self {
        format!("https://{self}")
    }
}

pub(crate) trait LogError {
    fn log_error(self) -> Self;
    fn ignore_logging(self);
}

impl<T, E: std::fmt::Display> LogError for Result<T, E> {
    fn log_error(self) -> Self {
        self.inspect_err(|e| tracing::error!("{e}"))
    }

    fn ignore_logging(self) {
        let _ = self.inspect_err(|e| tracing::error!("{e}"));
    }
}
