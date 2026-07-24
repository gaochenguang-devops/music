use std::{
    fs::File,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use rodio::{
    Decoder, DeviceSinkBuilder, MixerDeviceSink, Player,
    cpal::traits::{DeviceTrait, HostTrait},
};

#[derive(Debug)]
pub enum PlayerCommand {
    Load { path: PathBuf, autoplay: bool },
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetVolume(f32),
    SetSpeed(f32),
    SwitchDevice(Option<String>),
    RefreshDevices,
    Shutdown,
}

#[derive(Debug)]
pub enum PlayerEvent {
    Position(Duration),
    Playing(bool),
    Finished,
    Devices {
        names: Vec<String>,
        selected: Option<String>,
    },
    Error(String),
}

pub struct AudioController {
    pub commands: Sender<PlayerCommand>,
    pub events: Receiver<PlayerEvent>,
}

impl AudioController {
    pub fn spawn() -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        thread::Builder::new()
            .name("sonora-audio".into())
            .spawn(move || audio_thread(command_rx, event_tx))
            .expect("failed to create audio thread");
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    /// Moves the event receiver out so a headless server can bridge playback
    /// events into an async WebSocket stream while retaining the audio guard.
    #[allow(dead_code)]
    pub fn take_events(&mut self) -> Receiver<PlayerEvent> {
        let (_sender, replacement) = mpsc::channel();
        std::mem::replace(&mut self.events, replacement)
    }
}

impl Drop for AudioController {
    fn drop(&mut self) {
        let _ = self.commands.send(PlayerCommand::Shutdown);
    }
}

struct AudioState {
    stream: Option<MixerDeviceSink>,
    player: Option<Player>,
    path: Option<PathBuf>,
    selected_device: Option<String>,
    volume: f32,
    speed: f32,
    loaded: bool,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            stream: None,
            player: None,
            path: None,
            selected_device: None,
            volume: 0.75,
            speed: 1.0,
            loaded: false,
        }
    }
}

fn audio_thread(commands: Receiver<PlayerCommand>, events: Sender<PlayerEvent>) {
    let mut state = AudioState::default();
    loop {
        match commands.recv_timeout(Duration::from_millis(75)) {
            Ok(PlayerCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Ok(command) => handle_command(command, &mut state, &events),
        }

        if let Some(player) = &state.player {
            let _ = events.send(PlayerEvent::Position(player.get_pos()));
            if state.loaded && player.empty() {
                state.loaded = false;
                let _ = events.send(PlayerEvent::Finished);
                let _ = events.send(PlayerEvent::Playing(false));
            }
        }
    }
}

fn handle_command(command: PlayerCommand, state: &mut AudioState, events: &Sender<PlayerEvent>) {
    let result: Result<(), String> = match command {
        PlayerCommand::Load { path, autoplay } => load_track(state, path, autoplay).map(|_| {
            let _ = events.send(PlayerEvent::Playing(autoplay));
        }),
        PlayerCommand::Play => state
            .player
            .as_ref()
            .map(|p| p.play())
            .ok_or_else(|| "尚未加载歌曲".into())
            .map(|_| {
                let _ = events.send(PlayerEvent::Playing(true));
            }),
        PlayerCommand::Pause => state
            .player
            .as_ref()
            .map(|p| p.pause())
            .ok_or_else(|| "尚未加载歌曲".into())
            .map(|_| {
                let _ = events.send(PlayerEvent::Playing(false));
            }),
        PlayerCommand::Stop => state
            .player
            .as_ref()
            .map(|p| {
                p.pause();
                let _ = p.try_seek(Duration::ZERO);
            })
            .ok_or_else(|| "尚未加载歌曲".into())
            .map(|_| {
                let _ = events.send(PlayerEvent::Position(Duration::ZERO));
                let _ = events.send(PlayerEvent::Playing(false));
            }),
        PlayerCommand::Seek(position) => state
            .player
            .as_ref()
            .ok_or_else(|| "尚未加载歌曲".into())
            .and_then(|p| p.try_seek(position).map_err(|e| format!("跳转失败: {e}"))),
        PlayerCommand::SetVolume(volume) => {
            state.volume = volume.clamp(0.0, 1.0);
            if let Some(p) = &state.player {
                p.set_volume(state.volume);
            }
            Ok(())
        }
        PlayerCommand::SetSpeed(speed) => {
            state.speed = speed.clamp(0.5, 2.0);
            if let Some(p) = &state.player {
                p.set_speed(state.speed);
            }
            Ok(())
        }
        PlayerCommand::SwitchDevice(name) => {
            switch_device(state, name).map(|_| send_devices(events, state.selected_device.clone()))
        }
        PlayerCommand::RefreshDevices => {
            send_devices(events, state.selected_device.clone());
            Ok(())
        }
        PlayerCommand::Shutdown => Ok(()),
    };
    if let Err(err) = result {
        let _ = events.send(PlayerEvent::Error(err));
    }
}

fn load_track(state: &mut AudioState, path: PathBuf, autoplay: bool) -> Result<(), String> {
    if state.stream.is_none() {
        open_output(state, state.selected_device.clone())?;
    }
    let stream = state
        .stream
        .as_ref()
        .ok_or_else(|| "没有可用的音频输出设备".to_owned())?;
    if let Some(old) = state.player.take() {
        old.stop();
    }
    let file = File::open(&path).map_err(|e| format!("无法打开 {}: {e}", path.display()))?;
    let source = Decoder::try_from(file).map_err(|e| format!("MP3 解码失败: {e}"))?;
    let player = Player::connect_new(stream.mixer());
    player.set_volume(state.volume);
    player.set_speed(state.speed);
    player.append(source);
    if !autoplay {
        player.pause();
    }
    state.path = Some(path);
    state.player = Some(player);
    state.loaded = true;
    Ok(())
}

fn switch_device(state: &mut AudioState, name: Option<String>) -> Result<(), String> {
    if state.stream.is_none() && state.player.is_none() && state.path.is_none() {
        state.selected_device = name;
        return Ok(());
    }
    let position = state
        .player
        .as_ref()
        .map(Player::get_pos)
        .unwrap_or_default();
    let paused = state.player.as_ref().is_none_or(Player::is_paused);
    let path = state.path.clone();
    if let Some(player) = state.player.take() {
        player.stop();
    }
    state.stream = None;
    open_output(state, name)?;
    if let Some(path) = path {
        load_track(state, path, !paused)?;
        if let Some(player) = &state.player {
            player
                .try_seek(position)
                .map_err(|e| format!("恢复播放位置失败: {e}"))?;
        }
    }
    Ok(())
}

fn open_output(state: &mut AudioState, requested: Option<String>) -> Result<(), String> {
    let host = rodio::cpal::default_host();
    let device = if let Some(name) = requested {
        host.output_devices()
            .map_err(|e| format!("无法枚举音频设备: {e}"))?
            .find(|device| {
                device
                    .description()
                    .map(|d| d.to_string() == name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("找不到音频设备: {name}"))?
    } else {
        host.default_output_device()
            .ok_or_else(|| "系统没有默认音频输出设备".to_owned())?
    };
    let name = device
        .description()
        .map(|d| d.to_string())
        .unwrap_or_else(|_| "Unknown device".into());
    let stream = DeviceSinkBuilder::from_device(device)
        .and_then(|builder| builder.open_sink_or_fallback())
        .map_err(|e| format!("无法打开音频设备: {e}"))?;
    state.stream = Some(stream);
    state.selected_device = Some(name);
    Ok(())
}

fn send_devices(events: &Sender<PlayerEvent>, selected: Option<String>) {
    let host = rodio::cpal::default_host();
    let names = host
        .output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|v| v.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let _ = events.send(PlayerEvent::Devices { names, selected });
}
