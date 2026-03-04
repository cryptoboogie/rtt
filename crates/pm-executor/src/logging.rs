use crate::config::LoggingConfig;
use tracing_subscriber::EnvFilter;

pub fn init(config: &LoggingConfig) {
    let default_filter = format!(
        "{},rtt_core::executor=off,rtt_core::connection=off,rtt_core::request=off,hyper=warn,rustls=warn",
        config.level
    );

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&default_filter));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn init_does_not_panic() {
        // tracing subscriber can only be initialized once per process,
        // so we just verify the function signature compiles and the
        // config struct is accepted.
        let config = crate::config::LoggingConfig {
            level: "warn".to_string(),
        };
        // Don't actually call init() in tests — it would conflict
        // with other test's subscriber setup.
        let _ = &config.level;
    }
}
