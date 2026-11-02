//! Top bar, error banner, status bar, KPI cards, 100-cell hex matrix, and live stream panels.
use super::charts::{
    hex_digit_color, normality_histogram, speed_sparkline, HistogramMode, CLR_EMPTY_CELL,
    CLR_ERROR, CLR_INFO, CLR_MUTED, CLR_PRIMARY, CLR_SUCCESS, CLR_WARN,
};
use crate::app_state::LogEntry;
use crate::math::MAX_BATCH_DIGITS;
use crate::record::UiSnapshot;
use crate::view::{format_count, format_duration, pi_tail, ProgressInfo, Stats};
use egui::{Align2, Color32, FontId, Sense, Stroke, TextStyle};

const DIGIT_CELLS: usize = 100;
const DIGIT_GRID_COLS: usize = 10;
const DIGIT_CELL_SIZE: f32 = 22.0;
const COMPACT_PREVIEW_DIGITS: usize = 36;

pub struct Frame<'a> {
    pub snapshot: &'a UiSnapshot,
    pub stats: &'a Stats,
    pub paused: bool,
    pub running: bool,
    pub target: Option<u64>,
    pub fatal_error: Option<&'a str>,
    pub status: String,
    pub hex_digits: &'a str,
    pub decimal_digits: &'a str,
    pub hex_freq: &'a [u64; 16],
    pub dec_freq: &'a [u64; 10],
}

impl Frame<'_> {
    fn state_label(&self) -> (&'static str, Color32) {
        if self.fatal_error.is_some() {
            ("ERROR", CLR_ERROR)
        } else if !self.running {
            ("FINISHED", CLR_INFO)
        } else if self.paused {
            ("PAUSED", CLR_WARN)
        } else {
            ("RUNNING", CLR_SUCCESS)
        }
    }
}

#[derive(Default)]
pub struct TopBarActions {
    pub toggle_pause: bool,
    pub save: bool,
    pub exit: bool,
}

pub fn top_bar(ctx: &egui::Context, frame: &Frame<'_>) -> TopBarActions {
    let mut actions = TopBarActions::default();
    let paused = frame.paused;
    let (label, color) = frame.state_label();
    egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("π Pi Calculator");
            ui.separator();
            ui.colored_label(color, format!("[{label}]"));
            ui.colored_label(CLR_INFO, "[✓ MPFR Certified]").on_hover_text(
                "All hexadecimal digits are certified via directed-rounding MPFR interval arithmetic (bbp-mpfr-directed-v1).",
            );
            if let Some(target) = frame.target {
                ui.separator();
                ui.colored_label(CLR_WARN, format!("Target: {} hex", format_count(target)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                actions.exit = ui
                    .button("Exit")
                    .on_hover_text("Stop computation, save record, and exit (Ctrl+Q / Esc)")
                    .clicked();
                actions.save = ui
                    .button("Save")
                    .on_hover_text("Write atomic record snapshot immediately (Ctrl+S)")
                    .clicked();
                let toggle_text = if paused { "Resume" } else { "Pause" };
                actions.toggle_pause = ui
                    .add_enabled(frame.running, egui::Button::new(toggle_text))
                    .on_hover_text("Pause or resume computation (Space / Ctrl+P)")
                    .clicked();
            });
        });
    });
    actions
}

pub fn error_banner(ctx: &egui::Context, error: &str) {
    egui::TopBottomPanel::top("fatal_error_banner").show(ctx, |ui| {
        egui::Frame::none()
            .fill(Color32::from_rgb(70, 18, 24))
            .stroke(Stroke::new(1.5_f32, CLR_ERROR))
            .inner_margin(6.0)
            .rounding(4.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(CLR_ERROR, "⚠ COMPUTATION STOPPED:");
                    ui.label(error);
                    ui.colored_label(
                        CLR_MUTED,
                        "(The window remains open so you can inspect the error; click Exit or press Esc to close.)",
                    );
                });
            });
    });
}

pub fn status_bar(ctx: &egui::Context, frame: &Frame<'_>) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let (state, _) = frame.state_label();
            let dig_s = if frame.stats.last10_average_s > 0.0 {
                1.0 / frame.stats.last10_average_s
            } else {
                0.0
            };
            ui.label(format!(
                "Total: {} | Speed: {:.1} dig/s ({:.2} ms/dig) | Latest: D-{} | Status: {state}",
                format_duration(frame.snapshot.total_elapsed_s),
                dig_s,
                frame.stats.last10_average_s * 1_000.0,
                format_count(frame.snapshot.hex_count)
            ));
            if !frame.status.is_empty() {
                ui.separator();
                ui.colored_label(CLR_INFO, &frame.status);
            }
        });
    });
}

fn kpi_card(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    primary: &str,
    primary_color: Color32,
    subtitle: &str,
) {
    egui::Frame::group(ui.style())
        .fill(Color32::from_rgb(24, 27, 34))
        .inner_margin(6.0)
        .rounding(4.0)
        .show(ui, |ui| {
            ui.set_width((width - 14.0).max(60.0));
            ui.colored_label(CLR_MUTED, title);
            ui.colored_label(
                primary_color,
                egui::RichText::new(primary).monospace().strong().size(15.0),
            );
            ui.colored_label(CLR_MUTED, egui::RichText::new(subtitle).size(11.0));
        });
}

fn kpi_and_progress_panel(ui: &mut egui::Ui, frame: &Frame<'_>, width: f32) {
    let snap = frame.snapshot;
    let stats = frame.stats;
    let card_w = (width - ui.spacing().item_spacing.x) * 0.5;

    let recent_dig_s = if stats.last10_average_s > 0.0 {
        format!("{:.1} dig/s", 1.0 / stats.last10_average_s)
    } else {
        "—".to_string()
    };
    let speed_sub = format!(
        "Last 10: {:.2} ms | Avg: {:.2} ms",
        stats.last10_average_s * 1_000.0,
        stats.average_s * 1_000.0
    );
    let extrema_sub = if snap.hex_count > 0 {
        format!(
            "Fastest {:.2} ms | Slowest {:.2} ms",
            snap.fastest_s * 1_000.0,
            snap.slowest_s * 1_000.0
        )
    } else {
        "Fastest — | Slowest —".to_string()
    };

    ui.horizontal(|ui| {
        kpi_card(
            ui,
            card_w,
            "Certified Hex Digits",
            &format_count(snap.hex_count),
            CLR_WARN,
            &format!("Next index: #{}", format_count(snap.hex_count)),
        );
        kpi_card(
            ui,
            card_w,
            "Finalized Decimal Digits",
            &format_count(snap.decimal_count),
            CLR_SUCCESS,
            "Exact base-10 prefix",
        );
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        kpi_card(
            ui,
            card_w,
            "Throughput & Latency",
            &recent_dig_s,
            CLR_PRIMARY,
            &speed_sub,
        );
        kpi_card(
            ui,
            card_w,
            "Elapsed Time & Extrema",
            &format!(
                "{} (session {})",
                format_duration(snap.total_elapsed_s),
                format_duration(stats.session_s)
            ),
            CLR_INFO,
            &extrema_sub,
        );
    });
    ui.add_space(4.0);

    // Next Milestone / Target Progress Bar
    let progress = ProgressInfo::new(snap.hex_count, frame.target, stats);
    let kind = if progress.is_explicit_target {
        "Target"
    } else {
        "Next Milestone"
    };
    let eta_text = match progress.eta_s {
        Some(0.0) => "Completed".to_string(),
        Some(s) => format!("ETA ~{}", format_duration(s)),
        None => "ETA —".to_string(),
    };
    let bar_text = format!(
        "{kind}: {} / {} ({:.1}%) • {eta_text}",
        format_count(snap.hex_count),
        format_count(progress.target),
        progress.fraction * 100.0
    );
    ui.add(
        egui::ProgressBar::new(progress.fraction)
            .text(bar_text)
            .desired_width(width),
    );
}

fn digit_grid(ui: &mut egui::Ui, hex_display: &str, total_hex: u64, width: f32) {
    ui.group(|ui| {
        ui.set_width(width);
        let recent = &hex_display[hex_display.len().saturating_sub(DIGIT_CELLS)..];
        let count = recent.len();
        let first_idx = total_hex.saturating_sub(count as u64) + 1;

        ui.horizontal(|ui| {
            ui.strong("Recent 100 Hex Digit Matrix");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if count > 0 {
                    ui.colored_label(
                        CLR_MUTED,
                        format!(
                            "D-{} … D-{}",
                            format_count(first_idx),
                            format_count(total_hex)
                        ),
                    );
                } else {
                    ui.colored_label(CLR_MUTED, "Empty");
                }
            });
        });
        ui.add_space(3.0);

        let chars: Vec<char> = recent.chars().collect();
        let newest_start = count.saturating_sub(MAX_BATCH_DIGITS);
        let font = FontId::monospace(12.0);

        egui::Grid::new("hex_matrix_10x10")
            .spacing([4.0, 4.0])
            .min_col_width(DIGIT_CELL_SIZE)
            .show(ui, |ui| {
                for row in 0..(DIGIT_CELLS / DIGIT_GRID_COLS) {
                    for col in 0..DIGIT_GRID_COLS {
                        let cell_idx = row * DIGIT_GRID_COLS + col;
                        let (rect, response) = ui.allocate_exact_size(
                            egui::Vec2::splat(DIGIT_CELL_SIZE),
                            Sense::hover(),
                        );
                        if let Some(&ch) = chars.get(cell_idx) {
                            let bg = hex_digit_color(ch);
                            let is_latest_batch = cell_idx >= newest_start;
                            ui.painter().rect_filled(rect, 3.0, bg);
                            if is_latest_batch {
                                ui.painter().rect_stroke(
                                    rect,
                                    3.0,
                                    Stroke::new(1.3_f32, Color32::from_white_alpha(190)),
                                );
                            }
                            let text_color =
                                if matches!(ch.to_ascii_uppercase(), '1' | '6' | '7' | '8' | 'F') {
                                    Color32::from_rgb(15, 18, 24)
                                } else {
                                    Color32::WHITE
                                };
                            ui.painter().text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                ch.to_string(),
                                font.clone(),
                                text_color,
                            );
                            let d_idx = first_idx + cell_idx as u64;
                            let val = ch.to_digit(16).unwrap_or(0);
                            let batch_note = if is_latest_batch {
                                "\n• Certified in the latest batch"
                            } else {
                                ""
                            };
                            response.on_hover_text(format!(
                                "D-{} | Hex: {ch} (Decimal value: {val}){batch_note}",
                                format_count(d_idx)
                            ));
                        } else {
                            ui.painter().rect_filled(rect, 3.0, CLR_EMPTY_CELL);
                        }
                    }
                    ui.end_row();
                }
            });
    });
}

fn compact_pi_row(
    ui: &mut egui::Ui,
    label: &str,
    digits: &str,
    total: u64,
    color: Color32,
) -> Option<String> {
    let mut copied = None;
    let (is_start, tail) = pi_tail(digits, total, COMPACT_PREVIEW_DIGITS);
    let prefix = if is_start { "3." } else { "…" };
    let formatted = format!("{prefix}{tail}");
    ui.horizontal(|ui| {
        ui.colored_label(CLR_MUTED, egui::RichText::new(label).monospace());
        ui.colored_label(color, egui::RichText::new(&formatted).monospace());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("Copy")
                .on_hover_text("Copy displayed trailing digits to clipboard")
                .clicked()
            {
                copied = Some(formatted);
            }
        });
    });
    copied
}

fn pi_value_panel(ui: &mut egui::Ui, frame: &Frame<'_>, width: f32) -> Option<String> {
    let mut copied = None;
    ui.group(|ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.strong("Recent Digits Summary");
            ui.colored_label(CLR_MUTED, format!("(last {COMPACT_PREVIEW_DIGITS} digits)"));
        });
        ui.add_space(2.0);
        if let Some(c) = compact_pi_row(
            ui,
            "Hex:",
            frame.hex_digits,
            frame.snapshot.hex_count,
            CLR_WARN,
        ) {
            copied = Some(c);
        }
        if let Some(c) = compact_pi_row(
            ui,
            "Dec:",
            frame.decimal_digits,
            frame.snapshot.decimal_count,
            CLR_SUCCESS,
        ) {
            copied = Some(c);
        }
    });
    copied
}

fn log_row(ui: &mut egui::Ui, entry: &LogEntry) {
    ui.horizontal(|ui| {
        ui.colored_label(
            CLR_MUTED,
            egui::RichText::new(format!("D-{:<9}", format_count(entry.index))).monospace(),
        );
        ui.colored_label(CLR_MUTED, "│");
        ui.colored_label(
            CLR_WARN,
            egui::RichText::new(format!("H: {}", entry.hex)).monospace(),
        );
        ui.colored_label(CLR_MUTED, "│");
        let decimal = if entry.decimal.is_empty() {
            "-"
        } else {
            &entry.decimal
        };
        ui.colored_label(
            CLR_SUCCESS,
            egui::RichText::new(format!("Dec: {decimal:<3}")).monospace(),
        );
        ui.colored_label(CLR_MUTED, "│");
        let ms = entry.duration * 1_000.0;
        let dig_s = if entry.duration > 0.0 {
            1.0 / entry.duration
        } else {
            0.0
        };
        ui.colored_label(
            CLR_INFO,
            egui::RichText::new(format!("{ms:>7.3} ms ({dig_s:>6.1} dig/s)")).monospace(),
        );
    });
}

fn stream_panel(
    ui: &mut egui::Ui,
    logs: &[LogEntry],
    auto_scroll: &mut bool,
    width: f32,
    height: f32,
) {
    ui.group(|ui| {
        ui.set_width(width);
        ui.set_height(height.max(90.0));
        ui.horizontal(|ui| {
            ui.strong(format!("Live Computation Stream (Last {})", logs.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.checkbox(auto_scroll, "Auto-scroll");
            });
        });
        ui.add_space(2.0);
        let row_height = ui.text_style_height(&TextStyle::Monospace) + 4.0;
        egui::ScrollArea::vertical()
            .stick_to_bottom(*auto_scroll)
            .auto_shrink([false, false])
            .show_rows(ui, row_height, logs.len().max(1), |ui, row_range| {
                if logs.is_empty() {
                    ui.colored_label(CLR_MUTED, "Waiting for certified digit batch…");
                    return;
                }
                for idx in row_range {
                    if let Some(entry) = logs.get(idx) {
                        log_row(ui, entry);
                    }
                }
            });
    });
}

pub fn central_panel(
    ctx: &egui::Context,
    frame: &Frame<'_>,
    logs: &[LogEntry],
    auto_scroll: &mut bool,
    histogram_mode: &mut HistogramMode,
) -> Option<String> {
    let mut copied = None;
    egui::CentralPanel::default().show(ctx, |ui| {
        let total_width = ui.available_width();
        let left_width = (total_width * 0.43 - ui.spacing().item_spacing.x).max(320.0);
        let right_width = (total_width - left_width - ui.spacing().item_spacing.x).max(360.0);
        let height = ui.available_height();
        let column = egui::Layout::top_down(egui::Align::Min);

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(left_width, height), column, |ui| {
                kpi_and_progress_panel(ui, frame, left_width);
                ui.add_space(4.0);
                digit_grid(
                    ui,
                    &frame.snapshot.hex_display,
                    frame.snapshot.hex_count,
                    left_width,
                );
                ui.add_space(4.0);
                if let Some(text) = pi_value_panel(ui, frame, left_width) {
                    copied = Some(text);
                }
            });

            ui.allocate_ui_with_layout(egui::vec2(right_width, height), column, |ui| {
                speed_sparkline(ui, logs, right_width);
                ui.add_space(4.0);
                normality_histogram(
                    ui,
                    histogram_mode,
                    frame.hex_freq,
                    frame.snapshot.hex_count,
                    frame.dec_freq,
                    frame.snapshot.decimal_count,
                    right_width,
                );
                ui.add_space(4.0);
                let remaining_h = (ui.available_height() - 6.0).max(80.0);
                stream_panel(ui, logs, auto_scroll, right_width, remaining_h);
            });
        });
    });
    copied
}
