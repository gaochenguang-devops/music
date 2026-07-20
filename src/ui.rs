use std::{fs, path::PathBuf, time::Duration};

use directories::ProjectDirs;
use eframe::egui::{
    self, Align, Color32, FontData, FontDefinitions, FontFamily, Layout, RichText, Sense, Stroke,
    TextureHandle, Vec2,
};
use rand::prelude::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::{
    lrc::Lyrics,
    player::{AudioController, PlayerCommand, PlayerEvent},
    playlist::{PlayMode, Playlist, Track},
    utils::format_duration,
};

const BG: Color32 = Color32::from_rgb(14, 17, 20);
const PANEL: Color32 = Color32::from_rgb(23, 27, 31);
const PANEL_HOVER: Color32 = Color32::from_rgb(34, 39, 44);
const TEXT: Color32 = Color32::from_rgb(230, 233, 235);
const MUTED: Color32 = Color32::from_rgb(135, 145, 151);
const ACCENT: Color32 = Color32::from_rgb(56, 214, 164);
const WARM: Color32 = Color32::from_rgb(245, 174, 73);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppConfig {
    volume: f32,
    mode: PlayMode,
    speed: f32,
    dark_theme: bool,
    device: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            volume: 0.75,
            mode: PlayMode::Sequential,
            speed: 1.0,
            dark_theme: true,
            device: None,
        }
    }
}

impl AppConfig {
    fn path() -> Option<PathBuf> {
        ProjectDirs::from("com", "Sonora", "Sonora").map(|p| p.config_dir().join("config.json"))
    }
    fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}

pub struct MusicApp {
    audio: AudioController,
    playlist: Playlist,
    lyrics: Lyrics,
    position: Duration,
    is_playing: bool,
    volume: f32,
    previous_volume: f32,
    speed: f32,
    mode: PlayMode,
    devices: Vec<String>,
    selected_device: Option<String>,
    cover_texture: Option<TextureHandle>,
    cover_key: Option<PathBuf>,
    status: Option<String>,
    dragged_track: Option<usize>,
    scroll_to_lyric: Option<usize>,
}

impl MusicApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_fonts(&cc.egui_ctx);
        configure_style(&cc.egui_ctx);
        let config = AppConfig::load();
        let audio = AudioController::spawn();
        let _ = audio.commands.send(PlayerCommand::SetVolume(config.volume));
        let _ = audio.commands.send(PlayerCommand::SetSpeed(config.speed));
        if config.device.is_some() {
            let _ = audio
                .commands
                .send(PlayerCommand::SwitchDevice(config.device.clone()));
        }
        Self {
            audio,
            playlist: Playlist::default(),
            lyrics: Lyrics::default(),
            position: Duration::ZERO,
            is_playing: false,
            volume: config.volume,
            previous_volume: config.volume.max(0.5),
            speed: config.speed,
            mode: config.mode,
            devices: Vec::new(),
            selected_device: config.device,
            cover_texture: None,
            cover_key: None,
            status: None,
            dragged_track: None,
            scroll_to_lyric: None,
        }
    }

    fn command(&self, command: PlayerCommand) {
        let _ = self.audio.commands.send(command);
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.audio.events.try_recv() {
            match event {
                PlayerEvent::Position(position) => self.position = position,
                PlayerEvent::Playing(playing) => self.is_playing = playing,
                PlayerEvent::Finished => self.advance(false),
                PlayerEvent::Devices { names, selected } => {
                    self.devices = names;
                    self.selected_device = selected;
                }
                PlayerEvent::Error(message) => self.status = Some(message),
            }
        }
    }

    fn play_index(&mut self, index: usize) {
        if index >= self.playlist.tracks.len() {
            return;
        }
        self.playlist.current = Some(index);
        self.position = Duration::ZERO;
        let track = &self.playlist.tracks[index];
        self.lyrics = track
            .lrc_path
            .as_deref()
            .and_then(|p| Lyrics::from_file(p).ok())
            .unwrap_or_default();
        self.cover_key = None;
        self.command(PlayerCommand::Load {
            path: track.path.clone(),
            autoplay: true,
        });
    }

    fn advance(&mut self, backwards: bool) {
        let len = self.playlist.tracks.len();
        if len == 0 {
            return;
        }
        let current = self.playlist.current.unwrap_or(0);
        let next = match self.mode {
            PlayMode::RepeatOne if !backwards => current,
            PlayMode::Shuffle => *(0..len)
                .collect::<Vec<_>>()
                .choose(&mut rand::rng())
                .unwrap_or(&current),
            PlayMode::Sequential => {
                if backwards {
                    current.saturating_sub(1)
                } else if current + 1 < len {
                    current + 1
                } else {
                    self.is_playing = false;
                    return;
                }
            }
            PlayMode::RepeatAll | PlayMode::RepeatOne => {
                if backwards {
                    (current + len - 1) % len
                } else {
                    (current + 1) % len
                }
            }
        };
        self.play_index(next);
    }

    fn current_track(&self) -> Option<&Track> {
        self.playlist
            .current
            .and_then(|i| self.playlist.tracks.get(i))
    }

    fn add_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("MP3 audio", &["mp3"])
            .pick_files()
        {
            let errors = self.playlist.add_files(paths);
            self.report_errors(errors);
        }
    }

    fn add_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            let errors = self.playlist.add_folder(&folder);
            self.report_errors(errors);
        }
    }

    fn report_errors(&mut self, errors: Vec<String>) {
        if !errors.is_empty() {
            self.status = Some(format!("有 {} 个文件无法添加：{}", errors.len(), errors[0]));
        }
    }

    fn load_manual_lrc(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("LRC lyrics", &["lrc"])
            .pick_file()
        else {
            return;
        };
        match Lyrics::from_file(&path) {
            Ok(lyrics) => {
                self.lyrics = lyrics;
                if let Some(index) = self.playlist.current {
                    self.playlist.tracks[index].lrc_path = Some(path);
                }
            }
            Err(err) => self.status = Some(err.to_string()),
        }
    }

    fn update_cover(&mut self, ctx: &egui::Context) {
        let Some(track) = self.current_track() else {
            self.cover_texture = None;
            return;
        };
        if self.cover_key.as_ref() == Some(&track.path) {
            return;
        }
        let path = track.path.clone();
        let cover = track.cover.clone();
        self.cover_key = Some(path);
        self.cover_texture = cover.as_ref().and_then(|bytes| {
            let image = image::load_from_memory(bytes).ok()?.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            Some(ctx.load_texture(
                "album-cover",
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                Default::default(),
            ))
        });
    }

    fn save_config(&self) {
        AppConfig {
            volume: self.volume,
            mode: self.mode,
            speed: self.speed,
            dark_theme: true,
            device: self.selected_device.clone(),
        }
        .save();
    }
}

impl eframe::App for MusicApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events();
        self.update_cover(&ctx);
        ctx.request_repaint_after(Duration::from_millis(33));

        egui::Panel::top("header")
            .exact_size(100.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(26, 14)),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("controls")
            .exact_size(128.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(24, 12)),
            )
            .show(ui, |ui| self.controls(ui));
        egui::Panel::right("playlist")
            .resizable(true)
            .default_size(360.0)
            .size_range(280.0..=480.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(18, 21, 24))
                    .inner_margin(egui::Margin::symmetric(16, 18)),
            )
            .show(ui, |ui| self.playlist_panel(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ui, |ui| self.lyrics_panel(ui));

        if let Some(message) = self.status.clone() {
            egui::Window::new("提示")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    ui.label(message);
                    if ui.button("关闭").clicked() {
                        self.status = None;
                    }
                });
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_config();
    }
}

impl MusicApp {
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("SONORA").size(12.0).color(ACCENT).strong());
                let (title, artist, album) = self
                    .current_track()
                    .map(|t| (t.title.as_str(), t.artist.as_str(), t.album.as_str()))
                    .unwrap_or(("选择一首音乐", "音乐正在等待", "—"));
                ui.label(RichText::new(title).size(24.0).color(TEXT).strong());
                ui.label(
                    RichText::new(format!("{artist}  ·  {album}"))
                        .size(13.0)
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("选择歌词")
                    .on_hover_text("手动加载 LRC 文件")
                    .clicked()
                {
                    self.load_manual_lrc();
                }
                let selected = self
                    .selected_device
                    .clone()
                    .unwrap_or_else(|| "默认输出设备".into());
                egui::ComboBox::from_id_salt("device")
                    .selected_text(selected)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for name in self.devices.clone() {
                            if ui
                                .selectable_label(
                                    self.selected_device.as_ref() == Some(&name),
                                    &name,
                                )
                                .clicked()
                            {
                                self.selected_device = Some(name.clone());
                                self.command(PlayerCommand::SwitchDevice(Some(name)));
                            }
                        }
                    });
            });
        });
    }

    fn lyrics_panel(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        ui.horizontal(|ui| {
            ui.set_height(available.y);
            ui.vertical_centered(|ui| {
                let side = available.x.min(available.y) * 0.27;
                if let Some(texture) = &self.cover_texture {
                    ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(Vec2::splat(side))
                            .corner_radius(6),
                    );
                } else {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 6, Color32::from_rgb(31, 37, 41));
                    ui.painter()
                        .circle_stroke(rect.center(), side * 0.22, Stroke::new(2.0, MUTED));
                    ui.painter()
                        .circle_filled(rect.center(), side * 0.055, WARM);
                }
                ui.add_space(12.0);
                ui.label(RichText::new("ALBUM / LYRICS").size(10.0).color(MUTED));
            });
            ui.separator();
            let active = self.lyrics.active_index(self.position);
            egui::ScrollArea::vertical()
                .id_salt("lyrics-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(available.y * 0.35);
                    if self.lyrics.lines.is_empty() {
                        ui.vertical_centered(|ui| {
                            ui.add_space(50.0);
                            ui.label(RichText::new("暂无歌词").size(22.0).color(MUTED));
                        });
                    } else {
                        for index in 0..self.lyrics.lines.len() {
                            let line = &self.lyrics.lines[index];
                            let is_active = active == Some(index);
                            let text = RichText::new(&line.text)
                                .size(if is_active { 23.0 } else { 16.0 })
                                .color(if is_active { ACCENT } else { MUTED })
                                .strong();
                            let response = ui.add_sized(
                                [ui.available_width(), if is_active { 44.0 } else { 34.0 }],
                                egui::Button::new(text)
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE),
                            );
                            if response.clicked() {
                                self.command(PlayerCommand::Seek(line.time));
                                self.position = line.time;
                            }
                            if is_active && self.scroll_to_lyric != active {
                                response.scroll_to_me(Some(Align::Center));
                                self.scroll_to_lyric = active;
                            }
                        }
                    }
                    ui.add_space(available.y * 0.35);
                });
        });
    }

    fn playlist_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("播放列表").size(18.0).color(TEXT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("清空").clicked() {
                    self.playlist = Playlist::default();
                    self.lyrics = Lyrics::default();
                    self.command(PlayerCommand::Stop);
                }
                if ui.small_button("文件夹").clicked() {
                    self.add_folder();
                }
                if ui.small_button("添加").clicked() {
                    self.add_files();
                }
            });
        });
        ui.label(
            RichText::new(format!("{} TRACKS", self.playlist.tracks.len()))
                .size(10.0)
                .color(MUTED),
        );
        ui.add_space(10.0);
        let mut play = None;
        let mut remove = None;
        let mut move_to = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, track) in self.playlist.tracks.iter().enumerate() {
                let current = self.playlist.current == Some(index);
                let frame = egui::Frame::new()
                    .fill(if current {
                        Color32::from_rgb(26, 55, 48)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(5)
                    .inner_margin(egui::Margin::symmetric(10, 8));
                let response =
                    frame
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(if current { "▶" } else { "≡" })
                                        .color(if current { ACCENT } else { MUTED }),
                                );
                                ui.vertical(|ui| {
                                    ui.set_width((ui.available_width() - 60.0).max(80.0));
                                    ui.label(RichText::new(&track.title).color(TEXT).strong());
                                    ui.label(RichText::new(&track.artist).size(11.0).color(MUTED));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("×").on_hover_text("从列表移除").clicked()
                                    {
                                        remove = Some(index);
                                    }
                                    ui.label(
                                        RichText::new(format_duration(track.duration))
                                            .size(11.0)
                                            .color(MUTED),
                                    );
                                });
                            });
                        })
                        .response
                        .interact(Sense::click_and_drag());
                if response.double_clicked() {
                    play = Some(index);
                }
                if response.drag_started() {
                    self.dragged_track = Some(index);
                }
                if response.hovered()
                    && self.dragged_track.is_some()
                    && ui.input(|i| i.pointer.any_released())
                {
                    move_to = Some(index);
                }
            }
        });
        if let Some(index) = play {
            self.play_index(index);
        }
        if let Some(index) = remove {
            if self.playlist.current == Some(index) {
                self.command(PlayerCommand::Stop);
                self.lyrics = Lyrics::default();
            }
            self.playlist.remove(index);
        }
        if let Some(to) = move_to {
            if let Some(from) = self.dragged_track.take() {
                self.playlist.move_track(from, to);
            }
        } else if ui.input(|i| i.pointer.any_released()) {
            self.dragged_track = None;
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        let duration = self.current_track().map(|t| t.duration).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_duration(self.position))
                    .monospace()
                    .color(MUTED),
            );
            let mut value = self.position.as_secs_f32();
            let slider = egui::Slider::new(&mut value, 0.0..=duration.as_secs_f32().max(0.01))
                .show_value(false);
            if ui
                .add_sized([ui.available_width() - 55.0, 18.0], slider)
                .changed()
            {
                self.position = Duration::from_secs_f32(value);
                self.command(PlayerCommand::Seek(self.position));
            }
            ui.label(
                RichText::new(format_duration(duration))
                    .monospace()
                    .color(MUTED),
            );
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for mode in PlayMode::ALL {
                        if ui
                            .selectable_value(&mut self.mode, mode, mode.label())
                            .changed()
                        {
                            self.save_config();
                        }
                    }
                });
            ui.add_space(12.0);
            if ui.button("◀").on_hover_text("上一曲").clicked() {
                self.advance(true);
            }
            let play_text = if self.is_playing { "Ⅱ" } else { "▶" };
            if ui
                .add_sized(
                    [48.0, 38.0],
                    egui::Button::new(RichText::new(play_text).size(20.0).color(BG)).fill(ACCENT),
                )
                .clicked()
            {
                if self.playlist.current.is_none() && !self.playlist.tracks.is_empty() {
                    self.play_index(0);
                } else if self.is_playing {
                    self.command(PlayerCommand::Pause);
                } else {
                    self.command(PlayerCommand::Play);
                }
            }
            if ui.button("■").on_hover_text("停止").clicked() {
                self.command(PlayerCommand::Stop);
            }
            if ui.button("▶").on_hover_text("下一曲").clicked() {
                self.advance(false);
            }
            ui.add_space(16.0);
            egui::ComboBox::from_id_salt("speed")
                .selected_text(format!("{:.2}×", self.speed))
                .width(70.0)
                .show_ui(ui, |ui| {
                    for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                        if ui
                            .selectable_value(&mut self.speed, speed, format!("{speed:.2}×"))
                            .changed()
                        {
                            self.command(PlayerCommand::SetSpeed(speed));
                            self.save_config();
                        }
                    }
                });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{:02}", (self.volume * 100.0).round() as u8))
                        .monospace()
                        .color(MUTED),
                );
                let mut volume = self.volume;
                if ui
                    .add_sized(
                        [110.0, 18.0],
                        egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false),
                    )
                    .changed()
                {
                    self.volume = volume;
                    if volume > 0.0 {
                        self.previous_volume = volume;
                    }
                    self.command(PlayerCommand::SetVolume(volume));
                }
                if ui
                    .button(if self.volume == 0.0 { "×" } else { "◖" })
                    .on_hover_text("静音")
                    .clicked()
                {
                    self.volume = if self.volume == 0.0 {
                        self.previous_volume
                    } else {
                        self.previous_volume = self.volume;
                        0.0
                    };
                    self.command(PlayerCommand::SetVolume(self.volume));
                }
            });
        });
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(31, 36, 40);
    style.visuals.widgets.hovered.bg_fill = PANEL_HOVER;
    style.visuals.widgets.active.bg_fill = ACCENT;
    style.visuals.selection.bg_fill = ACCENT;
    style.spacing.item_spacing = Vec2::new(9.0, 7.0);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn configure_fonts(ctx: &egui::Context) {
    let candidates = if cfg!(target_os = "windows") {
        vec![
            r"C:\Windows\Fonts\msyh.ttc",
            r"C:\Windows\Fonts\segoeui.ttf",
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/SFNS.ttf",
        ]
    } else {
        vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    };
    if let Some((path, bytes)) = candidates
        .into_iter()
        .find_map(|p| fs::read(p).ok().map(|b| (p, b)))
    {
        let mut fonts = FontDefinitions::default();
        fonts
            .font_data
            .insert("sonora-ui".into(), FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "sonora-ui".into());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push("sonora-ui".into());
        ctx.set_fonts(fonts);
        let _ = path;
    }
}
