//! Live preview rendering.
//!
//! Evaluating a 256² graph (and the atlas-style downsample) takes long enough
//! to drop frames, so it runs on a worker thread and the UI only uploads the
//! finished pixels. One job is in flight at a time.

use std::sync::mpsc::{self, TryRecvError};

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Vec2};

use crate::bake;
use crate::eval;

use super::{Preview, TexgenApp};

/// A preview render running off the UI thread.
pub(crate) struct PreviewJob {
    revision: u64,
    edge: u32,
    receiver: mpsc::Receiver<Result<PreviewPixels, String>>,
}

/// Finished preview pixels, ready to become egui textures.
struct PreviewPixels {
    pixels: Vec<u8>,
    tile: Vec<u8>,
}

impl TexgenApp {
    pub(super) fn poll_preview(&mut self, ctx: &egui::Context) {
        // Finish (or keep waiting for) the in-flight job first.
        if let Some(job) = self.preview_job.take() {
            match job.receiver.try_recv() {
                Ok(result) => {
                    // A result for an older revision is stale; the debounce
                    // below schedules a fresh render.
                    if job.revision == self.revision && job.edge == self.preview_size {
                        match result {
                            Ok(done) => self.apply_preview(ctx, job.edge, done),
                            Err(error) => self.preview_error = Some(error),
                        }
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.preview_job = Some(job);
                    return;
                }
                Err(TryRecvError::Disconnected) => {
                    self.preview_error = Some("preview worker stopped without a result".to_owned());
                }
            }
        }

        let Some(due) = self.preview_due else {
            return;
        };
        if std::time::Instant::now() < due {
            return;
        }
        self.preview_due = None;

        let graph = self.graph.clone();
        let edge = self.preview_size;
        let revision = self.revision;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<PreviewPixels, String> {
                let program = graph.compile().map_err(|error| error.to_string())?;
                let pixels = eval::evaluate_at(&program, edge);
                let tile = bake::downsample_edge(&pixels, edge, graph.alpha)
                    .map_err(|error| error.to_string())?;
                Ok(PreviewPixels {
                    pixels,
                    tile: tile.to_vec(),
                })
            })();
            let _ = sender.send(result);
        });
        self.preview_job = Some(PreviewJob {
            revision,
            edge,
            receiver,
        });
        // Come back soon to collect the result; the idle loop also repaints at
        // 20 fps, so this only shortens the wait.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    fn apply_preview(&mut self, ctx: &egui::Context, edge: u32, done: PreviewPixels) {
        let source_image =
            egui::ColorImage::from_rgba_unmultiplied([edge as usize, edge as usize], &done.pixels);
        let tile_image = egui::ColorImage::from_rgba_unmultiplied([16, 16], &done.tile);
        let revision = self.revision;
        let source = ctx.load_texture(
            format!("preview-source-{revision}-{edge}"),
            source_image,
            egui::TextureOptions::NEAREST,
        );
        let tile = ctx.load_texture(
            format!("preview-tile-{revision}-{edge}"),
            tile_image,
            egui::TextureOptions::NEAREST,
        );
        self.preview = Some(Preview { source, tile, edge });
        self.preview_error = None;
    }

    pub(super) fn preview_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Preview");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("preview-size")
                    .selected_text(format!("{}²", self.preview_size))
                    .width(72.0)
                    .show_ui(ui, |ui| {
                        for size in [64, 128, 256] {
                            if ui
                                .selectable_label(self.preview_size == size, format!("{size}²"))
                                .clicked()
                            {
                                self.preview_size = size;
                                self.invalidate_preview();
                            }
                        }
                    });
            });
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_tiled, "wrap");
            ui.checkbox(&mut self.show_checker, "alpha");
            if let Some(preview) = &self.preview {
                ui.label(
                    RichText::new(format!("{}² / {:?}", preview.edge, self.graph.alpha))
                        .weak()
                        .small(),
                );
            }
        });

        let available = ui.available_width();
        let source_size = (available * 0.66).clamp(140.0, 220.0);
        let tile_size = (available - source_size - 10.0).clamp(56.0, 100.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(source_size), egui::Sense::hover());
            if let Some(preview) = &self.preview {
                self.draw_texture(ui, rect, &preview.source, self.show_tiled);
            } else {
                ui.painter()
                    .rect_filled(rect, 4.0, Color32::from_rgb(18, 20, 24));
            }
            let (tile_rect, _) =
                ui.allocate_exact_size(Vec2::splat(tile_size), egui::Sense::hover());
            if let Some(preview) = &self.preview {
                self.draw_texture(ui, tile_rect, &preview.tile, self.show_tiled);
            }
            if !self.show_tiled {
                ui.painter().text(
                    tile_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "16²",
                    egui::FontId::proportional(11.0),
                    Color32::from_white_alpha(180),
                );
            }
        });
        if let Some(error) = &self.preview_error {
            ui.colored_label(Color32::from_rgb(235, 104, 104), error);
        }
    }

    fn draw_texture(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        texture: &egui::TextureHandle,
        tiled: bool,
    ) {
        let painter = ui.painter();
        if self.show_checker {
            draw_checker(painter, rect, (rect.width() / 16.0).max(4.0));
        }
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        if tiled {
            let half = rect.width() / 2.0;
            let half_h = rect.height() / 2.0;
            for row in 0..2 {
                for column in 0..2 {
                    let target = Rect::from_min_size(
                        Pos2::new(
                            rect.left() + column as f32 * half,
                            rect.top() + row as f32 * half_h,
                        ),
                        Vec2::new(half, half_h),
                    );
                    painter.image(texture.id(), target, uv, Color32::WHITE);
                }
            }
            painter.line_segment(
                [
                    Pos2::new(rect.center().x, rect.top()),
                    Pos2::new(rect.center().x, rect.bottom()),
                ],
                egui::Stroke::new(0.5_f32, Color32::from_black_alpha(60)),
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.center().y),
                    Pos2::new(rect.right(), rect.center().y),
                ],
                egui::Stroke::new(0.5_f32, Color32::from_black_alpha(60)),
            );
        } else {
            painter.image(texture.id(), rect, uv, Color32::WHITE);
        }
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0_f32, Color32::from_black_alpha(120)),
            egui::StrokeKind::Outside,
        );
    }
}

fn draw_checker(painter: &egui::Painter, rect: Rect, cell: f32) {
    painter.rect_filled(rect, 2.0, Color32::from_rgb(42, 44, 48));
    let columns = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for row in 0..rows {
        for column in 0..columns {
            if (row + column) % 2 == 0 {
                let min = Pos2::new(
                    rect.left() + column as f32 * cell,
                    rect.top() + row as f32 * cell,
                );
                let max = Pos2::new(
                    (min.x + cell).min(rect.right()),
                    (min.y + cell).min(rect.bottom()),
                );
                painter.rect_filled(
                    Rect::from_min_max(min, max),
                    0.0,
                    Color32::from_rgb(58, 61, 66),
                );
            }
        }
    }
}
