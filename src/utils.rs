use std::{path::Path, time::Duration};

/// Formats a duration as `mm:ss`, allowing minutes to exceed 59.
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// Derives artist and title from common `Artist - Title.mp3` filenames.
pub fn metadata_from_filename(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown track")
        .trim();
    match stem.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_owned(), title.trim().to_owned()),
        None => ("Unknown artist".to_owned(), stem.to_owned()),
    }
}

pub fn is_mp3(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_fallback_extracts_artist_and_title() {
        let (artist, title) = metadata_from_filename(Path::new("Portishead - Roads.mp3"));
        assert_eq!(artist, "Portishead");
        assert_eq!(title, "Roads");
    }

    #[test]
    fn duration_format_supports_long_tracks() {
        assert_eq!(format_duration(Duration::from_secs(3723)), "62:03");
    }
}
