//! Desktop GUI application state, keyboard shortcuts, and frame update loop.
use super::{
    charts::HistogramMode,
    explorer::ExplorerState,
    panels::{self, Frame},
};
use crate::app_state::{LogEntry, Shared};
use crate::record::UiSnapshot;
use crate::view::{accumulate_decimal_frequencies, accumulate_hex_frequencies, pi_preview, Stats};
use egui::{Key, Modifiers};
use std::time::{Duration, Instant};

const MAX_LOCAL_LOGS: usize = 100;
const REPAINT_INTERVAL: Duration = Duration::from_millis(50);
const PREVIEW_TAIL_LIMIT: usize = 10_000;

pub struct GuiApp {
    state: Shared,
    session_start: Instant,
    last_snapshot: UiSnapshot,
    last_counts: (u64, u64),
    hex_digits: String,
    decimal_digits: String,
    hex_preview: String,
    decimal_preview: String,
    hex_freq: [u64; 16],
    dec_freq: [u64; 10],
    histogram_mode: HistogramMode,
    explorer: ExplorerState,
    logs: Vec<LogEntry>,
    auto_scroll: bool,
    exit_requested: bool,
}

struct Commands {
    toggle_pause: bool,
    save: bool,
    exit: bool,
}

fn read_shortcuts(ctx: &egui::Context) -> Commands {
    let typing = ctx.wants_keyboard_input();
    ctx.input_mut(|input| Commands {
        toggle_pause: (!typing && input.consume_key(Modifiers::NONE, Key::Space))
            | input.consume_key(Modifiers::CTRL, Key::P),
        save: input.consume_key(Modifiers::CTRL, Key::S),
        exit: input.consume_key(Modifiers::CTRL, Key::Q)
            | (!typing && input.consume_key(Modifiers::NONE, Key::Escape)),
    })
}

impl GuiApp {
    pub fn new(state: Shared) -> Self {
        Self {
            state,
            session_start: Instant::now(),
            last_snapshot: UiSnapshot {
                hex_count: 0,
                decimal_count: 0,
                total_elapsed_s: 0.0,
                fastest_s: 0.0,
                slowest_s: 0.0,
                hex_display: String::new(),
                decimal_display: String::new(),
                digit_times: Vec::new(),
            },
            last_counts: (0, 0),
            hex_digits: String::new(),
            decimal_digits: String::new(),
            hex_preview: "3.".to_string(),
            decimal_preview: "3.".to_string(),
            hex_freq: [0; 16],
            dec_freq: [0; 10],
            histogram_mode: HistogramMode::Hex,
            explorer: ExplorerState::default(),
            logs: Vec::new(),
            auto_scroll: true,
            exit_requested: false,
        }
    }

    fn sync_from_core(&mut self) {
        let Some(core) = self.state.core() else {
            return;
        };
        let counts = (
            core.record.hex_digits.len() as u64,
            core.record.decimal_digits.len() as u64,
        );
        self.last_snapshot = core.ui_snapshot_with_limits(128, 0);

        if counts != self.last_counts {
            if core.record.hex_digits.len() < self.hex_digits.len() {
                self.hex_digits.clear();
                self.hex_freq = [0; 16];
            }
            if core.record.decimal_digits.len() < self.decimal_digits.len() {
                self.decimal_digits.clear();
                self.dec_freq = [0; 10];
            }
            let new_hex = &core.record.hex_digits[self.hex_digits.len()..];
            if !new_hex.is_empty() {
                accumulate_hex_frequencies(&mut self.hex_freq, new_hex);
                self.hex_digits.push_str(new_hex);
            }
            let new_dec = &core.record.decimal_digits[self.decimal_digits.len()..];
            if !new_dec.is_empty() {
                accumulate_decimal_frequencies(&mut self.dec_freq, new_dec);
                self.decimal_digits.push_str(new_dec);
            }

            let hex_tail = &self.hex_digits[self.hex_digits.len().saturating_sub(128)..];
            let dec_tail = &self.decimal_digits
                [self.decimal_digits.len().saturating_sub(PREVIEW_TAIL_LIMIT)..];
            self.hex_preview = pi_preview(hex_tail, counts.0);
            self.decimal_preview = pi_preview(dec_tail, counts.1);
            self.last_counts = counts;
        }
    }

    fn merge_logs(&mut self) {
        for entry in self.state.recent_logs().into_iter().rev() {
            if self.logs.last().is_none_or(|last| entry.index > last.index) {
                self.logs.push(entry);
            }
        }
        let excess = self.logs.len().saturating_sub(MAX_LOCAL_LOGS);
        self.logs.drain(..excess);
    }

    fn apply(&mut self, commands: Commands, ctx: &egui::Context) {
        if commands.toggle_pause && self.state.is_running() {
            if let Err(error) = self.state.toggle_pause() {
                self.state
                    .set_status(format!("Failed to update pause state: {error}"));
            }
        }
        if commands.save {
            self.state.request_save();
        }
        if commands.exit {
            self.exit_requested = true;
            self.state.request_stop();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_from_core();
        self.merge_logs();

        let running = self.state.is_running();
        let fatal_error = self.state.fatal_error();
        if !running && (self.exit_requested || fatal_error.is_none()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let shortcuts = read_shortcuts(ctx);
        self.apply(shortcuts, ctx);

        let stats = Stats::new(&self.last_snapshot, self.session_start);
        let frame = Frame {
            snapshot: &self.last_snapshot,
            stats: &stats,
            paused: self.state.is_paused(),
            running,
            target: self.state.target(),
            fatal_error: fatal_error.as_deref(),
            status: self.state.status_text(),
            hex_digits: &self.hex_digits,
            decimal_digits: &self.decimal_digits,
            hex_freq: &self.hex_freq,
            dec_freq: &self.dec_freq,
        };

        let top = panels::top_bar(ctx, &frame);
        if let Some(err) = fatal_error.as_deref() {
            panels::error_banner(ctx, err);
        }
        panels::status_bar(ctx, &frame);

        if let Some(copied_text) = self.explorer.show(
            ctx,
            &self.hex_digits,
            &self.decimal_digits,
            &self.hex_preview,
            &self.decimal_preview,
        ) {
            ctx.output_mut(|o| o.copied_text = copied_text);
            self.state.set_status("Preview copied to clipboard");
        }

        if let Some(copied_tail) = panels::central_panel(
            ctx,
            &frame,
            &self.logs,
            &mut self.auto_scroll,
            &mut self.histogram_mode,
        ) {
            ctx.output_mut(|o| o.copied_text = copied_tail);
            self.state.set_status("Recent digits copied to clipboard");
        }

        self.apply(
            Commands {
                toggle_pause: top.toggle_pause,
                save: top.save,
                exit: top.exit,
            },
            ctx,
        );
        ctx.request_repaint_after(REPAINT_INTERVAL);
    }
}
