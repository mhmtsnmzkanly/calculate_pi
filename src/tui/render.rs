//! Terminal user interface (TUI) rendering and frame layout module.
use crate::{
    app_state::LogEntry,
    record::UiSnapshot,
    view::{format_count, format_duration, pi_tail, ProgressInfo, Stats},
};
use crossterm::cursor::MoveTo;
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use std::io::Write;
use std::time::Instant;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

const CLR_PRIMARY: Color = rgb(0, 180, 216);
const CLR_SUCCESS: Color = rgb(128, 237, 153);
const CLR_WARN: Color = rgb(255, 183, 3);
const CLR_INFO: Color = rgb(144, 224, 239);
const CLR_TEXT: Color = rgb(248, 249, 250);
const CLR_MUTED: Color = rgb(140, 148, 156);

const ANIMATION_COLORS: [Color; 6] = [
    rgb(0, 180, 216),
    rgb(72, 202, 228),
    rgb(128, 237, 153),
    rgb(255, 183, 3),
    rgb(251, 133, 0),
    rgb(187, 134, 252),
];

const LEFT_WIDTH: usize = 38;
const RIGHT_WIDTH: usize = 39;
const PANEL_ROWS: usize = 22;
const LOG_ROWS: usize = PANEL_ROWS - 1;
pub(super) const PI_DIGITS_WIDTH: usize = 29;
pub(super) const HEX_STRIP_WIDTH: usize = 35;
const STATUS_WIDTH: usize = 36;
const SPARK_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RightTab {
    #[default]
    Stream,
    Normality,
}

impl RightTab {
    pub fn toggled(self) -> Self {
        match self {
            Self::Stream => Self::Normality,
            Self::Normality => Self::Stream,
        }
    }
}

pub struct Screen {
    pub snapshot: UiSnapshot,
    pub recent_logs: Vec<LogEntry>,
    pub hex_freq: [u64; 16],
    pub target: Option<u64>,
    pub tab: RightTab,
    pub paused: bool,
    pub session_start: Instant,
    pub status: String,
}

/// Truncates or right-pads `text` with spaces to an exact Unicode character width (`chars().count()`).
fn fit_text(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count == width {
        text.to_string()
    } else if count > width {
        let mut s: String = text.chars().take(width.saturating_sub(1)).collect();
        if width > 0 {
            s.push('…');
        }
        s
    } else {
        let mut s = String::with_capacity(text.len() + (width - count));
        s.push_str(text);
        s.extend(std::iter::repeat_n(' ', width - count));
        s
    }
}

fn format_pi_line(prefix: &str, digits: &str, total: u64, color: Color) -> String {
    // Only the true start of π is shown with "3."; truncated tails use "… ".
    let (is_start, shown) = pi_tail(digits, total, PI_DIGITS_WIDTH);
    let lead = if is_start { "3." } else { "… " };
    let prefix_cell = fit_text(prefix, 5);
    let shown_cell = fit_text(shown, PI_DIGITS_WIDTH);
    format!(
        "  {}{prefix_cell}{lead}{}{shown_cell}{}",
        SetForegroundColor(CLR_TEXT),
        SetForegroundColor(color),
        ResetColor
    )
}

fn format_stat_line(label: &str, value: &str, value_color: Color) -> String {
    let label_cell = fit_text(label, 15);
    let value_cell = fit_text(value, 19);
    format!(
        "  {}{label_cell}{}: {}{value_cell}{}",
        SetForegroundColor(CLR_TEXT),
        SetForegroundColor(CLR_MUTED),
        SetForegroundColor(value_color),
        ResetColor
    )
}

fn unicode_bar(fraction: f32, width: usize) -> String {
    let filled = ((fraction.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    let mut bar = String::with_capacity(width * 3);
    bar.extend(std::iter::repeat_n('█', filled));
    bar.extend(std::iter::repeat_n('░', width - filled));
    bar
}

fn latency_sparkline(times: &[f64], width: usize) -> String {
    if times.is_empty() {
        return fit_text("Waiting for data…", width);
    }
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    let max = times.iter().copied().fold(0.0_f64, f64::max);
    let span = (max - min).max(1e-9);
    let mut out = String::new();
    for &t in times.iter().rev().take(width).rev() {
        let idx = (((t - min) / span) * 7.0).round().clamp(0.0, 7.0) as usize;
        out.push(SPARK_CHARS[idx]);
    }
    let avg_ms = (times.iter().sum::<f64>() / times.len() as f64) * 1_000.0;
    let combined = format!("{out} {avg_ms:.1}ms");
    fit_text(&combined, width)
}

fn format_log_entry(entry: &LogEntry) -> String {
    let muted = SetForegroundColor(CLR_MUTED);
    let decimal = if entry.decimal.is_empty() {
        "-"
    } else {
        &entry.decimal
    };
    let index = fit_text(&format!(" > D-{}", entry.index), 11);
    let decimal = fit_text(decimal, 2);
    let time = fit_text(&format!("{:.3}s", entry.duration), 6);
    // Visible width: fields + " │ H: x │ D: " (12) + " │ " (3) = 11 + 1 + 2 + 6 + 15 = 35
    let visible = 35;
    let padding = " ".repeat(RIGHT_WIDTH.saturating_sub(visible));
    format!(
        "{muted}{index} │ H: {}{}{muted} │ D: {}{decimal}{muted} │ {}{time}{padding}{}",
        SetForegroundColor(CLR_WARN),
        entry.hex,
        SetForegroundColor(CLR_SUCCESS),
        SetForegroundColor(CLR_INFO),
        ResetColor
    )
}

fn fit_status(message: &str) -> String {
    fit_text(message, STATUS_WIDTH)
}

fn left_panel(screen: &Screen, frame: usize) -> Vec<String> {
    let snap = &screen.snapshot;
    let stats = Stats::new(snap, screen.session_start);
    let progress = ProgressInfo::new(snap.hex_count, screen.target, &stats);
    let primary = SetForegroundColor(CLR_PRIMARY);
    let reset = ResetColor;
    let divider = format!("├{}┤", "─".repeat(LEFT_WIDTH - 2));

    let c0 = SetForegroundColor(ANIMATION_COLORS[frame % ANIMATION_COLORS.len()]);
    let c1 = SetForegroundColor(ANIMATION_COLORS[(frame + 1) % ANIMATION_COLORS.len()]);
    let c2 = SetForegroundColor(ANIMATION_COLORS[(frame + 2) % ANIMATION_COLORS.len()]);

    let mut lines = vec![
        format!(
            "  {c0}████████████{reset}  {}{}{reset}",
            SetForegroundColor(CLR_TEXT),
            fit_text("π Pi Calculator", 22)
        ),
        format!(
            "  {c1}    ██    ██{reset}  {}{}{reset}",
            SetForegroundColor(CLR_INFO),
            fit_text("[✓ MPFR Certified]", 22)
        ),
        format!(
            "  {c2}  ▄▄██  ▄▄██{reset}  {}{}{reset}",
            SetForegroundColor(CLR_MUTED),
            fit_text("bbp-mpfr-directed-v1", 22)
        ),
    ];

    lines.push(divider.clone());
    lines.push(format!(
        "  {primary}{}{reset}",
        fit_text("── [ Pi Value ] ─────────────────────", LEFT_WIDTH - 2)
    ));
    lines.push(format_pi_line(
        "Hex:",
        &snap.hex_display,
        snap.hex_count,
        CLR_WARN,
    ));
    lines.push(format_pi_line(
        "Dec:",
        &snap.decimal_display,
        snap.decimal_count,
        CLR_SUCCESS,
    ));
    lines.push(divider.clone());
    lines.push(format!(
        "  {primary}{}{reset}",
        fit_text("── [ Statistics & Progress ] ────────", LEFT_WIDTH - 2)
    ));

    let (state_label, state_color) = if screen.paused {
        ("PAUSED", CLR_WARN)
    } else {
        ("RUNNING", CLR_SUCCESS)
    };
    let dig_s = if stats.last10_average_s > 0.0 {
        1.0 / stats.last10_average_s
    } else {
        0.0
    };
    let speed_str = format!(
        "{:.1} dig/s ({:.1}ms)",
        dig_s,
        stats.last10_average_s * 1_000.0
    );
    let eta_str = match progress.eta_s {
        Some(0.0) => "Done".to_string(),
        Some(s) => format_duration(s),
        None => "—".to_string(),
    };
    let bar_str = format!(
        "{} {:>4.1}% {}",
        unicode_bar(progress.fraction, 6),
        progress.fraction * 100.0,
        eta_str
    );
    let target_label = if progress.is_explicit_target {
        "Target (--until)"
    } else {
        "Next Milestone"
    };

    let rows = [
        ("Hex Digits", format_count(snap.hex_count), CLR_WARN),
        (
            "Decimal Digits",
            format_count(snap.decimal_count),
            CLR_SUCCESS,
        ),
        ("Instant Speed", speed_str, CLR_WARN),
        (
            "Average Time",
            format!("{:.4} s/dig", stats.average_s),
            CLR_TEXT,
        ),
        (
            "Total / Session",
            format!(
                "{} / {}",
                format_duration(snap.total_elapsed_s),
                format_duration(stats.session_s)
            ),
            CLR_INFO,
        ),
        (
            "Fastest/Slowest",
            format!("{:.3}s / {:.3}s", snap.fastest_s, snap.slowest_s),
            CLR_SUCCESS,
        ),
        (
            target_label,
            format!("{} hex", format_count(progress.target)),
            CLR_INFO,
        ),
        ("Progress / ETA", bar_str, CLR_PRIMARY),
        (
            "Latency (Last10)",
            latency_sparkline(&snap.digit_times, 19),
            CLR_INFO,
        ),
        ("Status", state_label.to_string(), state_color),
    ];
    lines.extend(
        rows.iter()
            .map(|(label, value, color)| format_stat_line(label, value, *color)),
    );
    lines.push(divider);
    lines.push(format!(
        "  {}{}{reset}",
        SetForegroundColor(CLR_MUTED),
        fit_text("[Space/P] Pause  [S] Save  [Tab] View", LEFT_WIDTH - 2)
    ));
    lines.push(format!(
        "  {}{}{reset}",
        SetForegroundColor(CLR_INFO),
        fit_status(&screen.status)
    ));
    lines
}

fn right_tab_header(active: RightTab) -> String {
    let (c1, c2) = match active {
        RightTab::Stream => (CLR_WARN, CLR_MUTED),
        RightTab::Normality => (CLR_MUTED, CLR_WARN),
    };
    let t1 = fit_text(" [1:Live Stream]", 18);
    let t2 = fit_text(" [2:Normality/Matrix]", 21);
    format!(
        "{}{t1}{}{t2}{}",
        SetForegroundColor(c1),
        SetForegroundColor(c2),
        ResetColor
    )
}

fn normality_panel(screen: &Screen) -> Vec<String> {
    let mut lines = vec![right_tab_header(screen.tab)];
    let snap = &screen.snapshot;
    let reset = ResetColor;

    lines.push(format!(
        "{}{}{reset}",
        SetForegroundColor(CLR_TEXT),
        fit_text("  Recent Hex Strip (Live):", RIGHT_WIDTH)
    ));

    let (_, tail) = pi_tail(&snap.hex_display, snap.hex_count, HEX_STRIP_WIDTH);
    let mut strip = String::from("  ");
    for (i, ch) in tail.chars().enumerate() {
        let color = ANIMATION_COLORS[(ch as usize + i) % ANIMATION_COLORS.len()];
        strip.push_str(&format!("{}{ch}", SetForegroundColor(color)));
    }
    let pad = RIGHT_WIDTH.saturating_sub(2 + tail.chars().count());
    strip.push_str(&format!("{reset}{}", " ".repeat(pad)));
    lines.push(strip);

    lines.push(format!(
        "{}{}{reset}",
        SetForegroundColor(CLR_INFO),
        fit_text("  Hex Distribution (0-F, Exp. 6.25%)", RIGHT_WIDTH)
    ));
    lines.push(format!(
        "{}  {}  {reset}",
        SetForegroundColor(CLR_MUTED),
        "─".repeat(RIGHT_WIDTH - 4)
    ));

    let total = snap.hex_count.max(1);
    let max_count = screen.hex_freq.iter().copied().max().unwrap_or(1).max(1);
    for (digit, &count) in screen.hex_freq.iter().enumerate() {
        let ch = char::from_digit(digit as u32, 16)
            .unwrap_or('?')
            .to_ascii_uppercase();
        let pct = if snap.hex_count > 0 {
            count as f64 * 100.0 / total as f64
        } else {
            0.0
        };
        let bar = unicode_bar(count as f32 / max_count as f32, 10);
        let text = format!("  {ch}: {bar} {pct:>5.2}% ({count:>8})");
        let color = ANIMATION_COLORS[digit % ANIMATION_COLORS.len()];
        lines.push(format!(
            "{}{}{reset}",
            SetForegroundColor(color),
            fit_text(&text, RIGHT_WIDTH)
        ));
    }

    let summary = format!(
        "  Total: {} hex │ [Tab]: Switch View",
        format_count(snap.hex_count)
    );
    lines.push(format!(
        "{}{}{reset}",
        SetForegroundColor(CLR_MUTED),
        fit_text(&summary, RIGHT_WIDTH)
    ));
    lines
}

fn right_panel(screen: &Screen) -> Vec<String> {
    if screen.tab == RightTab::Normality {
        return normality_panel(screen);
    }
    let mut lines = vec![right_tab_header(screen.tab)];
    lines.extend((0..LOG_ROWS).map(|row| match screen.recent_logs.get(row) {
        Some(entry) => format_log_entry(entry),
        None => " ".repeat(RIGHT_WIDTH),
    }));
    lines
}

/// Composes the full frame without a trailing `\r\n` on the bottom row so a standard 24-row terminal never scrolls.
fn compose(screen: &Screen, frame: usize) -> String {
    let border = SetForegroundColor(CLR_PRIMARY);
    let reset = ResetColor;
    let left = left_panel(screen, frame);
    let right = right_panel(screen);
    debug_assert_eq!(left.len(), PANEL_ROWS);
    debug_assert_eq!(right.len(), PANEL_ROWS);

    let mut text = format!(
        "{}{border}┌{}┬{}┐{reset}\r\n",
        MoveTo(0, 0),
        "─".repeat(LEFT_WIDTH),
        "─".repeat(RIGHT_WIDTH)
    );
    for (left_line, right_line) in left.iter().zip(&right) {
        text.push_str(&format!(
            "{border}│{left_line}{border}│{right_line}{border}│{reset}\r\n"
        ));
    }
    let footer_label = "─ Space/P: Pause ─ S: Save ─ Tab: View ─ Q/Esc: Quit ";
    let footer_pad = (LEFT_WIDTH + RIGHT_WIDTH + 1).saturating_sub(footer_label.chars().count());
    text.push_str(&format!(
        "{border}└{footer_label}{}┘{reset}",
        "─".repeat(footer_pad)
    ));
    text
}

pub fn draw(screen: &Screen, frame: usize) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(compose(screen, frame).as_bytes())?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn screen(status: &str, digits: usize, tab: RightTab) -> Screen {
        let logs = (0..21)
            .map(|index| LogEntry {
                index: 100_000 + index,
                hex: 'A',
                decimal: "12".to_string(),
                duration: 0.001,
            })
            .collect();
        Screen {
            snapshot: UiSnapshot {
                hex_count: digits as u64,
                decimal_count: digits as u64,
                total_elapsed_s: 1.0,
                fastest_s: 0.1,
                slowest_s: 0.2,
                hex_display: "A".repeat(digits),
                decimal_display: "1".repeat(digits),
                digit_times: vec![0.1, 0.15, 0.12],
            },
            recent_logs: logs,
            hex_freq: [digits as u64 / 16; 16],
            target: Some(1_000),
            tab,
            paused: false,
            session_start: Instant::now(),
            status: status.to_string(),
        }
    }

    #[test]
    fn every_row_has_the_same_width() {
        for tab in [RightTab::Stream, RightTab::Normality] {
            for (status, digits) in [("", 3), (&*"é".repeat(80), 200)] {
                let text = visible(&compose(&screen(status, digits, tab), 0));
                let rows: Vec<&str> = text.split("\r\n").collect();
                assert_eq!(rows.len(), PANEL_ROWS + 2);
                for row in rows {
                    assert_eq!(row.chars().count(), LEFT_WIDTH + RIGHT_WIDTH + 3, "{row}");
                }
            }
        }
    }

    #[test]
    fn long_status_stays_inside_the_frame() {
        assert_eq!(fit_status("Saved").chars().count(), STATUS_WIDTH);
        let fitted = fit_status(&"é".repeat(80));
        assert_eq!(fitted.chars().count(), STATUS_WIDTH);
        assert!(fitted.ends_with('…'));
    }

    #[test]
    fn pi_line_marks_only_the_real_start_as_three_point() {
        assert!(format_pi_line("Hex:", "243F", 4, CLR_WARN).contains("3."));
        let tail = format_pi_line("Hex:", &"A".repeat(30), 30, CLR_WARN);
        assert!(tail.contains('…'));
        assert!(!tail.contains("3."));
        let presliced_tail = format_pi_line("Hex:", &"A".repeat(PI_DIGITS_WIDTH), 30, CLR_WARN);
        assert!(presliced_tail.contains('…'));
        assert!(!presliced_tail.contains("3."));
    }
}
