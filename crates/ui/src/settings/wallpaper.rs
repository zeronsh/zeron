//! Folder-backed wallpaper rotation. Scan and decode away from the UI thread.
use std::path::{Path, PathBuf};

use gpui::{App, Global, Task};
use std::collections::VecDeque;

const HISTORY_LIMIT: usize = 8;

pub(super) fn remember(history: &mut Vec<PathBuf>, source: &Path) {
    history.retain(|path| path != source);
    history.insert(0, source.to_path_buf());
    history.truncate(HISTORY_LIMIT);
}

fn choose(
    folder: &Path,
    history: &[PathBuf],
) -> Result<
    (
        PathBuf,
        crate::attachments::StagedAttachment,
        image::DynamicImage,
    ),
    String,
> {
    let entries = std::fs::read_dir(folder).map_err(|_| {
        "Unable to read the wallpaper folder. Choose an accessible folder in Appearance."
            .to_string()
    })?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "Unable to read the wallpaper folder.".to_string())?;
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff"
                )
            });
        if supported && path.is_file() {
            // Independent random ranks give a random order without a new RNG dependency.
            candidates.push((uuid::Uuid::new_v4(), path));
        }
    }
    // Leave at least two random choices when the folder permits it. If those
    // files cannot decode, fall back through history from oldest to newest.
    let cooldown = candidates.len().saturating_sub(2).clamp(1, HISTORY_LIMIT);
    candidates.sort_by_key(|(rank, path)| {
        let recency = history
            .iter()
            .take(cooldown)
            .position(|recent| recent == path)
            .map_or(0, |position| cooldown - position);
        (recency, *rank)
    });
    for (_, path) in candidates {
        if let Ok(staged) = crate::attachments::stage_file_verbatim(&path)
            && let Ok(image) = crate::new_thread_background_image::decode(staged.bytes())
        {
            return Ok((path, staged, image));
        }
    }
    Err("No readable wallpapers found in this folder. Add images such as PNG or JPEG, or choose another folder.".into())
}

const LOOKAHEAD: usize = 3;

#[derive(Clone, PartialEq, Eq)]
struct QueueKey {
    folder: Option<PathBuf>,
    history: Vec<PathBuf>,
    /// Managed path only: adjusting the active image's framing must not
    /// discard the decoded lookahead.
    background: Option<String>,
    effect: super::NewThreadBackgroundEffect,
    light: bool,
    data_dir: PathBuf,
}

impl QueueKey {
    fn current(cx: &App) -> Self {
        let settings = super::current(cx);
        let mut history = settings.wallpaper_history;
        if let Some(source) = settings.wallpaper_source {
            remember(&mut history, &source);
        }
        Self {
            folder: settings.wallpaper_folder,
            history,
            background: settings
                .new_thread_composer_background
                .map(|background| background.path),
            effect: settings.new_thread_background_effect,
            light: cx
                .try_global::<crate::theme::Theme>()
                .is_some_and(|theme| matches!(theme.appearance, crate::theme::Appearance::Light)),
            data_dir: cx
                .try_global::<super::SettingsStore>()
                .map(|store| store.data_dir.clone())
                .unwrap_or_default(),
        }
    }
}

struct Candidate {
    source: PathBuf,
    color: Option<zeron_theme::Color>,
    file: super::PreparedBackgroundFile,
    artwork: crate::new_thread_background_effects::PreloadedArtwork,
}

impl Candidate {
    fn load(key: &QueueKey, history: &[PathBuf]) -> Result<Self, String> {
        let folder = key
            .folder
            .as_deref()
            .ok_or_else(|| "Choose a wallpaper folder in Appearance first.".to_string())?;
        let (source, staged, image) = choose(folder, history)?;
        let artwork = crate::new_thread_background_effects::PreloadedArtwork::load(
            &image, key.effect, key.light,
        );
        drop(image);
        let color = artwork.color();
        let file = super::prepare_background_file(staged, &key.data_dir)?;
        Ok(Self {
            source,
            color,
            file,
            artwork,
        })
    }
}

#[derive(Default)]
struct PreloadQueue {
    key: Option<QueueKey>,
    ready: VecDeque<Candidate>,
    generation: u64,
    loading: bool,
    error: Option<String>,
}
impl Global for PreloadQueue {}

/// Preloaded copies still queued when the app exits never run their `Drop`
/// cleanup. Before this process prepares any of its own, retire managed files
/// other than the active background so earlier sessions' copies don't pile up.
fn remove_orphaned_preloads(key: &QueueKey) {
    if key.data_dir.as_os_str().is_empty() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(key.data_dir.join(super::NEW_THREAD_BACKGROUND_DIR)) else {
        return;
    };
    // Compare names, not full paths, so a differently spelled data directory
    // can never make the active image look orphaned.
    let active = key
        .background
        .as_deref()
        .and_then(|path| Path::new(path).file_name().map(|name| name.to_owned()));
    for entry in entries.flatten() {
        let name = entry.file_name();
        let managed = name
            .to_str()
            .is_some_and(|name| name.starts_with("new-thread-background-"));
        if managed && active.as_deref() != Some(name.as_os_str()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn synchronize(cx: &mut App) {
    let key = QueueKey::current(cx);
    if cx.try_global::<PreloadQueue>().is_none() {
        remove_orphaned_preloads(&key);
        cx.set_global(PreloadQueue::default());
    }
    let queue = cx.global_mut::<PreloadQueue>();
    if queue.key.as_ref() != Some(&key) {
        queue.ready.clear();
        queue.generation = queue.generation.wrapping_add(1);
        queue.loading = false;
        queue.error = None;
        queue.key = Some(key);
    }
}

/// Called on render as well as after a switch: warms on launch and invalidates
/// when the folder, manual background, effect, or light/dark appearance changes.
pub fn preload(cx: &mut App) {
    synchronize(cx);
    let queue = cx.global_mut::<PreloadQueue>();
    let key = queue.key.as_ref().unwrap().clone();
    if key.folder.is_none()
        || queue.loading
        || queue.error.is_some()
        || queue.ready.len() >= LOOKAHEAD
    {
        return;
    }
    let mut history = key.history.clone();
    for candidate in &queue.ready {
        remember(&mut history, &candidate.source);
    }
    queue.loading = true;
    let generation = queue.generation;
    let worker = cx
        .background_executor()
        .spawn(async move { Candidate::load(&key, &history) });
    cx.spawn(async move |cx| {
        let result = worker.await;
        cx.update(|cx| {
            synchronize(cx);
            let queue = cx.global_mut::<PreloadQueue>();
            if queue.generation != generation {
                return;
            }
            queue.loading = false;
            match result {
                Ok(candidate) => queue.ready.push_back(candidate),
                Err(error) => queue.error = Some(error),
            }
            preload(cx);
        });
    })
    .detach();
}

fn take_ready(cx: &mut App) -> Option<Result<(), String>> {
    let candidate = cx.global_mut::<PreloadQueue>().ready.pop_front()?;
    let path = candidate.file.path().to_path_buf();
    let result = super::commit_background(&candidate.source, candidate.file, candidate.color, cx);
    if result.is_ok() {
        // Seed the exact managed path the renderer reads. No file read, decode,
        // resize, or effect generation is needed on the next frame.
        candidate.artwork.install(&path);
        let key = QueueKey::current(cx);
        cx.global_mut::<PreloadQueue>().key = Some(key);
        preload(cx);
    } else {
        cx.global_mut::<PreloadQueue>().key = None;
    }
    Some(result)
}

pub fn randomize(cx: &mut App) -> Task<Result<(), String>> {
    synchronize(cx);
    // A user request retries a failed speculative preload (e.g. a folder that
    // has since been restored), but background rendering never spins on errors.
    cx.global_mut::<PreloadQueue>().error = None;
    preload(cx);
    let folder = super::current(cx).wallpaper_folder;
    let generation = cx.global::<PreloadQueue>().generation;
    if let Some(result) = take_ready(cx) {
        return cx.spawn(async move |_| result);
    }
    cx.spawn(async move |cx| {
        if folder.is_none() {
            return Err("Choose a wallpaper folder in Appearance first.".into());
        }
        loop {
            let result = cx.update(|cx| {
                if super::current(cx).wallpaper_folder != folder {
                    return Some(Ok(()));
                }
                preload(cx);
                if cx.global::<PreloadQueue>().generation != generation {
                    return Some(Ok(()));
                }
                take_ready(cx).or_else(|| cx.global::<PreloadQueue>().error.clone().map(Err))
            });
            if let Some(result) = result {
                return result;
            }
            // Only a cold/fully exhausted queue waits; warm shortcuts commit
            // synchronously above. Keep image work on the background executor.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(16))
                .await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallpaper_folder(root: &Path) -> PathBuf {
        let folder = root.join("wallpapers");
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..12 {
            image::RgbaImage::from_pixel(4, 4, image::Rgba([i * 10, 100, 200, 255]))
                .save(folder.join(format!("{i}.png")))
                .unwrap();
        }
        folder
    }

    #[gpui::test]
    fn preload_three_refills_and_warm_switch_needs_no_source_reads(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let folder = wallpaper_folder(dir.path());
        cx.update(|cx| {
            cx.set_global(crate::theme::Theme::default());
            let settings = super::super::UiSettings {
                wallpaper_folder: Some(folder.clone()),
                new_thread_background_effect: super::super::NewThreadBackgroundEffect::Ascii,
                ..Default::default()
            };
            super::super::init(settings, dir.path().join("data"), cx);
            preload(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let queue = cx.global::<PreloadQueue>();
            assert_eq!(queue.ready.len(), LOOKAHEAD);
            let sources: std::collections::HashSet<_> =
                queue.ready.iter().map(|entry| &entry.source).collect();
            assert_eq!(sources.len(), LOOKAHEAD);
            assert!(super::super::current(cx).wallpaper_history.is_empty());
            assert!(
                super::super::current(cx)
                    .new_thread_composer_background
                    .is_none()
            );
            randomize(cx).detach();
            assert_eq!(cx.global::<PreloadQueue>().ready.len(), LOOKAHEAD - 1);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(cx.global::<PreloadQueue>().ready.len(), LOOKAHEAD);
            let settings = super::super::current(cx);
            for candidate in &cx.global::<PreloadQueue>().ready {
                assert!(!settings.wallpaper_history.contains(&candidate.source));
            }
            // All queued selections and their effects must be usable even if
            // the original files disappear after preloading.
            std::fs::remove_dir_all(&folder).unwrap();
            for _ in 0..LOOKAHEAD {
                let expected = cx.global::<PreloadQueue>().ready[0].source.clone();
                randomize(cx).detach();
                let settings = super::super::current(cx);
                assert_eq!(settings.wallpaper_source.as_ref(), Some(&expected));
                let path = PathBuf::from(settings.new_thread_composer_background.unwrap().path);
                assert!(path.is_file());
                let theme = crate::theme::Theme::default();
                assert!(
                    crate::new_thread_background_effects::prepare(
                        settings.new_thread_background_effect,
                        &theme,
                        &path,
                        cx,
                    )
                    .is_some(),
                    "ready switch must reuse the decoded image and effect immediately"
                );
            }
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn warm_switch_resets_previous_background_adjustment(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let folder = wallpaper_folder(dir.path());
        let data = dir.path().join("data");
        cx.update(|cx| {
            cx.set_global(crate::theme::Theme::default());
            super::super::init(
                super::super::UiSettings {
                    wallpaper_folder: Some(folder.clone()),
                    ..Default::default()
                },
                &data,
                cx,
            );
            super::super::install_new_thread_composer_background(&folder.join("0.png"), cx)
                .unwrap();
            preload(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            // Framing belongs to the active image; it must not flush the
            // decoded lookahead for the next shuffle.
            let generation = cx.global::<PreloadQueue>().generation;
            super::super::set_new_thread_background_adjustment(
                super::super::NewThreadBackgroundAdjustment {
                    focal_x: 0.2,
                    focal_y: 0.8,
                    zoom: 2.0,
                },
                cx,
            );
            preload(cx);
            assert_eq!(cx.global::<PreloadQueue>().generation, generation);
            assert_eq!(cx.global::<PreloadQueue>().ready.len(), LOOKAHEAD);
            let expected = cx.global::<PreloadQueue>().ready[0].source.clone();
            randomize(cx).detach();
            assert_eq!(cx.global::<PreloadQueue>().ready.len(), LOOKAHEAD - 1);
            let settings = super::super::current(cx);
            assert_eq!(settings.wallpaper_source.as_ref(), Some(&expected));
            assert_eq!(
                settings
                    .new_thread_composer_background
                    .as_ref()
                    .unwrap()
                    .adjustment,
                super::super::NewThreadBackgroundAdjustment::default(),
            );
            assert_eq!(super::super::UiSettings::load(&data), settings);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn first_preload_retires_copies_left_by_a_previous_session(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let backgrounds = data.join(super::super::NEW_THREAD_BACKGROUND_DIR);
        std::fs::create_dir_all(&backgrounds).unwrap();
        let active = backgrounds.join("new-thread-background-active.png");
        let orphan = backgrounds.join("new-thread-background-orphan.png");
        let unrelated = backgrounds.join("keep.txt");
        for path in [&active, &orphan, &unrelated] {
            std::fs::write(path, b"x").unwrap();
        }
        cx.update(|cx| {
            super::super::init(
                super::super::UiSettings {
                    new_thread_composer_background: Some(
                        super::super::NewThreadComposerBackground {
                            path: active.to_string_lossy().into_owned(),
                            name: "active.png".into(),
                            adjustment: super::super::NewThreadBackgroundAdjustment::default(),
                        },
                    ),
                    ..Default::default()
                },
                &data,
                cx,
            );
            preload(cx);
        });
        assert!(active.is_file());
        assert!(unrelated.is_file());
        assert!(!orphan.exists());
    }

    #[gpui::test]
    fn stale_preloads_are_discarded_and_unused_managed_files_are_removed(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let folder = wallpaper_folder(dir.path());
        let data = dir.path().join("data");
        cx.update(|cx| {
            super::super::init(
                super::super::UiSettings {
                    wallpaper_folder: Some(folder.clone()),
                    ..Default::default()
                },
                &data,
                cx,
            );
            preload(cx);
            // Invalidate before the first background job can publish.
            super::super::update(super::super::SavePolicy::Immediate, cx, |settings| {
                settings.wallpaper_folder = None
            });
            preload(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert!(cx.global::<PreloadQueue>().ready.is_empty());
            super::super::update(super::super::SavePolicy::Immediate, cx, |settings| {
                settings.wallpaper_folder = Some(folder)
            });
            preload(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let files: Vec<_> = cx
                .global::<PreloadQueue>()
                .ready
                .iter()
                .map(|entry| entry.file.path().to_path_buf())
                .collect();
            assert_eq!(files.len(), LOOKAHEAD);
            assert!(files.iter().all(|path| path.is_file()));
            super::super::update(super::super::SavePolicy::Immediate, cx, |settings| {
                settings.new_thread_background_effect =
                    super::super::NewThreadBackgroundEffect::Dither
            });
            synchronize(cx);
            assert!(cx.global::<PreloadQueue>().ready.is_empty());
            assert!(files.iter().all(|path| !path.exists()));
        });
        assert_eq!(
            std::fs::read_dir(data.join(super::super::NEW_THREAD_BACKGROUND_DIR))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn wallpaper_settings_migrate_and_round_trip() {
        let mut settings: super::super::UiSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.wallpaper_folder.is_none());
        assert_eq!(settings.keymap.random_wallpaper, "mod-u");
        settings.wallpaper_folder = Some("/wallpapers".into());
        settings.wallpaper_source = Some("/wallpapers/current.png".into());
        settings.wallpaper_history = vec![
            "/wallpapers/current.png".into(),
            "/wallpapers/older.png".into(),
        ];
        settings.keymap.set(
            super::super::ShortcutId::RandomWallpaper,
            "mod-shift-u".into(),
        );
        let saved = serde_json::to_string(&settings).unwrap();
        let loaded: super::super::UiSettings = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn recent_wallpapers_cool_down_and_small_folders_keep_random_choices() {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..10)
            .map(|i| dir.path().join(format!("{i}.png")))
            .collect();
        for path in &paths {
            image::RgbaImage::new(2, 2).save(path).unwrap();
        }
        let mut history = paths[..8].to_vec();
        for _ in 0..40 {
            let selected = choose(dir.path(), &history).unwrap().0;
            assert!(!history.contains(&selected));
            remember(&mut history, &selected);
            assert_eq!(history.len(), HISTORY_LIMIT);
        }
        for path in &paths[4..] {
            std::fs::remove_file(path).unwrap();
        }
        // Four images leave two eligible choices, avoiding a fixed cycle.
        let history = paths[..4].to_vec();
        for _ in 0..20 {
            let selected = choose(dir.path(), &history).unwrap().0;
            assert!(paths[2..4].contains(&selected));
        }
        // Corrupt unplayed images must not force a repeat of the newest image.
        std::fs::write(&paths[2], b"broken").unwrap();
        std::fs::write(&paths[3], b"broken").unwrap();
        assert_eq!(choose(dir.path(), &history).unwrap().0, paths[1]);
    }

    #[test]
    fn rotation_skips_invalid_files_and_avoids_immediate_repeats() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.PNG");
        let second = dir.path().join("second.png");
        let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 100, 200, 255]));
        pixels
            .save_with_format(&first, image::ImageFormat::Png)
            .unwrap();
        pixels.save(&second).unwrap();
        std::fs::write(dir.path().join("broken.png"), b"not an image").unwrap();
        std::fs::create_dir(dir.path().join("directory.png")).unwrap();
        for _ in 0..10 {
            assert_eq!(
                choose(dir.path(), std::slice::from_ref(&first)).unwrap().0,
                second
            );
            assert_eq!(
                choose(dir.path(), std::slice::from_ref(&second)).unwrap().0,
                first
            );
        }
        std::fs::remove_file(&second).unwrap();
        assert_eq!(
            choose(dir.path(), std::slice::from_ref(&first)).unwrap().0,
            first
        );
        std::fs::remove_file(&first).unwrap();
        assert!(choose(dir.path(), &[]).is_err());
        assert!(choose(&dir.path().join("missing"), &[]).is_err());
    }
}
