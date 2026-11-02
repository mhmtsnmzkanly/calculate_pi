//! Terminal user interface (TUI) event loop and keyboard shortcut handling.
mod render;

use crate::{app_state::Shared, view::accumulate_hex_frequencies};
use crossterm::{
    cursor,
    event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, style, terminal,
};
use render::{RightTab, Screen};
use std::{
    error::Error,
    io,
    time::{Duration, Instant},
};

const INPUT_POLL: Duration = Duration::from_millis(100);

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(
            out,
            terminal::EnterAlternateScreen,
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        )?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(
            out,
            style::ResetColor,
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
        let _ = terminal::disable_raw_mode();
    }
}

fn handle_key(state: &Shared, tab: &mut RightTab, key: KeyEvent) -> io::Result<()> {
    if key.kind != KeyEventKind::Press {
        return Ok(());
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c' | 'q') => state.request_stop(),
            KeyCode::Char('s') => state.request_save(),
            KeyCode::Char('p') => state.toggle_pause()?,
            _ => {}
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Char(' ' | 'p' | 'P') => state.toggle_pause()?,
        KeyCode::Char('s' | 'S') => state.request_save(),
        KeyCode::Char('q' | 'Q') | KeyCode::Esc => state.request_stop(),
        KeyCode::Tab => *tab = tab.toggled(),
        KeyCode::Char('1') => *tab = RightTab::Stream,
        KeyCode::Char('2') => *tab = RightTab::Normality,
        _ => {}
    }
    Ok(())
}

pub fn run(state: Shared) -> Result<(), Box<dyn Error>> {
    let _guard = TerminalGuard::enter()?;
    let session_start = Instant::now();
    let mut frame = 0usize;
    let mut tab = RightTab::Stream;
    let mut hex_freq = [0u64; 16];
    let mut counted_hex_len = 0usize;

    while state.is_running() {
        if poll(INPUT_POLL)? {
            if let Event::Key(key) = read()? {
                handle_key(&state, &mut tab, key)?;
            }
        }
        let Some(snapshot) = ({
            let Some(core) = state.core() else {
                break;
            };
            if core.record.hex_digits.len() < counted_hex_len {
                hex_freq = [0; 16];
                counted_hex_len = 0;
            }
            if core.record.hex_digits.len() > counted_hex_len {
                accumulate_hex_frequencies(
                    &mut hex_freq,
                    &core.record.hex_digits[counted_hex_len..],
                );
                counted_hex_len = core.record.hex_digits.len();
            }
            Some(core.ui_snapshot_with_limits(render::HEX_STRIP_WIDTH, render::PI_DIGITS_WIDTH))
        }) else {
            break;
        };
        let screen = Screen {
            snapshot,
            recent_logs: state.recent_logs(),
            hex_freq,
            target: state.target(),
            tab,
            paused: state.is_paused(),
            session_start,
            status: state.status_text(),
        };
        render::draw(&screen, frame)?;
        frame = frame.wrapping_add(1);
    }
    Ok(())
}
