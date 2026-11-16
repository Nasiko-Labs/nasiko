//! How much of a process's `tracing` output goes to an error tracker's log search
//! (`SENTRY_LOGS_LEVEL`), shared by every binary that reports to one.

/// The lowest level sent as logs. Each level includes the ones above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogsLevel {
    Off,
    Error,
    Warn,
    Info,
}

impl LogsLevel {
    /// `off`, `error`, `warn` or `info`, any case.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Self::Off),
            "error" => Some(Self::Error),
            "warn" | "warning" => Some(Self::Warn),
            "info" => Some(Self::Info),
            _ => None,
        }
    }

    /// Whether a `tracing` line at `level` is sent as a log.
    pub fn includes(self, level: &tracing::Level) -> bool {
        let needed = match *level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            _ => return false,
        };
        self != Self::Off && self >= needed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_level_includes_the_ones_above_it() {
        assert!(LogsLevel::Warn.includes(&tracing::Level::ERROR));
        assert!(LogsLevel::Warn.includes(&tracing::Level::WARN));
        assert!(!LogsLevel::Warn.includes(&tracing::Level::INFO));
        assert!(LogsLevel::Info.includes(&tracing::Level::INFO));
        assert!(!LogsLevel::Info.includes(&tracing::Level::DEBUG));
        assert!(!LogsLevel::Off.includes(&tracing::Level::ERROR));
    }

    #[test]
    fn parsing() {
        assert_eq!(LogsLevel::parse(" WARNING "), Some(LogsLevel::Warn));
        assert_eq!(LogsLevel::parse("none"), Some(LogsLevel::Off));
        assert_eq!(LogsLevel::parse("loud"), None);
    }
}
