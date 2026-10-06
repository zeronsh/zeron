use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;

const AUDIO_FORMAT: &str = "140/139/bestaudio[ext=m4a]/bestaudio[acodec*=mp4a]/bestaudio[ext=mp3]";
const MAX_TRACKS: usize = 200;
const LIST_TIMEOUT: Duration = Duration::from_secs(45);
const FETCH_TIMEOUT: Duration = Duration::from_secs(180);
const ART_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;
const ART_MIN_WIDTH: u32 = 160;
const LOCAL_EXTENSIONS: &[&str] = &["aac", "flac", "m4a", "mp3", "mp4", "oga", "ogg", "wav"];

static FETCH_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    Remote(String),
    Local(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub title: String,
    pub origin: Origin,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
}

pub fn is_local_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| LOCAL_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn local_track(path: PathBuf) -> Track {
    let title = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Track {
        title,
        origin: Origin::Local(path),
        duration: None,
        thumbnail: None,
    }
}

pub async fn fetch_art(url: &str) -> Result<Vec<u8>> {
    let response = reqwest::Client::builder()
        .timeout(ART_TIMEOUT)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_ART_BYTES)
    {
        bail!("Cover too large");
    }
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > MAX_ART_BYTES {
        bail!("Cover too large");
    }
    Ok(bytes.to_vec())
}

pub fn embedded_art(path: &Path) -> Option<Vec<u8>> {
    use symphonia::core::meta::{MetadataOptions, StandardVisualKey, Visual};
    let file = std::fs::File::open(path).ok()?;
    let stream = symphonia::core::io::MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = symphonia::core::probe::Hint::new();
    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &Default::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let pick = |visuals: &[Visual]| {
        visuals
            .iter()
            .find(|visual| visual.usage == Some(StandardVisualKey::FrontCover))
            .or_else(|| visuals.first())
            .map(|visual| visual.data.to_vec())
    };
    let container = probed
        .format
        .metadata()
        .current()
        .and_then(|revision| pick(revision.visuals()));
    container.or_else(|| {
        probed.metadata.get().and_then(|metadata| {
            metadata
                .current()
                .and_then(|revision| pick(revision.visuals()))
        })
    })
}

pub async fn resolve(input: &str) -> Result<Vec<Track>> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Nothing to play");
    }
    let path = Path::new(input.trim_matches('"'));
    if is_local_audio(path) && path.is_file() {
        return Ok(vec![local_track(path.to_path_buf())]);
    }
    let target = if input.starts_with("https://") || input.starts_with("http://") {
        input.to_string()
    } else {
        format!("ytsearch1:{}", input.replace(['\r', '\n'], " "))
    };
    let stdout = run(
        &["--flat-playlist", "--dump-single-json", "--", &target],
        LIST_TIMEOUT,
    )
    .await?;
    let tracks = parse_listing(&stdout)?;
    if tracks.is_empty() {
        bail!("No playable results");
    }
    Ok(tracks)
}

pub async fn fetch(track: &Track, dir: &Path) -> Result<PathBuf> {
    let url = match &track.origin {
        Origin::Local(path) => return Ok(path.clone()),
        Origin::Remote(url) => url,
    };
    tokio::fs::create_dir_all(dir)
        .await
        .context("Could not create the music cache")?;
    let id = FETCH_COUNTER.fetch_add(1, Ordering::Relaxed);
    let template = dir.join(format!("{id}.%(ext)s"));
    let template = template.to_string_lossy();
    let stdout = run(
        &[
            "--no-playlist",
            "--format",
            AUDIO_FORMAT,
            "--output",
            &template,
            "--no-simulate",
            "--print",
            "after_move:filepath",
            "--",
            url,
        ],
        FETCH_TIMEOUT,
    )
    .await?;
    let path = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("yt-dlp returned no file"))?;
    if !path.is_file() {
        bail!("yt-dlp returned no file");
    }
    Ok(path)
}

async fn run(args: &[&str], timeout: Duration) -> Result<String> {
    let program = std::env::var_os("ZERON_YTDLP").unwrap_or_else(|| "yt-dlp".into());
    let mut command = tokio::process::Command::new(program);
    command
        .args([
            "--ignore-config",
            "--no-warnings",
            "--no-progress",
            "--no-color",
            "--socket-timeout",
            "15",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            anyhow!("yt-dlp is not installed")
        } else {
            anyhow!("Could not start yt-dlp: {err}")
        }
    })?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("yt-dlp timed out"))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .rev()
            .find(|line| line.contains("ERROR"))
            .map(|line| line.trim_start_matches("ERROR:").trim().to_string())
            .unwrap_or_else(|| format!("yt-dlp exited with {}", output.status));
        if reason.contains("HTTP Error 403") {
            bail!("Blocked by the site. Update yt-dlp (yt-dlp -U)");
        }
        bail!(reason);
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Deserialize)]
struct Listing {
    #[serde(default)]
    entries: Option<Vec<Entry>>,
    #[serde(flatten)]
    item: Entry,
}

#[derive(Deserialize, Default)]
struct Entry {
    title: Option<String>,
    url: Option<String>,
    webpage_url: Option<String>,
    original_url: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    thumbnails: Option<Vec<Thumbnail>>,
}

#[derive(Deserialize)]
struct Thumbnail {
    url: String,
    width: Option<u32>,
}

impl Entry {
    fn art_url(thumbnail: Option<String>, thumbnails: Option<Vec<Thumbnail>>) -> Option<String> {
        let thumbnails = thumbnails.unwrap_or_default();
        thumbnails
            .iter()
            .filter(|thumb| thumb.width.is_some_and(|width| width >= ART_MIN_WIDTH))
            .min_by_key(|thumb| thumb.width)
            .map(|thumb| thumb.url.clone())
            .or(thumbnail)
            .or_else(|| thumbnails.last().map(|thumb| thumb.url.clone()))
            .filter(|url| url.starts_with("http"))
    }

    fn into_track(self) -> Option<Track> {
        let url = self
            .webpage_url
            .or(self.url)
            .or(self.original_url)
            .filter(|url| url.starts_with("http"))?;
        Some(Track {
            thumbnail: Self::art_url(self.thumbnail, self.thumbnails),
            title: self
                .title
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| url.clone()),
            origin: Origin::Remote(url),
            duration: self.duration.filter(|d| d.is_finite() && *d > 0.0),
        })
    }
}

fn parse_listing(stdout: &str) -> Result<Vec<Track>> {
    let listing: Listing = serde_json::from_str(stdout).context("yt-dlp returned invalid JSON")?;
    Ok(match listing.entries {
        Some(entries) => entries
            .into_iter()
            .filter_map(Entry::into_track)
            .take(MAX_TRACKS)
            .collect(),
        None => listing.item.into_track().into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlists_keep_entry_order_and_skip_unplayable_rows() {
        let json = r#"{"_type":"playlist","title":"Mix","entries":[
            {"title":"One","url":"https://www.youtube.com/watch?v=a","duration":61.0},
            {"title":"Private video","url":null},
            {"title":"Two","url":"https://www.youtube.com/watch?v=b"}]}"#;
        let tracks = parse_listing(json).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "One");
        assert_eq!(tracks[0].duration, Some(61.0));
        assert_eq!(
            tracks[1].origin,
            Origin::Remote("https://www.youtube.com/watch?v=b".into())
        );
    }

    #[test]
    fn single_items_prefer_the_page_url() {
        let json = r#"{"title":"Song","webpage_url":"https://soundcloud.com/x/y","url":"https://cdn/x.mp3","duration":200}"#;
        let tracks = parse_listing(json).unwrap();
        assert_eq!(
            tracks,
            vec![Track {
                title: "Song".into(),
                origin: Origin::Remote("https://soundcloud.com/x/y".into()),
                duration: Some(200.0),
                thumbnail: None,
            }]
        );
    }

    #[test]
    fn art_prefers_the_smallest_sharp_thumbnail() {
        let json = r#"{"_type":"playlist","entries":[
            {"url":"https://www.youtube.com/watch?v=a","thumbnails":[
                {"url":"https://i.ytimg.com/a/default.jpg","width":120},
                {"url":"https://i.ytimg.com/a/mq.jpg","width":320},
                {"url":"https://i.ytimg.com/a/hq.jpg","width":480}]},
            {"url":"https://x.com/b","thumbnail":"https://x.com/b.jpg"},
            {"url":"https://x.com/c","thumbnails":[{"url":"https://x.com/c.webp"}]}]}"#;
        let art: Vec<_> = parse_listing(json)
            .unwrap()
            .into_iter()
            .map(|track| track.thumbnail)
            .collect();
        assert_eq!(
            art,
            vec![
                Some("https://i.ytimg.com/a/mq.jpg".into()),
                Some("https://x.com/b.jpg".into()),
                Some("https://x.com/c.webp".into()),
            ]
        );
    }

    #[test]
    fn local_audio_is_recognized_by_extension() {
        assert!(is_local_audio(Path::new("C:/music/a.MP3")));
        assert!(is_local_audio(Path::new("/x/b.flac")));
        assert!(!is_local_audio(Path::new("/x/c.opus")));
        assert!(!is_local_audio(Path::new("https://youtube.com")));
    }
}
