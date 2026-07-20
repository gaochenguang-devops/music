#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod lrc;
mod player;
mod playlist;
mod ui;
mod utils;

use anyhow::Result;

fn main() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sonora")
            .with_inner_size([1080.0, 700.0])
            .with_min_inner_size([860.0, 580.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Sonora",
        options,
        Box::new(|cc| Ok(Box::new(ui::MusicApp::new(cc)))),
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))
}
