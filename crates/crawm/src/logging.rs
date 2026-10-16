//! Logging and crash handling.

use std::path::PathBuf;

use tracing_subscriber::EnvFilter;

/// Initialises `tracing`, honouring `filter` then `RUST_LOG`, then a sensible
/// default.
pub fn init(filter: Option<&str>) {
    let directives = filter
        .map(str::to_owned)
        .or_else(|| std::env::var("RUST_LOG").ok())
        .unwrap_or_else(|| "crawm=debug,info".to_owned());

    let env_filter = EnvFilter::try_new(&directives).unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(true)
        .init();
}

/// Installs a panic hook that records a backtrace under `$XDG_STATE_HOME/crawm/`.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        if let Some(path) = crash_log_path() {
            let report = format!("{info}\n\n{backtrace}\n");
            match std::fs::write(&path, report) {
                Ok(()) => eprintln!("crawm: panic; details written to {}", path.display()),
                Err(err) => eprintln!("crawm: panic; could not write {}: {err}", path.display()),
            }
        }
        previous(info);
    }));
}

fn crash_log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    let dir = base.join("crawm");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("crash.log"))
}
