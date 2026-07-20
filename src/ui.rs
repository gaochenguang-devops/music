use std::{fs, path::PathBuf, time::Duration};

use directories::ProjectDirs;
use eframe::egui::{
    self, Align, Color32, FontData, FontDefinitions, FontFamily, FontId, Layout, RichText, Sense,
    Shadow, Stroke, TextureHandle, Vec2,
};
use rand::prelude::IndexedRandom;
use serde::{Deserialize, Serialize};

use crate::{
    lrc::Lyrics,
    player::{AudioController, PlayerCommand, PlayerEvent},
    playlist::{PlayMode, Playlist, Track},
    utils::format_duration,
};

const BG: Color32 = Color32::from_rgb(232, 239, 242);
const SURFACE: Color32 = Color32::from_rgb(250, 252, 253);
const SURFACE_ALT: Color32 = Color32::from_rgb(242, 247, 249);
const PANEL_HOVER: Color32 = Color32::from_rgb(232, 243, 246);
const TEXT: Color32 = Color32::from_rgb(25, 37, 47);
const MUTED: Color32 = Color32::from_rgb(112, 128, 139);
const ACCENT: Color32 = Color32::from_rgb(14, 179, 197);
const ACCENT_DARK: Color32 = Color32::from_rgb(4, 126, 142);
const ACCENT_PALE: Color32 = Color32::from_rgb(222, 247, 246);
const BORDER: Color32 = Color32::from_rgb(214, 225, 230);
const SHADOW: Shadow = Shadow {
    offset: [0, 4],
    blur: 18,
    spread: 0,
    color: Color32::from_black_alpha(24),
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppConfig {
    volume: f32,
    mode: PlayMode,
    speed: f32,
    dark_theme: bool,
    device: Option<String>,
    #[serde(default = "default_playlist_open")]
    playlist_open: bool,
}

fn default_playlist_open() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            volume: 0.75,
            mode: PlayMode::Sequential,
            speed: 1.0,
            dark_theme: false,
            device: None,
            playlist_open: true,
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
    playlist_open: bool,
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
        let mut app = Self {
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
            playlist_open: config.playlist_open,
        };

        let startup_paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        for path in startup_paths {
            let errors = if path.is_dir() {
                app.playlist.add_folder(&path)
            } else {
                app.playlist.add_files([path])
            };
            app.report_errors(errors);
        }
        if !app.playlist.tracks.is_empty() {
            app.play_index(0);
        }
        app
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
            dark_theme: false,
            device: self.selected_device.clone(),
            playlist_open: self.playlist_open,
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
        ui.painter().rect_filled(ui.max_rect(), 0, BG);

        egui::Panel::top("header")
            .exact_size(118.0)
            .frame(
                surface_frame()
                    .outer_margin(egui::Margin::symmetric(18, 12))
                    .inner_margin(egui::Margin::symmetric(22, 14)),
            )
            .show(ui, |ui| self.header(ui));
        egui::Panel::bottom("controls")
            .exact_size(136.0)
            .frame(
                surface_frame()
                    .outer_margin(egui::Margin::symmetric(18, 12))
                    .inner_margin(egui::Margin::symmetric(22, 12)),
            )
            .show(ui, |ui| self.controls(ui));
        if self.playlist_open {
            egui::Panel::right("playlist")
                .resizable(true)
                .default_size(360.0)
                .size_range(280.0..=480.0)
                .frame(
                    surface_frame()
                        .outer_margin(egui::Margin {
                            left: 6,
                            right: 18,
                            top: 8,
                            bottom: 8,
                        })
                        .inner_margin(egui::Margin {
                            left: 28,
                            right: 16,
                            top: 16,
                            bottom: 16,
                        }),
                )
                .show(ui, |ui| self.playlist_panel(ui));
        } else {
            egui::Panel::right("playlist-collapsed")
                .exact_size(48.0)
                .frame(
                    surface_frame()
                        .outer_margin(egui::Margin {
                            left: 6,
                            right: 18,
                            top: 8,
                            bottom: 8,
                        })
                        .inner_margin(egui::Margin::same(8)),
                )
                .show(ui, |ui| {
                    ui.add_space(((ui.available_height() - 44.0) * 0.5).max(0.0));
                    ui.vertical_centered(|ui| {
                        if ui
                            .add_sized([32.0, 44.0], egui::Button::new("◀"))
                            .on_hover_text("展开播放列表")
                            .clicked()
                        {
                            self.playlist_open = true;
                        }
                    });
                });
        }
        egui::CentralPanel::default()
            .frame(
                surface_frame()
                    .outer_margin(egui::Margin {
                        left: 18,
                        right: 6,
                        top: 8,
                        bottom: 8,
                    })
                    .inner_margin(egui::Margin::same(18)),
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

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }
}

impl MusicApp {
    fn header(&mut self, ui: &mut egui::Ui) {
        let (title, artist, album) = self
            .current_track()
            .map(|track| {
                (
                    track.title.clone(),
                    track.artist.clone(),
                    track.album.clone(),
                )
            })
            .unwrap_or_else(|| {
                (
                    "选择一首音乐".to_owned(),
                    "音乐正在等待".to_owned(),
                    "本地音乐".to_owned(),
                )
            });
        let cover = self.cover_texture.clone();
        let info_width = (ui.available_width() * 0.46).clamp(320.0, 560.0);

        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(info_width, 72.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if let Some(texture) = cover {
                        ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(Vec2::splat(64.0))
                                .corner_radius(10),
                        );
                    } else {
                        let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
                        ui.painter().rect_filled(rect, 10, SURFACE_ALT);
                        ui.painter()
                            .circle_stroke(rect.center(), 18.0, Stroke::new(2.0, MUTED));
                        ui.painter().circle_filled(rect.center(), 4.5, ACCENT);
                    }
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&title).size(22.0).color(TEXT).strong())
                                .truncate(),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{artist}  ·  {album}"))
                                    .size(13.0)
                                    .color(MUTED),
                            )
                            .truncate(),
                        );
                        egui::Frame::new()
                            .fill(ACCENT_PALE)
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(8, 3))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(if self.lyrics.lines.is_empty() {
                                        "MP3"
                                    } else {
                                        "LRC SYNC"
                                    })
                                    .size(10.0)
                                    .color(ACCENT_DARK)
                                    .strong(),
                                );
                            });
                    });
                },
            );
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let selected = self
                    .selected_device
                    .clone()
                    .unwrap_or_else(|| "默认输出设备".into());
                egui::ComboBox::from_id_salt("device")
                    .selected_text(selected)
                    .width((ui.available_width() - 12.0).clamp(130.0, 220.0))
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("主歌词").size(15.0).color(TEXT).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("+")
                    .on_hover_text("选择 LRC 歌词")
                    .clicked()
                {
                    self.load_manual_lrc();
                }
                let title = self
                    .current_track()
                    .map(|track| track.title.as_str())
                    .unwrap_or("等待播放");
                ui.label(RichText::new(title).size(12.0).color(MUTED));
            });
        });
        ui.add_space(8.0);
        let available = ui.available_size();
        let active = self.lyrics.active_index(self.position);
        let lyrics_width = available.x.max(120.0);
        ui.allocate_ui_with_layout(
            Vec2::new(lyrics_width, available.y),
            Layout::top_down(Align::Center).with_cross_justify(true),
            |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("lyrics-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(lyrics_width);
                        ui.add_space(available.y * 0.38);
                        if self.lyrics.lines.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(50.0);
                                ui.label(RichText::new("暂无歌词").size(22.0).color(MUTED));
                            });
                        } else {
                            let mut active_response = None;
                            for index in 0..self.lyrics.lines.len() {
                                let line = &self.lyrics.lines[index];
                                let is_active = active == Some(index);
                                let font_size = if is_active { 25.0 } else { 18.0 };
                                let color = if is_active { TEXT } else { MUTED };
                                let row_width = (lyrics_width - 40.0).max(80.0);
                                let mut layout_job = egui::text::LayoutJob::simple(
                                    line.text.clone(),
                                    FontId::new(font_size, FontFamily::Proportional),
                                    color,
                                    row_width - 24.0,
                                );
                                layout_job.halign = Align::Center;
                                let galley = ui.painter().layout_job(layout_job);
                                let row_height = (galley.size().y + 28.0).max(if is_active {
                                    76.0
                                } else {
                                    52.0
                                });
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(row_width, row_height),
                                    Sense::click(),
                                );
                                if is_active {
                                    ui.painter().rect_filled(rect, 10, ACCENT_PALE);
                                    ui.painter().rect_stroke(
                                        rect,
                                        10,
                                        Stroke::new(1.0, ACCENT),
                                        egui::StrokeKind::Inside,
                                    );
                                } else if response.hovered() {
                                    ui.painter().rect_filled(rect, 8, PANEL_HOVER);
                                }
                                let text_position = rect.center() - galley.rect.center().to_vec2();
                                ui.painter().galley(text_position, galley, color);
                                if response.clicked() {
                                    self.command(PlayerCommand::Seek(line.time));
                                    self.position = line.time;
                                }
                                if is_active {
                                    active_response = Some(response);
                                }
                            }
                            // Request scrolling only after every row has contributed to
                            // the scroll area's full content height.
                            if self.scroll_to_lyric != active {
                                if let Some(response) = active_response {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                if active.is_some() {
                                    self.scroll_to_lyric = active;
                                }
                            }
                        }
                        ui.add_space(available.y * 0.38);
                    });
            },
        );
    }

    fn playlist_panel(&mut self, ui: &mut egui::Ui) {
        let toggle_rect = egui::Rect::from_center_size(
            egui::pos2(ui.clip_rect().left() + 14.0, ui.clip_rect().center().y),
            Vec2::new(26.0, 48.0),
        );
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("播放列表").size(18.0).color(TEXT).strong());
                ui.label(
                    RichText::new(format!("{} 首本地音乐", self.playlist.tracks.len()))
                        .size(11.0)
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_sized([30.0, 30.0], egui::Button::new("×"))
                    .on_hover_text("清空列表")
                    .clicked()
                {
                    self.playlist = Playlist::default();
                    self.lyrics = Lyrics::default();
                    self.command(PlayerCommand::Stop);
                }
                if ui
                    .add_sized([46.0, 30.0], egui::Button::new("目录"))
                    .clicked()
                {
                    self.add_folder();
                }
                if ui
                    .add_sized([30.0, 30.0], egui::Button::new("+"))
                    .on_hover_text("添加 MP3")
                    .clicked()
                {
                    self.add_files();
                }
            });
        });
        ui.add_space(14.0);
        let mut play = None;
        let mut remove = None;
        let mut move_to = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, track) in self.playlist.tracks.iter().enumerate() {
                let current = self.playlist.current == Some(index);
                let frame = egui::Frame::new()
                    .fill(if current {
                        ACCENT_PALE
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(10, 9));
                let row = frame.show(ui, |ui| {
                    let mut play_requested = false;
                    let mut drag_started = false;
                    ui.horizontal(|ui| {
                        let drag_response =
                            ui.add(
                                egui::Label::new(
                                    RichText::new(if current { "▶" } else { "≡" })
                                        .color(if current { ACCENT } else { MUTED }),
                                )
                                .sense(Sense::drag()),
                            );
                        drag_started = drag_response.drag_started();
                        let info_response = ui
                            .vertical(|ui| {
                                ui.set_width((ui.available_width() - 60.0).max(80.0));
                                ui.label(
                                    RichText::new(&track.title).size(14.0).color(TEXT).strong(),
                                );
                                ui.label(RichText::new(&track.artist).size(11.0).color(MUTED));
                            })
                            .response
                            .interact(Sense::click());
                        play_requested = info_response.double_clicked();
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("从列表移除").clicked() {
                                remove = Some(index);
                            }
                            ui.label(
                                RichText::new(format_duration(track.duration))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        });
                    });
                    (play_requested, drag_started)
                });
                if row.inner.0 {
                    play = Some(index);
                }
                if row.inner.1 {
                    self.dragged_track = Some(index);
                }
                if row.response.hovered()
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
        if ui
            .put(toggle_rect, egui::Button::new("▶"))
            .on_hover_text("收起播放列表")
            .clicked()
        {
            self.playlist_open = false;
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
            let slider_width = (ui.available_width() - 55.0).max(80.0);
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(slider_width, 20.0), Sense::click_and_drag());
            let rail = egui::Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
            ui.painter().rect_filled(rail, 2, BORDER);
            let fraction = if duration.is_zero() {
                0.0
            } else {
                (self.position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
            };
            let knob_x = egui::lerp(rail.x_range(), fraction);
            let played = egui::Rect::from_min_max(rail.min, egui::pos2(knob_x, rail.max.y));
            ui.painter().rect_filled(played, 2, ACCENT);
            ui.painter()
                .circle_filled(egui::pos2(knob_x, rail.center().y), 7.0, SURFACE);
            ui.painter().circle_stroke(
                egui::pos2(knob_x, rail.center().y),
                7.0,
                Stroke::new(2.0, ACCENT),
            );
            if (response.clicked() || response.dragged())
                && let Some(pointer) = response.interact_pointer_pos()
            {
                let fraction = ((pointer.x - rail.left()) / rail.width()).clamp(0.0, 1.0);
                self.position = duration.mul_f32(fraction);
                self.command(PlayerCommand::Seek(self.position));
            }
            ui.label(
                RichText::new(format_duration(duration))
                    .monospace()
                    .color(MUTED),
            );
        });
        ui.add_space(8.0);
        ui.columns(3, |columns| {
            columns[0].scope(|ui| {
                ui.spacing_mut().interact_size.y = 42.0;
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("mode")
                        .selected_text(self.mode.label())
                        .width(150.0)
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
                    egui::ComboBox::from_id_salt("speed")
                        .selected_text(format!("{:.2}×", self.speed))
                        .width(96.0)
                        .show_ui(ui, |ui| {
                            for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                                if ui
                                    .selectable_value(
                                        &mut self.speed,
                                        speed,
                                        format!("{speed:.2}×"),
                                    )
                                    .changed()
                                {
                                    self.command(PlayerCommand::SetSpeed(speed));
                                    self.save_config();
                                }
                            }
                        });
                });
            });

            columns[1].with_layout(Layout::top_down(Align::Center), |ui| {
                ui.horizontal(|ui| {
                    const TRANSPORT_WIDTH: f32 = 203.0;
                    ui.add_space(((ui.available_width() - TRANSPORT_WIDTH) * 0.5).max(0.0));
                    if ui
                        .add_sized(
                            [42.0, 42.0],
                            egui::Button::new(RichText::new("⏮").color(ACCENT_DARK))
                                .corner_radius(21),
                        )
                        .on_hover_text("上一曲")
                        .clicked()
                    {
                        self.advance(true);
                    }
                    let play_text = if self.is_playing { "⏸" } else { "▶" };
                    if ui
                        .add_sized(
                            [50.0, 42.0],
                            egui::Button::new(
                                RichText::new(play_text).size(20.0).color(Color32::WHITE),
                            )
                            .fill(ACCENT)
                            .corner_radius(21),
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
                    if ui
                        .add_sized(
                            [42.0, 42.0],
                            egui::Button::new(RichText::new("⏹").color(MUTED)).corner_radius(21),
                        )
                        .on_hover_text("停止")
                        .clicked()
                    {
                        self.command(PlayerCommand::Stop);
                    }
                    if ui
                        .add_sized(
                            [42.0, 42.0],
                            egui::Button::new(RichText::new("⏭").color(ACCENT_DARK))
                                .corner_radius(21),
                        )
                        .on_hover_text("下一曲")
                        .clicked()
                    {
                        self.advance(false);
                    }
                });
            });

            columns[2].with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                    .button(if self.volume == 0.0 { "×" } else { "🎵" })
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

fn surface_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(14)
        .shadow(SHADOW)
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = SURFACE;
    style.visuals.window_stroke = Stroke::new(1.0, BORDER);
    style.visuals.window_shadow = SHADOW;
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.faint_bg_color = SURFACE_ALT;
    style.visuals.extreme_bg_color = Color32::WHITE;
    style.visuals.widgets.inactive.bg_fill = SURFACE_ALT;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.2, MUTED);
    style.visuals.widgets.hovered.bg_fill = PANEL_HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, ACCENT_DARK);
    style.visuals.widgets.active.bg_fill = ACCENT;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    style.visuals.selection.bg_fill = ACCENT_PALE;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT_DARK);
    style.visuals.slider_trailing_fill = true;
    style.spacing.item_spacing = Vec2::new(9.0, 7.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style_of(egui::Theme::Light, style);
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
