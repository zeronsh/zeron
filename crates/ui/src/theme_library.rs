//! Durable custom-theme library and the bridge into the active runtime registry.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use gpui::{App, Global};
use zeron_theme::vscode::SourceCompilation;
use zeron_theme::{
    CustomThemeEntry, CustomThemeLibrary, InstallMode, LibraryError, ThemeRegistry,
    replace_custom_families,
};

use crate::appearance;
use crate::i18n::{self, Locale, MessageId};
use crate::theme::Appearance;

pub struct ThemeLibraryState {
    pub data_dir: PathBuf,
    pub library: CustomThemeLibrary,
    /// Why the library file could not be read at boot. Held as the error the
    /// library (or the filesystem) raised, so a language switch rewords the
    /// strip instead of freezing the boot language into it.
    pub load_error: Option<anyhow::Error>,
}

impl Global for ThemeLibraryState {}

pub fn init(data_dir: impl Into<PathBuf>, cx: &mut App) {
    let data_dir = data_dir.into();
    let (library, load_error) = match CustomThemeLibrary::load(&data_dir) {
        Ok(library) => (library, None),
        Err(error) => {
            tracing::warn!(error = %error, "could not load custom theme library");
            (CustomThemeLibrary::default(), Some(error))
        }
    };
    library.install_runtime();
    cx.set_global(ThemeLibraryState {
        data_dir,
        library,
        load_error,
    });
}

pub fn entries(cx: &App) -> Vec<CustomThemeEntry> {
    cx.try_global::<ThemeLibraryState>()
        .map(|state| state.library.entries.clone())
        .unwrap_or_default()
}

/// The boot-time load failure in the active locale, if the library could not be
/// read.
pub fn load_warning(cx: &App) -> Option<String> {
    let error = cx.try_global::<ThemeLibraryState>()?.load_error.as_ref()?;
    Some(error_text(error, i18n::locale(cx)))
}

/// The text for a failed library operation in `locale`. This crate resolves the
/// failures it authors on the spot; the ones `zeron-theme` names are mapped
/// here; everything else (a path, a parse error, the OS's own wording) passes
/// through as the diagnostic it is.
pub fn error_text(error: &anyhow::Error, locale: Locale) -> String {
    match error.downcast_ref::<LibraryError>() {
        Some(named) => named_library_error_text(named, locale),
        None => error.to_string(),
    }
}

fn named_library_error_text(error: &LibraryError, locale: Locale) -> String {
    match error {
        LibraryError::UnknownTheme { id } => {
            i18n::fill(MessageId::ThemeLibraryUnknown, "{id}", id, locale)
        }
        LibraryError::EntryLimit { limit } => i18n::fill(
            MessageId::ThemeLibraryEntryLimit,
            "{limit}",
            &limit.to_string(),
            locale,
        ),
        LibraryError::VariantLimit { limit } => i18n::fill(
            MessageId::ThemeLibraryVariantLimit,
            "{limit}",
            &limit.to_string(),
            locale,
        ),
        LibraryError::NoVariantSelected => {
            i18n::translate(MessageId::ThemeLibraryNoVariantSelected, locale).to_string()
        }
        LibraryError::ValidationFailed { errors } => i18n::fill(
            MessageId::ThemeLibraryValidationFailed,
            "{errors}",
            &errors.join("; "),
            locale,
        ),
        LibraryError::SnapshotCannotReload => {
            i18n::translate(MessageId::ThemeLibrarySnapshotCannotReload, locale).to_string()
        }
        LibraryError::VariantCountOverflow => {
            i18n::translate(MessageId::ThemeLibraryVariantOverflow, locale).to_string()
        }
    }
}

/// The library, or the reason it is missing. Failures here are shown in the
/// settings strip, so the phrase is chosen while the locale is still at hand.
fn library_state(cx: &mut App) -> Result<&mut ThemeLibraryState> {
    let locale = i18n::locale(cx);
    if !cx.has_global::<ThemeLibraryState>() {
        return Err(anyhow!(i18n::translate(
            MessageId::ThemeLibraryNotInitialized,
            locale
        )));
    }
    Ok(cx.global_mut::<ThemeLibraryState>())
}

pub fn compile(path: &Path, family_id: &str, family_name: &str) -> Result<SourceCompilation> {
    CustomThemeLibrary::compile(path, family_id, family_name)
}

pub fn install(
    compilation: SourceCompilation,
    selected_variant_ids: &[String],
    mode: InstallMode,
    cx: &mut App,
) -> Result<String> {
    let locale = i18n::locale(cx);
    let state = library_state(cx)?;
    let mut next = state.library.clone();
    let id = next.install(compilation, selected_variant_ids, mode)?;
    persist_and_activate(state, next, locale)?;
    reconcile_and_refresh(cx);
    Ok(id)
}

pub fn reload(id: &str, cx: &mut App) -> Result<()> {
    let locale = i18n::locale(cx);
    let state = library_state(cx)?;
    let mut next = state.library.clone();
    let result = next.reload(id);
    // Reload failures update the durable warning while deliberately preserving
    // the last known good family.
    persist_and_activate(state, next, locale)?;
    reconcile_and_refresh(cx);
    result
}

pub fn unlink(id: &str, cx: &mut App) -> Result<()> {
    mutate(cx, |library| library.unlink(id))
}

pub fn duplicate_as_snapshot(id: &str, cx: &mut App) -> Result<String> {
    let locale = i18n::locale(cx);
    let state = library_state(cx)?;
    let mut next = state.library.clone();
    let duplicate = next.duplicate_as_snapshot(id)?;
    persist_and_activate(state, next, locale)?;
    reconcile_and_refresh(cx);
    Ok(duplicate)
}

pub fn duplicate_as_editable(id: &str, cx: &mut App) -> Result<String> {
    let locale = i18n::locale(cx);
    let state = library_state(cx)?;
    let mut next = state.library.clone();
    let duplicate = next.duplicate_as_editable(id, &state.data_dir)?;
    persist_and_activate(state, next, locale)?;
    reconcile_and_refresh(cx);
    Ok(duplicate)
}

pub fn remove(id: &str, cx: &mut App) -> Result<()> {
    mutate(cx, |library| {
        library
            .remove(id)
            .then_some(())
            .ok_or_else(|| anyhow::Error::new(LibraryError::UnknownTheme { id: id.to_owned() }))
    })
}

pub fn reveal(id: &str, cx: &App) -> Result<()> {
    let path = cx
        .try_global::<ThemeLibraryState>()
        .and_then(|state| state.library.entry(id))
        .and_then(|entry| entry.source.path())
        .ok_or_else(|| {
            anyhow!(i18n::translate(
                MessageId::ThemeLibraryNoSource,
                i18n::locale(cx)
            ))
        })?;
    cx.reveal_path(path);
    Ok(())
}

fn mutate<T>(
    cx: &mut App,
    operation: impl FnOnce(&mut CustomThemeLibrary) -> Result<T>,
) -> Result<T> {
    let locale = i18n::locale(cx);
    let state = library_state(cx)?;
    let mut next = state.library.clone();
    let output = operation(&mut next)?;
    persist_and_activate(state, next, locale)?;
    reconcile_and_refresh(cx);
    Ok(output)
}

fn persist_and_activate(
    state: &mut ThemeLibraryState,
    next: CustomThemeLibrary,
    locale: Locale,
) -> Result<()> {
    next.save(&state.data_dir).map_err(|error| {
        error.context(i18n::translate(MessageId::ThemeLibrarySaveFailed, locale))
    })?;
    replace_custom_families(
        next.entries
            .iter()
            .map(|entry| entry.family.clone())
            .collect(),
    );
    state.library = next;
    state.load_error = None;
    Ok(())
}

fn reconcile_and_refresh(cx: &mut App) {
    let registry = ThemeRegistry::active();
    let selected = appearance::themes(cx);
    for appearance_kind in [Appearance::Light, Appearance::Dark] {
        let model = if appearance_kind.is_light() {
            zeron_theme::Appearance::Light
        } else {
            zeron_theme::Appearance::Dark
        };
        let selected_id = selected.variant_id(model);
        if registry.variant(selected_id).is_none()
            && let Some(fallback) = registry.variants_for(model).next()
        {
            appearance::set_theme(appearance_kind, fallback.id.clone(), cx);
        }
    }
    appearance::apply_registry_change(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_persistence_leaves_library_state_unchanged() {
        let data_dir = tempfile::tempdir().unwrap();
        let blocking_file = data_dir.path().join("not-a-directory");
        std::fs::write(&blocking_file, "occupied").unwrap();
        let original = CustomThemeLibrary::default();
        let mut state = ThemeLibraryState {
            data_dir: blocking_file,
            library: original.clone(),
            load_error: Some(anyhow::anyhow!("previous warning")),
        };
        let mut next = original.clone();
        next.entries.push(CustomThemeEntry {
            id: "unpersisted".into(),
            name: "Unpersisted".into(),
            source: zeron_theme::CustomThemeSource::ImportedSnapshot {
                imported_from: None,
            },
            family: zeron_theme::ThemeFamily {
                id: "unpersisted".into(),
                name: "Unpersisted".into(),
                variants: Vec::new(),
            },
            reports: Default::default(),
            selected_variant_ids: Vec::new(),
            status: zeron_theme::CustomThemeStatus::Ready,
        });

        assert!(persist_and_activate(&mut state, next, Locale::En).is_err());
        assert_eq!(state.library, original);
        assert_eq!(
            state.load_error.as_ref().map(|error| error.to_string()),
            Some("previous warning".to_string())
        );
    }

    #[test]
    fn library_errors_render_in_the_active_locale() {
        let cases: Vec<(LibraryError, &str, &str)> = vec![
            (
                LibraryError::UnknownTheme { id: "gone".into() },
                "unknown custom theme `gone`",
                "未知的自定义主题 `gone`",
            ),
            (
                LibraryError::EntryLimit { limit: 256 },
                "custom theme library is limited to 256 entries",
                "自定义主题库最多 256 个条目",
            ),
            (
                LibraryError::VariantLimit { limit: 1024 },
                "custom theme library is limited to 1024 variants",
                "自定义主题库最多 1024 个变体",
            ),
            (
                LibraryError::NoVariantSelected,
                "select at least one successfully compiled variant",
                "请至少选择一个编译成功的变体",
            ),
            (
                LibraryError::ValidationFailed {
                    errors: vec!["dark: accent is 1.2:1".into()],
                },
                "theme validation failed: dark: accent is 1.2:1",
                "主题校验失败：dark: accent is 1.2:1",
            ),
            (
                LibraryError::SnapshotCannotReload,
                "imported snapshots cannot reload",
                "导入的快照无法重新加载",
            ),
            (
                LibraryError::VariantCountOverflow,
                "custom theme variant count overflow",
                "自定义主题的变体数量溢出",
            ),
        ];
        for (named, english, chinese) in cases {
            // The English half is pinned to the library's own wording, so the
            // two renderers cannot drift apart.
            let error = anyhow::Error::new(named.clone());
            assert_eq!(error_text(&error, Locale::En), named.english());
            assert_eq!(error_text(&error, Locale::En), english);
            assert_eq!(error_text(&error, Locale::ZhCn), chinese);
        }

        // A diagnostic carries no key: it renders as the layer wrote it, in
        // both locales.
        let diagnostic = anyhow::anyhow!("could not parse /themes/library.json");
        assert_eq!(error_text(&diagnostic, Locale::En), diagnostic.to_string());
        assert_eq!(
            error_text(&diagnostic, Locale::ZhCn),
            "could not parse /themes/library.json"
        );
    }
}
