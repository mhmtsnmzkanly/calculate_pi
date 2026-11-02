//! Application entry point and command-line action dispatcher.
use pi_hesaplayici::{
    app_state::SharedState,
    bellard::bellard_hex_digits,
    cli::{self, CliAction, Mode},
    headless,
    lock::OutputLock,
    math::certified_hex_digits,
    storage::load_record,
    tui,
    verify::{reverify_v1, spot_check, verify_digits, Coverage},
    worker::WorkerHandle,
};
use std::{
    error::Error,
    path::Path,
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

const VERIFY_PROGRESS_INTERVAL: Duration = Duration::from_secs(10);

fn main() -> ExitCode {
    match dispatch() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch() -> Result<(), Box<dyn Error>> {
    let action = cli::parse(std::env::args().skip(1)).map_err(|e| format!("CLI: {e}"))?;
    match action {
        CliAction::Help => println!("{}", cli::HELP),
        CliAction::Query { index, count } => query(index, count)?,
        CliAction::Reverify { input, output } => {
            let cancelled = cancel_flag()?;
            reverify_v1(&input, &output, &|| cancelled.load(Ordering::Acquire))?;
        }
        CliAction::Verify { input, coverage } => verify(&input, coverage)?,
        CliAction::Run {
            mode,
            output,
            until,
        } => run(mode, output, until)?,
    }
    Ok(())
}

/// A flag raised by shutdown signals for commands that run without a worker thread.
fn cancel_flag() -> std::io::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        let flag = Arc::clone(&flag);
        pi_hesaplayici::signals::stop_on_signals(move || flag.store(true, Ordering::Release))?;
    }
    Ok(flag)
}

fn query(index: u64, count: usize) -> Result<(), Box<dyn Error>> {
    let cancelled = cancel_flag()?;
    let cancel = || cancelled.load(Ordering::Acquire);
    let start = Instant::now();
    let mut bbp_digits = Vec::with_capacity(count);
    while bbp_digits.len() < count {
        let offset = index
            .checked_add(bbp_digits.len() as u64)
            .ok_or("query index overflow")?;
        let chunk = certified_hex_digits(offset, count - bbp_digits.len(), &cancel)?;
        bbp_digits.extend(chunk);
    }
    bbp_digits.truncate(count);

    let mut bellard_digits = Vec::with_capacity(count);
    while bellard_digits.len() < count {
        let offset = index
            .checked_add(bellard_digits.len() as u64)
            .ok_or("query index overflow")?;
        let chunk = bellard_hex_digits(offset, count - bellard_digits.len(), &cancel)?;
        bellard_digits.extend(chunk);
    }
    bellard_digits.truncate(count);

    if bbp_digits != bellard_digits {
        return Err(format!(
            "BBP and Bellard disagreed at index {index}: BBP={bbp_digits:?}, Bellard={bellard_digits:?}"
        )
        .into());
    }
    let hex_str: String = bbp_digits
        .iter()
        .map(|&d| {
            char::from_digit(u32::from(d), 16)
                .unwrap_or('?')
                .to_ascii_uppercase()
        })
        .collect();
    let ms = start.elapsed().as_secs_f64() * 1_000.0;
    println!("Index {index} (hex): {hex_str} (certified by BBP + Bellard in {ms:.2} ms)");
    Ok(())
}

fn verify(input: &str, coverage: Coverage) -> Result<(), Box<dyn Error>> {
    if !Path::new(input).exists() {
        return Err(format!("record {input} does not exist").into());
    }
    let (record, _) = load_record(input)?;
    let cancelled = cancel_flag()?;
    let mut last_report = Instant::now();
    let report = verify_digits(
        &record.hex_digits,
        coverage,
        &|| cancelled.load(Ordering::Acquire),
        &mut |checked, total| {
            if last_report.elapsed() >= VERIFY_PROGRESS_INTERVAL {
                last_report = Instant::now();
                println!("{checked}/{total}");
            }
        },
    )?;
    println!(
        "OK: verified {} of {} hex digits with Bellard's formula",
        report.checked,
        record.hex_digits.len()
    );
    Ok(())
}

fn run(mode: Mode, output: String, until: Option<u64>) -> Result<(), Box<dyn Error>> {
    let _output_lock = OutputLock::acquire(&output)?;
    let (record, converter) = load_record(&output)?;
    spot_check(&record.hex_digits).map_err(|e| format!("record {output} rejected: {e}"))?;
    let state = SharedState::new(record, converter, output);
    #[cfg(unix)]
    {
        let state = state.clone();
        pi_hesaplayici::signals::stop_on_signals(move || state.request_stop())?;
    }

    let worker = WorkerHandle::start(state.clone(), until);
    let ui_result = match mode {
        #[cfg(feature = "gui")]
        Mode::Gui => pi_hesaplayici::gui::run_gui(state).map_err(|e| e.to_string()),
        Mode::Tui => tui::run(state).map_err(|e| e.to_string()),
        Mode::Headless => {
            headless::run(state);
            Ok(())
        }
    };
    worker.stop();
    let worker_result = worker.join().map_err(|e| e.to_string());
    match (ui_result, worker_result) {
        (Ok(()), Ok(())) => {
            println!("Exiting.");
            Ok(())
        }
        (Err(ui), Ok(())) => Err(format!("UI error: {ui}").into()),
        (Ok(()), Err(worker)) => Err(format!("worker error: {worker}").into()),
        (Err(ui), Err(worker)) => Err(format!("UI error: {ui}; worker error: {worker}").into()),
    }
}
