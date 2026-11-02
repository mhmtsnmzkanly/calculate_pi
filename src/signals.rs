//! Graceful shutdown handling for Unix signals (`SIGINT`, `SIGTERM`, `SIGHUP`).
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGTERM},
    iterator::Signals,
    low_level::emulate_default_handler,
};

/// The first `SIGINT`/`SIGTERM`/`SIGHUP` calls `request_stop` so the program can
/// perform a final save and shut down cleanly. A repeated signal terminates at once.
pub fn stop_on_signals(request_stop: impl Fn() + Send + 'static) -> std::io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    std::thread::spawn(move || {
        let mut stopping = false;
        for signal in signals.forever() {
            if stopping {
                let _ = emulate_default_handler(signal);
            }
            stopping = true;
            request_stop();
        }
    });
    Ok(())
}
