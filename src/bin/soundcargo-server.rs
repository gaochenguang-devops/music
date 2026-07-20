//! Headless HTTP/WebSocket controller for the SoundCargo player.
//!
//! The server shares the desktop player's rodio thread and scans `data/` in
//! the current working directory. Audio is rendered by the server machine;
//! browsers act as remote controllers and lyric/status displays.

use std::{
    env,
    net::SocketAddr,
    sync::{Arc, Mutex, mpsc::Receiver},
    thread,
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use rand::prelude::IndexedRandom;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tower::ServiceExt;
use tower_http::{services::ServeFile, trace::TraceLayer};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use SoundCargo::{
    lrc::Lyrics,
    player::{AudioController, PlayerCommand, PlayerEvent},
    playlist::{PlayMode, Playlist, Track},
};

const INDEX_HTML: &str = include_str!("../../web/index.html");

#[derive(Clone)]
struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    commands: std::sync::mpsc::Sender<PlayerCommand>,
    playlist: Mutex<Playlist>,
    position: Mutex<Duration>,
    playing: Mutex<bool>,
    volume: Mutex<f32>,
    speed: Mutex<f32>,
    mode: Mutex<PlayMode>,
    last_error: Mutex<Option<String>>,
    updates: broadcast::Sender<PlaybackSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
struct TrackView {
    index: usize,
    title: String,
    artist: String,
    album: String,
    duration_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
struct PlaybackSnapshot {
    tracks: Vec<TrackView>,
    current: Option<usize>,
    position_ms: u128,
    playing: bool,
    volume: f32,
    speed: f32,
    mode: PlayMode,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValueRequest<T> {
    value: T,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _log_guard = init_logging()?;
    let data_dir = env::current_dir()?.join("data");
    std::fs::create_dir_all(&data_dir)?;
    tracing::info!(path = %data_dir.display(), "loading music library");

    let mut playlist = Playlist::default();
    let errors = playlist.add_folder(&data_dir);
    if !errors.is_empty() {
        tracing::warn!(count = errors.len(), first_error = %errors[0], "some music files failed to load");
    }
    tracing::info!(tracks = playlist.tracks.len(), "music library loaded");

    let mut audio_controller = AudioController::spawn();
    let events = audio_controller.take_events();
    let (updates, _) = broadcast::channel(32);
    let state = AppState {
        inner: Arc::new(Inner {
            commands: audio_controller.commands.clone(),
            playlist: Mutex::new(playlist),
            position: Mutex::new(Duration::ZERO),
            playing: Mutex::new(false),
            volume: Mutex::new(0.75),
            speed: Mutex::new(1.0),
            mode: Mutex::new(PlayMode::Sequential),
            last_error: Mutex::new(None),
            updates,
        }),
    };
    spawn_event_bridge(events, state.clone());

    let app = Router::new()
        .route("/", get(index))
        .route("/api/state", get(state_snapshot))
        .route("/api/library", get(state_snapshot))
        .route("/api/lyrics/{index}", get(lyrics))
        .route("/api/audio/{index}", get(audio))
        .route("/api/events", get(events_ws))
        .route("/api/player/play", post(play))
        .route("/api/player/pause", post(pause))
        .route("/api/player/stop", post(stop))
        .route("/api/player/next", post(next))
        .route("/api/player/previous", post(previous))
        .route("/api/player/track/{index}", post(select_track))
        .route("/api/player/seek", post(seek))
        .route("/api/player/volume", post(volume))
        .route("/api/player/speed", post(speed))
        .route("/api/player/mode", post(mode))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind = env::var("SOUNDCARGO_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_owned());
    let address: SocketAddr = bind.parse()?;
    tracing::info!(%address, "SoundCargo server listening");
    axum::serve(tokio::net::TcpListener::bind(address).await?, app).await?;
    drop(audio_controller);
    Ok(())
}

fn init_logging() -> Result<tracing_appender::non_blocking::WorkerGuard, Box<dyn std::error::Error>>
{
    let log_dir = env::current_dir()?.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let file = tracing_appender::rolling::daily(log_dir, "soundcargo-server.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .try_init()?;
    Ok(guard)
}

fn spawn_event_bridge(events: Receiver<PlayerEvent>, state: AppState) {
    thread::Builder::new()
        .name("soundcargo-server-events".into())
        .spawn(move || {
            let mut last_position_push = Instant::now() - Duration::from_secs(1);
            while let Ok(event) = events.recv() {
                let immediate = !matches!(event, PlayerEvent::Position(_));
                match event {
                    PlayerEvent::Position(position) => {
                        *state.inner.position.lock().unwrap() = position
                    }
                    PlayerEvent::Playing(playing) => *state.inner.playing.lock().unwrap() = playing,
                    PlayerEvent::Finished => {
                        if !advance_after_finish(&state) {
                            *state.inner.playing.lock().unwrap() = false;
                        }
                    }
                    PlayerEvent::Devices { .. } => {}
                    PlayerEvent::Error(error) => {
                        tracing::error!(message = %error, "audio thread error");
                        *state.inner.last_error.lock().unwrap() = Some(error)
                    }
                }
                if immediate || last_position_push.elapsed() >= Duration::from_millis(200) {
                    last_position_push = Instant::now();
                    let _ = state.inner.updates.send(snapshot(&state));
                }
            }
        })
        .expect("failed to create server event thread");
}

/// Selects the next track when the audio thread reports end-of-file.
/// Returns false when sequential mode reaches the end of the list.
fn advance_after_finish(state: &AppState) -> bool {
    let (index, path) = {
        let mut playlist = state.inner.playlist.lock().unwrap();
        let len = playlist.tracks.len();
        if len == 0 {
            return false;
        }
        let current = playlist.current.unwrap_or(0);
        let mode = *state.inner.mode.lock().unwrap();
        let next = match mode {
            PlayMode::RepeatOne => current,
            PlayMode::RepeatAll => (current + 1) % len,
            PlayMode::Shuffle => (0..len)
                .collect::<Vec<_>>()
                .choose(&mut rand::rng())
                .copied()
                .unwrap_or(current),
            PlayMode::Sequential if current + 1 < len => current + 1,
            PlayMode::Sequential => return false,
        };
        playlist.current = Some(next);
        (next, playlist.tracks[next].path.clone())
    };
    tracing::info!(index, "advancing after track finished");
    if state
        .inner
        .commands
        .send(PlayerCommand::Load {
            path,
            autoplay: true,
        })
        .is_err()
    {
        return false;
    }
    *state.inner.position.lock().unwrap() = Duration::ZERO;
    *state.inner.playing.lock().unwrap() = true;
    true
}

fn snapshot(state: &AppState) -> PlaybackSnapshot {
    let playlist = state.inner.playlist.lock().unwrap();
    PlaybackSnapshot {
        tracks: playlist.tracks.iter().enumerate().map(track_view).collect(),
        current: playlist.current,
        position_ms: state.inner.position.lock().unwrap().as_millis(),
        playing: *state.inner.playing.lock().unwrap(),
        volume: *state.inner.volume.lock().unwrap(),
        speed: *state.inner.speed.lock().unwrap(),
        mode: *state.inner.mode.lock().unwrap(),
        error: state.inner.last_error.lock().unwrap().clone(),
    }
}

fn track_view((index, track): (usize, &Track)) -> TrackView {
    TrackView {
        index,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_ms: track.duration.as_millis(),
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn state_snapshot(State(state): State<AppState>) -> Json<PlaybackSnapshot> {
    Json(snapshot(&state))
}

async fn lyrics(
    Path(index): Path<usize>,
    State(state): State<AppState>,
) -> Result<Json<Lyrics>, ApiError> {
    let path = state
        .inner
        .playlist
        .lock()
        .unwrap()
        .tracks
        .get(index)
        .and_then(|track| track.lrc_path.clone())
        .ok_or_else(|| ApiError::not_found("该歌曲没有歌词"))?;
    tracing::debug!(index, path = %path.display(), "loading lyrics");
    Lyrics::from_file(&path)
        .map(Json)
        .map_err(|error| ApiError::bad_request(error.to_string()))
}

/// Streams an MP3 from the server's data directory to a browser `<audio>`
/// element. ServeFile handles range requests so browser seeking works.
async fn audio(
    Path(index): Path<usize>,
    State(state): State<AppState>,
    request: axum::http::Request<Body>,
) -> Response {
    let path = state
        .inner
        .playlist
        .lock()
        .unwrap()
        .tracks
        .get(index)
        .map(|track| track.path.clone());
    let Some(path) = path else {
        return (StatusCode::NOT_FOUND, "歌曲不存在").into_response();
    };
    tracing::info!(index, path = %path.display(), "serving audio stream");
    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => response.map(Body::new),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn events_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket(socket, state))
}

async fn websocket(mut socket: WebSocket, state: AppState) {
    tracing::info!("websocket client connected");
    let mut updates = state.inner.updates.subscribe();
    let initial = serde_json::to_string(&snapshot(&state)).unwrap_or_default();
    if socket.send(Message::Text(initial.into())).await.is_err() {
        return;
    }
    while let Ok(update) = updates.recv().await {
        let Ok(text) = serde_json::to_string(&update) else {
            continue;
        };
        if socket.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
    tracing::info!("websocket client disconnected");
}

async fn play(State(state): State<AppState>) -> Result<Json<PlaybackSnapshot>, ApiError> {
    let (index, already_loaded) = {
        let mut playlist = state.inner.playlist.lock().unwrap();
        let already_loaded = playlist.current.is_some();
        if !already_loaded {
            playlist.current = (!playlist.tracks.is_empty()).then_some(0);
        }
        (playlist.current, already_loaded)
    };
    if let Some(index) = index {
        if already_loaded {
            tracing::info!("resuming current track");
            state
                .inner
                .commands
                .send(PlayerCommand::Play)
                .map_err(ApiError::send)?;
        } else {
            let path = state.inner.playlist.lock().unwrap().tracks[index]
                .path
                .clone();
            state
                .inner
                .commands
                .send(PlayerCommand::Load {
                    path,
                    autoplay: true,
                })
                .map_err(ApiError::send)?;
        }
        *state.inner.playing.lock().unwrap() = true;
        Ok(Json(snapshot(&state)))
    } else {
        Err(ApiError::bad_request("播放列表为空"))
    }
}

async fn pause(State(state): State<AppState>) -> Result<Json<PlaybackSnapshot>, ApiError> {
    command(&state, PlayerCommand::Pause)
}

async fn stop(State(state): State<AppState>) -> Result<Json<PlaybackSnapshot>, ApiError> {
    command(&state, PlayerCommand::Stop)
}

async fn next(State(state): State<AppState>) -> Result<Json<PlaybackSnapshot>, ApiError> {
    select_relative(&state, 1).await
}

async fn previous(State(state): State<AppState>) -> Result<Json<PlaybackSnapshot>, ApiError> {
    select_relative(&state, -1).await
}

async fn select_track(
    Path(index): Path<usize>,
    State(state): State<AppState>,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    load_index(&state, index, true)
}

async fn select_relative(
    state: &AppState,
    delta: isize,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    let index = {
        let playlist = state.inner.playlist.lock().unwrap();
        let len = playlist.tracks.len();
        if len == 0 {
            return Err(ApiError::bad_request("播放列表为空"));
        }
        let current = playlist.current.unwrap_or(0) as isize;
        ((current + delta).rem_euclid(len as isize)) as usize
    };
    load_index(state, index, true)
}

fn load_index(
    state: &AppState,
    index: usize,
    autoplay: bool,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    let path = {
        let mut playlist = state.inner.playlist.lock().unwrap();
        let track = playlist
            .tracks
            .get(index)
            .ok_or_else(|| ApiError::not_found("歌曲不存在"))?;
        let path = track.path.clone();
        playlist.current = Some(index);
        path
    };
    tracing::info!(index, autoplay, path = %path.display(), "loading track");
    state
        .inner
        .commands
        .send(PlayerCommand::Load { path, autoplay })
        .map_err(ApiError::send)?;
    *state.inner.playing.lock().unwrap() = autoplay;
    *state.inner.position.lock().unwrap() = Duration::ZERO;
    Ok(Json(snapshot(state)))
}

async fn seek(
    State(state): State<AppState>,
    Json(input): Json<ValueRequest<f64>>,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    if !input.value.is_finite() || input.value < 0.0 {
        return Err(ApiError::bad_request("无效的跳转时间"));
    }
    command(
        &state,
        PlayerCommand::Seek(Duration::from_secs_f64(input.value)),
    )
}

async fn volume(
    State(state): State<AppState>,
    Json(input): Json<ValueRequest<f32>>,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    let value = input.value.clamp(0.0, 1.0);
    *state.inner.volume.lock().unwrap() = value;
    command(&state, PlayerCommand::SetVolume(value))
}

async fn speed(
    State(state): State<AppState>,
    Json(input): Json<ValueRequest<f32>>,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    let value = input.value.clamp(0.5, 2.0);
    *state.inner.speed.lock().unwrap() = value;
    command(&state, PlayerCommand::SetSpeed(value))
}

async fn mode(
    State(state): State<AppState>,
    Json(input): Json<ValueRequest<PlayMode>>,
) -> Result<Json<PlaybackSnapshot>, ApiError> {
    tracing::info!(mode = ?input.value, "changing play mode");
    *state.inner.mode.lock().unwrap() = input.value;
    Ok(Json(snapshot(&state)))
}

fn command(state: &AppState, command: PlayerCommand) -> Result<Json<PlaybackSnapshot>, ApiError> {
    tracing::info!(command = ?command, "sending player command");
    state.inner.commands.send(command).map_err(ApiError::send)?;
    Ok(Json(snapshot(state)))
}

#[derive(Debug)]
struct ApiError(StatusCode, String);

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, message.into())
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self(StatusCode::NOT_FOUND, message.into())
    }
    fn send(error: std::sync::mpsc::SendError<PlayerCommand>) -> Self {
        Self(StatusCode::SERVICE_UNAVAILABLE, error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.0, self.1).into_response()
    }
}
