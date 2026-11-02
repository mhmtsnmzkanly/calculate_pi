//! Speed/latency sparkline chart and digit distribution (normality) histogram.
use crate::app_state::LogEntry;
use crate::view::format_count;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};

pub const CLR_PRIMARY: Color32 = Color32::from_rgb(0, 180, 216);
pub const CLR_WARN: Color32 = Color32::from_rgb(255, 183, 3);
pub const CLR_SUCCESS: Color32 = Color32::from_rgb(128, 237, 153);
pub const CLR_ERROR: Color32 = Color32::from_rgb(230, 57, 70);
pub const CLR_INFO: Color32 = Color32::from_rgb(144, 224, 239);
pub const CLR_MUTED: Color32 = Color32::from_rgb(148, 156, 168);
pub const CLR_EMPTY_CELL: Color32 = Color32::from_rgb(28, 31, 38);
pub const CLR_CHART_BG: Color32 = Color32::from_rgb(20, 23, 29);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HistogramMode {
    Hex,
    Decimal,
}

/// High-contrast color palette for hex digits `0`–`F`, distinct from empty cells (`CLR_EMPTY_CELL`).
pub fn hex_digit_color(digit: char) -> Color32 {
    match digit.to_ascii_uppercase() {
        '0' => Color32::from_rgb(69, 123, 157),
        '1' => Color32::from_rgb(72, 202, 228),
        '2' => Color32::from_rgb(0, 180, 216),
        '3' => Color32::from_rgb(0, 150, 199),
        '4' => Color32::from_rgb(67, 97, 238),
        '5' => Color32::from_rgb(94, 96, 206),
        '6' => Color32::from_rgb(128, 237, 153),
        '7' => Color32::from_rgb(158, 240, 26),
        '8' => Color32::from_rgb(255, 183, 3),
        '9' => Color32::from_rgb(251, 133, 0),
        'A' => Color32::from_rgb(230, 57, 70),
        'B' => Color32::from_rgb(241, 91, 181),
        'C' => Color32::from_rgb(187, 134, 252),
        'D' => Color32::from_rgb(155, 93, 229),
        'E' => Color32::from_rgb(52, 152, 219),
        'F' => Color32::from_rgb(46, 196, 182),
        _ => CLR_EMPTY_CELL,
    }
}

fn draw_dashed_hline(painter: &egui::Painter, x_range: egui::Rangef, y: f32, stroke: Stroke) {
    let dash = 5.0;
    let gap = 4.0;
    let mut x = x_range.min;
    while x < x_range.max {
        let end = (x + dash).min(x_range.max);
        painter.line_segment([Pos2::new(x, y), Pos2::new(end, y)], stroke);
        x += dash + gap;
    }
}

/// Renders the per-digit computation latency (`ms/digit`) of recent digits as an area-filled curve.
pub fn speed_sparkline(ui: &mut egui::Ui, logs: &[LogEntry], width: f32) {
    ui.group(|ui| {
        ui.set_width(width);
        let values_ms: Vec<f64> = logs.iter().map(|e| e.duration * 1_000.0).collect();
        let (min_ms, avg_ms, max_ms) = if values_ms.is_empty() {
            (0.0, 0.0, 0.0)
        } else {
            let min = values_ms.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values_ms.iter().copied().fold(0.0_f64, f64::max);
            let avg = values_ms.iter().sum::<f64>() / values_ms.len() as f64;
            (min, avg, max)
        };

        ui.horizontal(|ui| {
            ui.strong("Latency Trend (ms/digit)");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !values_ms.is_empty() {
                    ui.colored_label(
                        CLR_MUTED,
                        format!(
                            "Min {:.2} | Avg {:.2} | Max {:.2} ms",
                            min_ms, avg_ms, max_ms
                        ),
                    );
                } else {
                    ui.colored_label(CLR_MUTED, "Waiting for digits…");
                }
            });
        });
        ui.add_space(2.0);

        let chart_size = Vec2::new((width - 12.0).max(80.0), 86.0);
        let (rect, response) = ui.allocate_exact_size(chart_size, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, CLR_CHART_BG);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::from_gray(45)));

        if values_ms.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No latency data yet",
                FontId::monospace(11.0),
                CLR_MUTED,
            );
            return;
        }

        let pad = 6.0;
        let plot = rect.shrink(pad);
        let lo = (min_ms * 0.9).min(avg_ms * 0.8);
        let hi = (max_ms * 1.1).max(lo + 1e-4);
        let span = (hi - lo).max(1e-6);

        let y_of = |ms: f64| -> f32 {
            let norm = ((ms - lo) / span).clamp(0.0, 1.0) as f32;
            plot.bottom() - norm * plot.height()
        };
        let x_of = |idx: usize, count: usize| -> f32 {
            if count <= 1 {
                plot.center().x
            } else {
                plot.left() + (idx as f32 / (count - 1) as f32) * plot.width()
            }
        };

        // Horizontal grid lines
        for step in [0.25_f32, 0.5, 0.75] {
            let y = plot.top() + step * plot.height();
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                Stroke::new(1.0_f32, Color32::from_gray(32)),
            );
        }

        let count = values_ms.len();
        let points: Vec<Pos2> = values_ms
            .iter()
            .enumerate()
            .map(|(i, &ms)| Pos2::new(x_of(i, count), y_of(ms)))
            .collect();

        // Semi-transparent fill under the curve
        let fill_color = Color32::from_rgba_unmultiplied(0, 180, 216, 42);
        for pair in points.windows(2) {
            let (p0, p1) = (pair[0], pair[1]);
            painter.add(Shape::convex_polygon(
                vec![
                    p0,
                    p1,
                    Pos2::new(p1.x, plot.bottom()),
                    Pos2::new(p0.x, plot.bottom()),
                ],
                fill_color,
                Stroke::NONE,
            ));
        }

        // Dashed mean line
        let avg_y = y_of(avg_ms);
        draw_dashed_hline(
            &painter,
            plot.x_range(),
            avg_y,
            Stroke::new(1.0_f32, CLR_WARN.gamma_multiply(0.75)),
        );

        // Primary latency curve
        painter.add(Shape::line(
            points.clone(),
            Stroke::new(1.6_f32, CLR_PRIMARY),
        ));

        // Hover crosshair and tooltip
        if let Some(pointer) = response.hover_pos() {
            if plot.contains(pointer) {
                let rel = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
                let idx = ((rel * (count.saturating_sub(1)) as f32).round() as usize)
                    .min(count.saturating_sub(1));
                let pt = points[idx];
                let entry = &logs[idx];
                let ms = values_ms[idx];
                let dig_s = if entry.duration > 0.0 {
                    1.0 / entry.duration
                } else {
                    0.0
                };
                painter.line_segment(
                    [Pos2::new(pt.x, plot.top()), Pos2::new(pt.x, plot.bottom())],
                    Stroke::new(1.0_f32, Color32::from_gray(120)),
                );
                painter.circle_filled(pt, 3.5, CLR_WARN);
                response.on_hover_text(format!(
                    "D-{} | Hex: {}\nLatency: {:.3} ms ({:.1} dig/s)",
                    format_count(entry.index),
                    entry.hex,
                    ms,
                    dig_s
                ));
            }
        }
    });
}

/// Bar chart of digit frequency / normality distribution (`0-F` Hex and `0-9` Decimal).
pub fn normality_histogram(
    ui: &mut egui::Ui,
    mode: &mut HistogramMode,
    hex_freq: &[u64; 16],
    hex_total: u64,
    dec_freq: &[u64; 10],
    dec_total: u64,
    width: f32,
) {
    ui.group(|ui| {
        ui.set_width(width);
        let is_hex = *mode == HistogramMode::Hex;
        let expected_pct = if is_hex { 6.25 } else { 10.0 };

        ui.horizontal(|ui| {
            ui.strong("Digit Distribution (Normality)");
            ui.colored_label(CLR_MUTED, format!("(expected {expected_pct:.2}%)"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.selectable_value(mode, HistogramMode::Decimal, "Decimal 0-9");
                ui.selectable_value(mode, HistogramMode::Hex, "Hex 0-F");
            });
        });
        ui.add_space(2.0);

        let chart_size = Vec2::new((width - 12.0).max(80.0), 104.0);
        let (rect, response) = ui.allocate_exact_size(chart_size, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, CLR_CHART_BG);
        painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, Color32::from_gray(45)));

        let (counts, total): (&[u64], u64) = if is_hex {
            (hex_freq, hex_total)
        } else {
            (dec_freq, dec_total)
        };
        let n_bins = counts.len();
        let label_height = 15.0;
        let plot = Rect::from_min_max(
            Pos2::new(rect.left() + 6.0, rect.top() + 8.0),
            Pos2::new(rect.right() - 6.0, rect.bottom() - label_height - 2.0),
        );

        let max_pct = counts
            .iter()
            .map(|&c| {
                if total > 0 {
                    c as f64 * 100.0 / total as f64
                } else {
                    0.0
                }
            })
            .fold(expected_pct * 1.25, f64::max)
            .max(1.0);

        let slot_w = plot.width() / n_bins as f32;
        let bar_w = (slot_w * 0.68).max(4.0);
        let pointer = response.hover_pos();
        let mut hovered_info: Option<String> = None;

        for (i, &count) in counts.iter().enumerate() {
            let pct = if total > 0 {
                count as f64 * 100.0 / total as f64
            } else {
                0.0
            };
            let h = ((pct / max_pct).clamp(0.0, 1.0) as f32) * plot.height();
            let center_x = plot.left() + (i as f32 + 0.5) * slot_w;
            let bar_rect = Rect::from_min_max(
                Pos2::new(center_x - bar_w * 0.5, plot.bottom() - h),
                Pos2::new(center_x + bar_w * 0.5, plot.bottom()),
            );
            let slot_rect = Rect::from_min_max(
                Pos2::new(plot.left() + i as f32 * slot_w, plot.top()),
                Pos2::new(plot.left() + (i + 1) as f32 * slot_w, rect.bottom()),
            );

            let ch = char::from_digit(i as u32, 16)
                .unwrap_or('?')
                .to_ascii_uppercase();
            let color = if is_hex {
                hex_digit_color(ch)
            } else {
                CLR_SUCCESS.gamma_multiply(0.85)
            };
            let is_hovered = pointer.is_some_and(|p| slot_rect.contains(p));
            painter.rect_filled(bar_rect, 2.0, color);
            if is_hovered {
                painter.rect_stroke(bar_rect, 2.0, Stroke::new(1.5_f32, Color32::WHITE));
                let diff = pct - expected_pct;
                hovered_info = Some(format!(
                    "Digit '{ch}': {} ({pct:.2}%)\nExpected uniform: {expected_pct:.2}% (Δ {diff:+.2}%)",
                    format_count(count)
                ));
            }

            painter.text(
                Pos2::new(center_x, rect.bottom() - label_height * 0.5),
                Align2::CENTER_CENTER,
                ch.to_string(),
                FontId::monospace(10.5),
                if is_hovered { Color32::WHITE } else { CLR_MUTED },
            );
        }

        // Expected uniform distribution reference line
        let ref_y = plot.bottom() - ((expected_pct / max_pct) as f32) * plot.height();
        draw_dashed_hline(
            &painter,
            plot.x_range(),
            ref_y,
            Stroke::new(1.0_f32, CLR_INFO.gamma_multiply(0.8)),
        );

        if let Some(info) = hovered_info {
            response.on_hover_text(info);
        }
    });
}
