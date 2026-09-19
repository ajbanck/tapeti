//! The status bar, the port of `src/ui/StatusBar.tsx`: the Dec/Hex switch, the
//! two compare modes, the lock, the waveform, and either the status message or
//! the playback progress.

use egui::{Align, Layout, RichText, Ui};

use tapeti_core::compare::{BlockCompareMode, TapeCompareMode};

use crate::app::App;
use crate::fmt;
use crate::icons::{self, Icon};

/// One clickable cell — `.statusbar .cell`: a rounded outlined pill holding the
/// icon the web bar shows and its text, tinted with the accent when it is `on`.
/// The value half of the text is `<b>`, so it is drawn in the text colour.
///
/// The data window borrows it for its own Dec/Hex switch, which is the same
/// control on another screen.
pub fn cell(
    ui: &mut Ui,
    icon: Option<&Icon>,
    text: &str,
    value: &str,
    on: bool,
    hover: &str,
    tok: &crate::theme::Tokens,
) -> bool {
    let (fg, strong, fill, edge) = if on {
        (tok.accent, tok.accent, tok.accent_soft, tok.accent_soft_2)
    } else {
        (tok.muted, tok.text, tok.surface_2, tok.border)
    };
    let font = egui::FontId::proportional(12.0);
    let p = ui.painter();
    let lead = (!text.is_empty()).then(|| p.layout_no_wrap(text.to_string(), font.clone(), fg));
    let val = (!value.is_empty()).then(|| p.layout_no_wrap(value.to_string(), font, strong));
    let icon_w = if icon.is_some() { 19.0 } else { 0.0 };
    let gap = if lead.is_some() && val.is_some() { 6.0 } else { 0.0 };
    let text_w = lead.as_ref().map_or(0.0, |g| g.size().x) + gap + val.as_ref().map_or(0.0, |g| g.size().x);
    let h = 22.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(20.0 + icon_w + text_w, h), egui::Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        egui::CornerRadius::same((h / 2.0) as u8),
        fill,
        egui::Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
    let mut x = rect.left() + 10.0;
    if let Some(icon) = icon {
        icons::paint(
            p,
            egui::Rect::from_center_size(egui::pos2(x + 6.5, rect.center().y), egui::vec2(13.0, 13.0)),
            icon,
            fg,
        );
        x += icon_w;
    }
    if let Some(g) = lead {
        let y = rect.center().y - g.size().y / 2.0;
        x += g.size().x + gap;
        p.galley(egui::pos2(x - g.size().x - gap, y), g, fg);
    }
    if let Some(g) = val {
        let y = rect.center().y - g.size().y / 2.0;
        p.galley(egui::pos2(x, y), g, strong);
    }
    response.on_hover_text(hover).clicked()
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let tok = app.tokens;
    ui.horizontal(|ui| {
        let hex = app.store.hex;
        let base = if hex { "Hex" } else { "Dec" };
        if cell(ui, Some(&icons::HASH), "", base, hex, "Number base on this screen", &tok) {
            app.store.hex = !hex;
            app.store.touch_view();
        }

        let bc = app.store.block_compare;
        let bc_label = match bc {
            BlockCompareMode::Data => "data",
            BlockCompareMode::DataTimings => "data + timings",
            BlockCompareMode::DataTimingsPauses => "data + timings + pauses",
        };
        if cell(ui, None, "Block compare", bc_label, false, "How two blocks are compared", &tok) {
            app.store.block_compare = match bc {
                BlockCompareMode::Data => BlockCompareMode::DataTimings,
                BlockCompareMode::DataTimings => BlockCompareMode::DataTimingsPauses,
                BlockCompareMode::DataTimingsPauses => BlockCompareMode::Data,
            };
        }

        let tc = app.store.tape_compare;
        let tc_label = match tc {
            TapeCompareMode::DataBlocks => "data blocks only",
            TapeCompareMode::IgnoreMetadata => "ignore metadata",
            TapeCompareMode::All => "all blocks",
        };
        let hover = "Which blocks take part in tape compare";
        if cell(ui, Some(&icons::COMPARE), "Tape compare", tc_label, false, hover, &tok) {
            app.store.tape_compare = match tc {
                TapeCompareMode::DataBlocks => TapeCompareMode::IgnoreMetadata,
                TapeCompareMode::IgnoreMetadata => TapeCompareMode::All,
                TapeCompareMode::All => TapeCompareMode::DataBlocks,
            };
        }

        let locked = app.store.locked;
        let hover =
            if locked { "Locked: click to allow editing" } else { "Unlocked: click to prevent edits" };
        let icon = if locked { &icons::LOCK } else { &icons::UNLOCK };
        let text = if locked { "Locked" } else { "Unlocked" };
        if cell(ui, Some(icon), text, "", locked, hover, &tok) {
            app.store.toggle_lock();
        }

        let mic = app.store.audio_mic;
        let label = if mic { "MIC emulation" } else { "Square wave" };
        let hover = "Waveform used for playback and WAV export";
        if cell(ui, Some(&icons::WAVE), label, "", false, hover, &tok) {
            app.store.audio_mic = !mic;
        }

        // The theme switch lives at the right end of the menu bar, where
        // `MenuBar.tsx` has always had it — not here as well.

        if app.progress.playing {
            progress(app, ui);
        } else {
            // `.cell.grow`: the message, with no pill around it.
            ui.add_space(4.0);
            let status = app.store.status().to_string();
            ui.label(RichText::new(status).size(12.0).color(tok.muted));
        }
    });
}

fn progress(app: &mut App, ui: &mut Ui) {
    let tok = app.tokens;
    let p = app.progress;
    let zero = app.store.settings.zero_based;
    icons::inline(ui, &icons::PLAY, tok.ok, 11.0);
    ui.label(
        RichText::new(format!("{} / {}", fmt::time(p.elapsed), fmt::time(p.total))).size(11.0).monospace(),
    );
    let (rect, response) = ui.allocate_exact_size(egui::vec2(160.0, 8.0), egui::Sense::click());
    let frac = if p.total > 0.0 { (p.elapsed / p.total) as f32 } else { 0.0 };
    ui.painter().rect_filled(rect, egui::CornerRadius::same(4), tok.surface_3);
    let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * frac, rect.height()));
    ui.painter().rect_filled(filled, egui::CornerRadius::same(4), tok.ok);
    if response.on_hover_text("Click to stop").clicked() {
        app.player.stop();
    }
    if p.block >= 0 {
        ui.label(
            RichText::new(format!("Block {}", fmt::block_no(p.block as usize, zero)))
                .size(11.0)
                .color(tok.muted),
        );
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.small_button("Stop").clicked() {
            app.player.stop();
        }
    });
}
