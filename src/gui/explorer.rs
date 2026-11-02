//! Indexed, grouped, and searchable bottom Digit Explorer panel.
use super::charts::{CLR_ERROR, CLR_INFO, CLR_MUTED, CLR_SUCCESS, CLR_WARN};
use crate::view::{find_digit_matches, format_count, match_covering_pos, MAX_SEARCH_MATCHES};
use egui::{text::LayoutJob, Color32, FontId, TextFormat, TextStyle};

const DIGITS_PER_ROW: usize = 50;
const GROUP_SIZE: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExplorerBase {
    Decimal,
    Hex,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExplorerView {
    Table,
    Raw,
}

pub struct ExplorerState {
    pub base: ExplorerBase,
    pub view: ExplorerView,
    pub search_query: String,
    last_search_key: (ExplorerBase, String, usize),
    pub matches: Vec<usize>,
    pub active_match: usize,
    scroll_to_match: bool,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            base: ExplorerBase::Decimal,
            view: ExplorerView::Table,
            search_query: String::new(),
            last_search_key: (ExplorerBase::Decimal, String::new(), 0),
            matches: Vec::new(),
            active_match: 0,
            scroll_to_match: false,
        }
    }
}

impl ExplorerState {
    fn refresh_search(&mut self, digits: &str) {
        let trimmed = self.search_query.trim().to_string();
        let key = (self.base, trimmed.clone(), digits.len());
        if key != self.last_search_key {
            let query_changed = self.last_search_key.0 != key.0 || self.last_search_key.1 != key.1;
            self.matches = find_digit_matches(digits, &trimmed);
            if query_changed || self.active_match >= self.matches.len() {
                self.active_match = 0;
                if query_changed && !self.matches.is_empty() {
                    self.scroll_to_match = true;
                }
            }
            self.last_search_key = key;
        }
    }

    fn step_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            return;
        }
        if forward {
            self.active_match = (self.active_match + 1) % self.matches.len();
        } else if self.active_match == 0 {
            self.active_match = self.matches.len() - 1;
        } else {
            self.active_match -= 1;
        }
        self.scroll_to_match = true;
    }

    /// Renders the bottom Digit Explorer panel; returns `Some(text)` if the Copy button was clicked.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        hex_digits: &str,
        decimal_digits: &str,
        hex_preview: &str,
        decimal_preview: &str,
    ) -> Option<String> {
        let mut copied: Option<String> = None;
        let active_digits = match self.base {
            ExplorerBase::Decimal => decimal_digits,
            ExplorerBase::Hex => hex_digits,
        };
        let active_preview = match self.base {
            ExplorerBase::Decimal => decimal_preview,
            ExplorerBase::Hex => hex_preview,
        };
        self.refresh_search(active_digits);

        egui::TopBottomPanel::bottom("pi_digit_explorer")
            .resizable(true)
            .default_height(215.0)
            .min_height(135.0)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.strong("Digit Explorer");
                    ui.separator();
                    ui.selectable_value(&mut self.base, ExplorerBase::Decimal, "Decimal (10)");
                    ui.selectable_value(&mut self.base, ExplorerBase::Hex, "Hex (16)");
                    ui.separator();
                    ui.selectable_value(&mut self.view, ExplorerView::Table, "Indexed Table");
                    ui.selectable_value(&mut self.view, ExplorerView::Raw, "Raw 10k Preview");
                    ui.separator();

                    ui.label("🔍");
                    let search_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Find digits (e.g. 1415, CAFE)…")
                            .desired_width(180.0),
                    );
                    if search_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.step_match(true);
                    }

                    let query_len = self.search_query.trim().len();
                    if query_len > 0 {
                        if self.matches.is_empty() {
                            ui.colored_label(CLR_ERROR, "No matches");
                        } else {
                            let capped = if self.matches.len() >= MAX_SEARCH_MATCHES {
                                "+"
                            } else {
                                ""
                            };
                            ui.colored_label(
                                CLR_WARN,
                                format!(
                                    "{} / {}{capped}",
                                    self.active_match + 1,
                                    format_count(self.matches.len() as u64)
                                ),
                            );
                            if ui.button("◀").on_hover_text("Previous match").clicked() {
                                self.step_match(false);
                            }
                            if ui.button("▶").on_hover_text("Next match (Enter)").clicked() {
                                self.step_match(true);
                            }
                        }
                        if ui.button("✕").on_hover_text("Clear search").clicked() {
                            self.search_query.clear();
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Copy Preview")
                            .on_hover_text("Copy 10,000-digit preview to clipboard")
                            .clicked()
                        {
                            copied = Some(active_preview.to_string());
                        }
                        ui.colored_label(
                            CLR_MUTED,
                            format!("3. + {} digits", format_count(active_digits.len() as u64)),
                        );
                    });
                });
                ui.add_space(4.0);

                match self.view {
                    ExplorerView::Raw => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let mut text = active_preview;
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .font(TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(8)
                                    .lock_focus(true),
                            );
                        });
                    }
                    ExplorerView::Table => {
                        self.render_indexed_table(ui, active_digits);
                    }
                }
            });

        copied
    }

    fn render_indexed_table(&mut self, ui: &mut egui::Ui, digits: &str) {
        let total_rows = digits.len().div_ceil(DIGITS_PER_ROW).max(1);
        let row_height = ui.text_style_height(&TextStyle::Monospace) + 2.0;
        let query_len = self.search_query.trim().len();

        let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.scroll_to_match {
            self.scroll_to_match = false;
            if let Some(&match_pos) = self.matches.get(self.active_match) {
                let target_row = match_pos / DIGITS_PER_ROW;
                let offset = (target_row.saturating_sub(2) as f32) * row_height;
                scroll = scroll.vertical_scroll_offset(offset);
            }
        }

        let base_color = match self.base {
            ExplorerBase::Decimal => CLR_SUCCESS,
            ExplorerBase::Hex => CLR_WARN,
        };

        scroll.show_rows(ui, row_height, total_rows, |ui, row_range| {
            if digits.is_empty() {
                ui.colored_label(CLR_MUTED, "+000,000 │ 3. (no digits computed yet)");
                return;
            }
            for row in row_range {
                let start = row * DIGITS_PER_ROW;
                let end = (start + DIGITS_PER_ROW).min(digits.len());
                let slice = &digits[start..end];
                let job = build_row_job(
                    row,
                    start,
                    slice,
                    base_color,
                    &self.matches,
                    query_len,
                    self.active_match,
                );
                ui.label(job);
            }
        });
    }
}

fn build_row_job(
    row: usize,
    start_idx: usize,
    slice: &str,
    digit_color: Color32,
    matches: &[usize],
    query_len: usize,
    active_match: usize,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let font_id = FontId::monospace(12.5);
    let muted_fmt = TextFormat {
        font_id: font_id.clone(),
        color: CLR_MUTED,
        ..Default::default()
    };
    let lead_fmt = TextFormat {
        font_id: font_id.clone(),
        color: CLR_INFO,
        ..Default::default()
    };
    let normal_fmt = TextFormat {
        font_id: font_id.clone(),
        color: digit_color,
        ..Default::default()
    };
    let match_fmt = TextFormat {
        font_id: font_id.clone(),
        color: Color32::BLACK,
        background: CLR_WARN,
        ..Default::default()
    };
    let active_match_fmt = TextFormat {
        font_id,
        color: Color32::BLACK,
        background: CLR_INFO,
        ..Default::default()
    };

    job.append(
        &format!("+{:>8} │ ", format_count(start_idx as u64)),
        0.0,
        muted_fmt.clone(),
    );
    if row == 0 {
        job.append("3.", 0.0, lead_fmt);
    } else {
        job.append("  ", 0.0, muted_fmt.clone());
    }

    for (offset, ch) in slice.chars().enumerate() {
        if offset > 0 && offset.is_multiple_of(GROUP_SIZE) {
            job.append(" ", 0.0, muted_fmt.clone());
        }
        let abs_pos = start_idx + offset;
        let fmt = match match_covering_pos(matches, query_len, abs_pos) {
            Some(idx) if idx == active_match => &active_match_fmt,
            Some(_) => &match_fmt,
            None => &normal_fmt,
        };
        let mut buf = [0u8; 4];
        job.append(ch.encode_utf8(&mut buf), 0.0, fmt.clone());
    }

    // End-of-row range indicator
    let row_end = start_idx + slice.len();
    let pad_chars = DIGITS_PER_ROW.saturating_sub(slice.len());
    let pad_groups = (DIGITS_PER_ROW - 1) / GROUP_SIZE - slice.len().saturating_sub(1) / GROUP_SIZE;
    if pad_chars + pad_groups > 0 {
        job.append(&" ".repeat(pad_chars + pad_groups), 0.0, muted_fmt.clone());
    }
    job.append(
        &format!(" │ {:>8}", format_count(row_end as u64)),
        0.0,
        muted_fmt,
    );
    job
}
