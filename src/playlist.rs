use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use id3::{Tag, TagLike};
use serde::{Deserialize, Serialize};
use symphonia::core::{
    formats::{FormatOptions, TrackType, probe::Hint},
    io::MediaSourceStream,
    meta::MetadataOptions,
    units::Timestamp,
};
use walkdir::WalkDir;

use crate::{
    lrc::Lyrics,
    utils::{is_mp3, metadata_from_filename},
};

#[derive(Debug, Clone)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub cover: Option<Vec<u8>>,
    pub lrc_path: Option<PathBuf>,
}

impl Track {
    pub fn from_path(path: PathBuf) -> Result<Self> {
        if !is_mp3(&path) {
            anyhow::bail!("仅支持 MP3 文件: {}", path.display());
        }
        let (fallback_artist, fallback_title) = metadata_from_filename(&path);
        let tag = Tag::read_from_path(&path).ok();
        let title = tag
            .as_ref()
            .and_then(TagLike::title)
            .map(str::to_owned)
            .unwrap_or(fallback_title);
        let mut artist = tag
            .as_ref()
            .and_then(TagLike::artist)
            .map(str::to_owned)
            .unwrap_or(fallback_artist);
        let album = tag
            .as_ref()
            .and_then(TagLike::album)
            .unwrap_or("Unknown album")
            .to_owned();
        let cover = tag
            .as_ref()
            .and_then(|t| t.pictures().next())
            .map(|p| p.data.clone());
        let lrc_candidate = path.with_extension("lrc");
        let lrc_path = lrc_candidate.exists().then_some(lrc_candidate);

        if let Some(ref lrc_path) = lrc_path
            && let Ok(lyrics) = Lyrics::from_file(lrc_path)
            && artist == "Unknown artist"
            && let Some(lrc_artist) = lyrics.metadata.artist
        {
            artist = lrc_artist;
        }

        let duration = probe_duration(&path).unwrap_or_default();
        Ok(Self {
            path,
            title,
            artist,
            album,
            duration,
            cover,
            lrc_path,
        })
    }
}

fn probe_duration(path: &Path) -> Result<Duration> {
    let file = File::open(path).with_context(|| format!("无法打开音频: {}", path.display()))?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("Symphonia 无法探测 MP3")?;
    let track = probed
        .default_track(TrackType::Audio)
        .context("MP3 中没有音轨")?;
    let time_base = track.time_base.context("音频未提供时间基准")?;
    let duration = track.duration.context("音频未提供时长")?;
    let timestamp = Timestamp::try_from(duration.get()).context("音频时长超出范围")?;
    let time = time_base.calc_time(timestamp).context("无法计算音频时长")?;
    let (seconds, nanos) = time.parts();
    let seconds = u64::try_from(seconds).context("音频时长为负数")?;
    Ok(Duration::new(seconds, nanos))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayMode {
    #[default]
    Sequential,
    RepeatOne,
    RepeatAll,
    Shuffle,
}

impl PlayMode {
    pub const ALL: [Self; 4] = [
        Self::Sequential,
        Self::RepeatOne,
        Self::RepeatAll,
        Self::Shuffle,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Sequential => "顺序播放",
            Self::RepeatOne => "单曲循环",
            Self::RepeatAll => "列表循环",
            Self::Shuffle => "随机播放",
        }
    }
}

#[derive(Default)]
pub struct Playlist {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
}

impl Playlist {
    pub fn add_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> Vec<String> {
        let mut errors = Vec::new();
        for path in paths {
            if self.tracks.iter().any(|track| track.path == path) {
                continue;
            }
            match Track::from_path(path) {
                Ok(track) => self.tracks.push(track),
                Err(err) => errors.push(err.to_string()),
            }
        }
        errors
    }

    pub fn add_folder(&mut self, folder: &Path) -> Vec<String> {
        let paths = WalkDir::new(folder)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_mp3(entry.path()))
            .map(|entry| entry.into_path());
        self.add_files(paths)
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }
        self.tracks.remove(index);
        self.current = match self.current {
            Some(current) if current == index => None,
            Some(current) if current > index => Some(current - 1),
            other => other,
        };
    }

    pub fn move_track(&mut self, from: usize, to: usize) {
        if from >= self.tracks.len() || to >= self.tracks.len() || from == to {
            return;
        }
        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
        self.current = self.current.map(|current| {
            if current == from {
                to
            } else if from < current && current <= to {
                current - 1
            } else if to <= current && current < from {
                current + 1
            } else {
                current
            }
        });
    }
}
