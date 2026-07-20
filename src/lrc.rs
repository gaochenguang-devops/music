use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use anyhow::{Context, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LrcMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub time: Duration,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lyrics {
    pub metadata: LrcMetadata,
    pub lines: Vec<LyricLine>,
}

impl Lyrics {
    pub fn from_file(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("无法读取歌词: {}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(Self::parse(&text))
    }

    /// Parses standard LRC tags. Invalid fragments are ignored without losing valid tags.
    pub fn parse(input: &str) -> Self {
        let mut metadata = LrcMetadata::default();
        let mut timed: BTreeMap<Duration, Vec<String>> = BTreeMap::new();

        for raw in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let mut rest = raw;
            let mut stamps = Vec::new();
            while let Some(body) = rest.strip_prefix('[').and_then(|s| s.split_once(']')) {
                let (tag, tail) = body;
                rest = tail;
                if let Some(time) = parse_timestamp(tag) {
                    stamps.push(time);
                } else if stamps.is_empty() {
                    parse_metadata(tag, &mut metadata);
                }
            }

            let lyric = rest.trim();
            if lyric.is_empty() || stamps.is_empty() {
                continue;
            }
            for stamp in stamps {
                timed.entry(stamp).or_default().push(lyric.to_owned());
            }
        }

        let lines = timed
            .into_iter()
            .flat_map(|(time, texts)| texts.into_iter().map(move |text| LyricLine { time, text }))
            .collect();
        Self { metadata, lines }
    }

    pub fn active_index(&self, position: Duration) -> Option<usize> {
        self.lines
            .partition_point(|line| line.time <= position)
            .checked_sub(1)
    }
}

fn parse_metadata(tag: &str, metadata: &mut LrcMetadata) {
    let Some((key, value)) = tag.split_once(':') else {
        return;
    };
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    let slot = match key.trim().to_ascii_lowercase().as_str() {
        "ti" => &mut metadata.title,
        "ar" => &mut metadata.artist,
        "al" => &mut metadata.album,
        "by" => &mut metadata.author,
        _ => return,
    };
    *slot = Some(value.to_owned());
}

fn parse_timestamp(tag: &str) -> Option<Duration> {
    let (minutes, seconds) = tag.trim().split_once(':')?;
    let minutes: u64 = minutes.parse().ok()?;
    let seconds: f64 = seconds.parse().ok()?;
    if !seconds.is_finite() || !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(Duration::from_secs_f64(minutes as f64 * 60.0 + seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_multiple_tags_and_duplicate_times() {
        let lrc = Lyrics::parse(
            "[ti:Roads]\n[ar:Portishead]\n[00:01.25][00:03]Oh, can't anybody see\n[00:03.00]Second voice\ninvalid",
        );
        assert_eq!(lrc.metadata.title.as_deref(), Some("Roads"));
        assert_eq!(lrc.metadata.artist.as_deref(), Some("Portishead"));
        assert_eq!(lrc.lines.len(), 3);
        assert_eq!(lrc.lines[1].time, Duration::from_secs(3));
        assert_eq!(lrc.lines[2].time, Duration::from_secs(3));
    }

    #[test]
    fn active_line_follows_playback_position() {
        let lrc = Lyrics::parse("[00:01]one\n[00:02.50]two");
        assert_eq!(lrc.active_index(Duration::from_millis(2499)), Some(0));
        assert_eq!(lrc.active_index(Duration::from_millis(2500)), Some(1));
    }
}
