//! Desktop localization: English plus Simplified Chinese.
//!
//! Design (docs/plans/2026-09-18-desktop-i18n.md): keys are stable semantic ids
//! ([`MessageId`]), copy is compiled into the binary, and the resolved
//! [`Locale`] lives in a gpui global. English display text is never used as a
//! key, so rewording copy cannot break a translation, and rendering does no
//! lookup, allocation, or platform query.
//!
//! # Why a global and not an atomic
//!
//! One process owns one [`gpui::App`], so a global carries the same meaning as a
//! process-wide atomic — and gpui's `TestAppContext` builds a fresh `App` per
//! test, which isolates locale between tests instead of forcing the suite to run
//! single-threaded.
//!
//! # Why switching repaints every window
//!
//! Copy is read during `render()`, and `Context::notify` repaints only the entity
//! it notifies. [`set_preference`] therefore ends with `App::refresh_windows`,
//! the same mechanism [`crate::appearance::apply`] uses: it marks every window
//! dirty *and* disables gpui's per-view prepaint cache for the frame, so each
//! view re-runs `render` and reads the new locale.

use gpui::{App, Global};
use serde::{Deserialize, Deserializer, Serialize};
use zeron_proto::view::CompactAge;

use crate::settings::{self, SavePolicy};

/// The user's language choice, persisted in `ui-settings.json`.
///
/// Device-local by design: it is not part of the Loro document, the account
/// profile, or multi-device sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguagePreference {
    /// Follow the machine's language.
    System,
    English,
    SimplifiedChinese,
}

impl Default for LanguagePreference {
    /// `System`, so a machine set to Simplified Chinese opens in Chinese. The
    /// copy migration left the UI chrome translated; what is still English is
    /// diagnostics (the theme importer's file messages, raw engine payloads),
    /// not a half-translated interface. See
    /// docs/plans/2026-09-18-desktop-i18n.md.
    fn default() -> Self {
        Self::System
    }
}

impl<'de> Deserialize<'de> for LanguagePreference {
    /// Tolerant on purpose. `UiSettings::load` deserializes the whole file at
    /// once and falls back to `UiSettings::default()` on *any* field error, so an
    /// unrecognized value here would silently reset themes, keymap, and pane
    /// widths. An unknown value means "follow the machine".
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        Ok(match value.as_deref() {
            Some("english") => Self::English,
            Some("simplifiedChinese") => Self::SimplifiedChinese,
            // "system", null, hand-edited values, and ids written by a future
            // build that this one does not know.
            _ => Self::System,
        })
    }
}

impl LanguagePreference {
    /// Every choice offered by the Appearance page, in display order.
    pub const ALL: [Self; 3] = [Self::System, Self::English, Self::SimplifiedChinese];

    /// Stable, locale-independent id for element ids and tests.
    pub const fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "english",
            Self::SimplifiedChinese => "simplified-chinese",
        }
    }
}

/// The locality a resolved preference selects. Rendering reads this, never the
/// preference itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    En,
    ZhCn,
}

impl Locale {
    /// Language name in its own language, for the settings status line.
    pub const fn endonym(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
        }
    }
}

macro_rules! messages {
    ($( $id:ident = { en: $en:literal, zh: $zh:literal } ),* $(,)?) => {
        /// A stable semantic message key. Keys are append-only for the lifetime
        /// of the product: renaming one forces a retranslation, so reword copy
        /// instead of renaming the id.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum MessageId {
            $( $id ),*
        }

        impl MessageId {
            /// Every key, for coverage checks.
            pub const ALL: &'static [MessageId] = &[ $( MessageId::$id ),* ];

            /// The English copy. Always present; a missing one is a build error.
            pub fn english(self) -> &'static str {
                match self {
                    $( MessageId::$id => $en ),*
                }
            }

            /// The Simplified Chinese copy, or `None` while the row still
            /// declares `zh: ""` (tracked by the coverage allowlist).
            pub fn chinese(self) -> Option<&'static str> {
                match self {
                    $( MessageId::$id => (!$zh.is_empty()).then_some($zh) ),*
                }
            }
        }
    };
}

messages! {
    // Application menu (app_menus.rs). Product names stay untranslated.
    AppMenuAbout = { en: "About Zeron", zh: "关于 Zeron" },
    AppMenuSettings = { en: "Settings", zh: "设置" },
    AppMenuServices = { en: "Services", zh: "服务" },
    AppMenuHide = { en: "Hide Zeron", zh: "隐藏 Zeron" },
    AppMenuHideOthers = { en: "Hide Others", zh: "隐藏其他" },
    AppMenuShowAll = { en: "Show All", zh: "全部显示" },
    AppMenuQuit = { en: "Quit Zeron", zh: "退出 Zeron" },
    EditMenu = { en: "Edit", zh: "编辑" },
    EditUndo = { en: "Undo", zh: "撤销" },
    EditRedo = { en: "Redo", zh: "重做" },
    EditCut = { en: "Cut", zh: "剪切" },
    EditCopy = { en: "Copy", zh: "拷贝" },
    EditPaste = { en: "Paste", zh: "粘贴" },
    EditSelectAll = { en: "Select All", zh: "全选" },
    ViewMenu = { en: "View", zh: "显示" },
    ViewAppearanceSystem = { en: "Appearance: System", zh: "外观：跟随系统" },
    ViewAppearanceLight = { en: "Appearance: Light", zh: "外观：浅色" },
    ViewAppearanceDark = { en: "Appearance: Dark", zh: "外观：深色" },
    WindowMenu = { en: "Window", zh: "窗口" },
    WindowMinimize = { en: "Minimize", zh: "最小化" },
    WindowZoom = { en: "Zoom", zh: "缩放" },
    WindowClose = { en: "Close Window", zh: "关闭窗口" },

    // Settings → Appearance.
    AppearanceTitle = { en: "Appearance", zh: "外观" },
    AppearanceSubtitle = {
        en: "Choose how Zeron looks. These settings stay on this device.",
        zh: "选择 Zeron 的外观。这些设置只保存在本机。"
    },
    AppearanceThemeLight = { en: "Light theme", zh: "浅色主题" },
    AppearanceThemeDark = { en: "Dark theme", zh: "深色主题" },
    AppearanceThemeActiveHint = {
        en: "Used whenever this appearance is active.",
        zh: "该外观生效时使用。"
    },
    AppearanceAccentColor = { en: "Accent color", zh: "强调色" },
    AppearanceGlass = { en: "Glass", zh: "玻璃质感" },
    AppearanceModeSystem = { en: "System", zh: "跟随系统" },
    AppearanceModeLight = { en: "Light", zh: "浅色" },
    AppearanceModeDark = { en: "Dark", zh: "深色" },

    // Settings → Appearance → Language.
    LanguageLabel = { en: "Language", zh: "语言" },
    LanguageSystem = { en: "System", zh: "跟随系统" },
    LanguageEnglish = { en: "English", zh: "English" },
    LanguageSimplifiedChinese = { en: "Simplified Chinese", zh: "简体中文" },
    LanguageHint = {
        en: "Applies to the interface and the menu bar.",
        zh: "作用于界面文案与菜单栏。"
    },

    // Settings → Appearance: font slots.
    FontKindUiLabel = { en: "Interface font", zh: "界面字体" },
    FontKindTerminalLabel = { en: "Terminal font", zh: "终端字体" },
    FontKindCodeLabel = { en: "Code & diff font", zh: "代码与差异字体" },
    FontKindUiDescription = { en: "Menus, sidebars, and conversation text.", zh: "菜单、侧边栏和会话文本。" },
    FontKindTerminalDescription = {
        en: "Terminal panes and shell output. Fixed-width families only.",
        zh: "终端面板和 shell 输出。仅限等宽字体。"
    },
    FontKindCodeDescription = { en: "Code blocks, diffs, and workspace file editors.", zh: "代码块、差异和工作区文件编辑器。" },
    AppearanceSearchFonts = { en: "Search fonts", zh: "搜索字体" },
    FontPickerNoMatches = { en: "No matching fonts", zh: "没有匹配的字体" },
    FontPickerNoFonts = { en: "No fonts", zh: "没有字体" },
    FontFallbackUnavailable = {
        en: "{slot} \"{family}\" isn't available on this device. Using {fallback}.",
        zh: "{slot}“{family}”在此设备上不可用，将使用 {fallback}。"
    },

    // Settings → Appearance: conversation width.
    AppearanceConversationWidth = { en: "Conversation width", zh: "会话宽度" },
    AppearanceConversationWidthHint = {
        en: "Maximum width of messages. Adapts to smaller windows.",
        zh: "消息的最大宽度。会随窗口变小而自适应。"
    },
    CommonReset = { en: "Reset", zh: "重置" },

    // Settings → Appearance: accent and surface helpers.
    AccentHelperDefault = {
        en: "Theme default · Uses the palette's intended color.",
        zh: "主题默认 · 使用配色方案预设的颜色。"
    },
    AccentHelperPreset = {
        en: "{accent} · Controls, glyphs, selections, code, and activity.",
        zh: "{accent} · 用于控件、图形、选中状态、代码和活动。"
    },
    SurfaceThemeDefault = { en: "Theme default", zh: "主题默认" },
    SurfaceFrosted = { en: "Frosted", zh: "磨砂" },
    SurfaceOpaque = { en: "Opaque", zh: "不透明" },
    SurfaceHelperThemeDefault = {
        en: "Uses this theme's {treatment} default.",
        zh: "使用该主题的{treatment}默认设置。"
    },
    SurfaceTreatmentFrosted = { en: "frosted", zh: "磨砂" },
    SurfaceTreatmentOpaque = { en: "opaque", zh: "不透明" },
    SurfaceHelperFrosted = {
        en: "Theme-colored glass where supported.",
        zh: "在支持的环境中使用主题色玻璃效果。"
    },
    SurfaceHelperOpaque = { en: "Solid surfaces for every theme.", zh: "所有主题均使用纯色表面。" },

    // Settings → Appearance: theme import dialog.
    ThemeImportTitle = { en: "Add a theme", zh: "添加主题" },
    ThemeImportSubtitle = {
        en: "Import a local theme into your library or keep it linked to its source.",
        zh: "将本地主题导入到主题库，或保持与来源的链接。"
    },
    ThemeImportSourcePlaceholder = {
        en: "Theme file, package.json, or extension folder",
        zh: "主题文件、package.json 或扩展目录"
    },
    ThemeImportSourceRequired = {
        en: "Choose a local theme file or extension folder.",
        zh: "请选择本地主题文件或扩展目录。"
    },
    ThemeImportChooseSource = { en: "Choose Theme Source", zh: "选择主题来源" },
    ThemeImportChooseBackground = {
        en: "Choose New Thread Composer Background",
        zh: "选择新会话输入框背景"
    },
    ThemeImportSource = { en: "Source", zh: "来源" },
    ThemeImportKeepUpdated = { en: "Keep it up to date", zh: "保持最新" },
    ThemeImportModeCopy = { en: "Import a copy", zh: "导入副本" },
    ThemeImportModeCopyHint = {
        en: "Works independently from the original file.",
        zh: "独立于原始文件，可独立使用。"
    },
    ThemeImportModeLink = { en: "Link to source", zh: "链接到来源" },
    ThemeImportModeLinkHint = {
        en: "Reload changes from the file on disk.",
        zh: "从磁盘文件重新加载改动。"
    },
    ThemeImportDetected = { en: "Detected themes", zh: "检测到的主题" },
    ThemeImportSelectVariant = {
        en: "Select at least one variant to import.",
        zh: "请至少选择一个要导入的变体。"
    },
    ThemeImportHideDetails = { en: "Hide details", zh: "隐藏详情" },
    ThemeImportShowDetails = { en: "Details", zh: "详情" },
    ThemeImportCompileFailed = {
        en: "{name} could not be compiled · {message}",
        zh: "{name} 无法编译 · {message}"
    },
    ThemeImportAutoVariants = {
        en: "Zeron finds light and dark variants automatically.",
        zh: "Zeron 会自动查找浅色与深色变体。"
    },
    ThemeImportActionImport = { en: "Import selected", zh: "导入所选" },
    ThemeImportActionAnalyze = { en: "Analyze theme", zh: "分析主题" },
    CountVariantOne = { en: "{n} variant", zh: "{n} 个变体" },
    CountVariantMany = { en: "{n} variants", zh: "{n} 个变体" },
    // Shared counted nouns. English needs a plural form and Chinese does not,
    // so a count picks between two templates everywhere it is shown.
    CountFileOne = { en: "{n} file", zh: "{n} 个文件" },
    CountFileMany = { en: "{n} files", zh: "{n} 个文件" },
    CountCommitOne = { en: "{n} commit", zh: "{n} 个提交" },
    CountCommitMany = { en: "{n} commits", zh: "{n} 个提交" },
    // Transcript review-comment badge and the prompt rail's bucket card.
    CountCommentOne = { en: "{n} comment", zh: "{n} 条评论" },
    CountCommentMany = { en: "{n} comments", zh: "{n} 条评论" },
    CountPromptOne = { en: "{n} prompt", zh: "{n} 条提示" },
    CountPromptMany = { en: "{n} prompts", zh: "{n} 条提示" },

    // Settings → Appearance: theme mapping dialog and report panel.
    ThemeReviewTitle = { en: "Theme mapping", zh: "主题映射" },
    ThemeReportSummary = {
        en: "{mapped} mapped · {adjusted} adjusted · {inferred} inferred/fallback · {unsupported} unsupported · {warnings} warnings · {validation} validation",
        zh: "{mapped} 已映射 · {adjusted} 已调整 · {inferred} 推断/回退 · {unsupported} 不支持 · {warnings} 条警告 · {validation} 项校验"
    },
    ThemeReportAdjusted = {
        en: "Adjusted · {role} {original} → {resolved} · {reason}",
        zh: "已调整 · {role} {original} → {resolved} · {reason}"
    },
    ThemeReportFallback = { en: "Fallback · {message}", zh: "回退 · {message}" },
    ThemeReportWarning = { en: "Warning · {message}", zh: "警告 · {message}" },
    ThemeReportValidation = {
        en: "Validation {category} {severity} · {message}",
        zh: "校验 {category} {severity} · {message}"
    },
    ThemeReportUnsupported = { en: "Unsupported · {message}", zh: "不支持 · {message}" },
    // The report's two enum labels, named here instead of `Debug`-formatted so
    // the panel reads as copy in both locales.
    ThemeReportCategoryStructural = { en: "Structural", zh: "结构" },
    ThemeReportCategoryContrast = { en: "Contrast", zh: "对比度" },
    ThemeReportSeverityWarning = { en: "Warning", zh: "警告" },
    ThemeReportSeverityError = { en: "Error", zh: "错误" },

    // Settings → Appearance: theme library.
    ThemeMenuLightHeading = { en: "Light themes", zh: "浅色主题" },
    ThemeMenuDarkHeading = { en: "Dark themes", zh: "深色主题" },
    ThemeLibraryTitle = { en: "Theme library", zh: "主题库" },
    ThemeLibrarySubtitle = { en: "Import or link custom themes.", zh: "导入或链接自定义主题。" },
    ThemeLibraryAdd = { en: "Add theme", zh: "添加主题" },
    ThemeLibraryImported = { en: "IMPORTED", zh: "已导入" },
    ThemeLibraryLinked = { en: "LINKED", zh: "已链接" },
    ThemeLibrarySelfContained = { en: "Self-contained snapshot", zh: "自包含快照" },
    ThemeLibraryStatus = {
        en: "{source} · {variants} · {detail}",
        zh: "{source} · {variants} · {detail}"
    },
    ThemeLibraryLastKnownGood = {
        en: "Using last known good · {message}",
        zh: "使用上次可用的版本 · {message}"
    },
    ThemeLibraryReload = { en: "Reload", zh: "重新加载" },
    ThemeLibraryReveal = { en: "Reveal", zh: "在文件夹中显示" },
    ThemeLibraryReview = { en: "Review", zh: "查看映射" },
    ThemeLibraryDuplicate = { en: "Duplicate as editable", zh: "复制为可编辑主题" },
    ThemeLibraryUnlink = { en: "Unlink", zh: "取消链接" },
    ThemeLibraryNotInitialized = {
        en: "custom theme library is not initialized",
        zh: "自定义主题库尚未初始化"
    },
    ThemeLibraryUnknown = {
        en: "unknown custom theme `{id}`",
        zh: "未知的自定义主题 `{id}`"
    },
    ThemeLibraryNoSource = {
        en: "theme source has no location to reveal",
        zh: "该主题的来源没有可在文件夹中显示的位置"
    },
    ThemeLibrarySaveFailed = {
        en: "could not save custom theme library",
        zh: "自定义主题库保存失败"
    },
    ThemeLibraryEntryLimit = {
        en: "custom theme library is limited to {limit} entries",
        zh: "自定义主题库最多 {limit} 个条目"
    },
    ThemeLibraryVariantLimit = {
        en: "custom theme library is limited to {limit} variants",
        zh: "自定义主题库最多 {limit} 个变体"
    },
    ThemeLibraryNoVariantSelected = {
        en: "select at least one successfully compiled variant",
        zh: "请至少选择一个编译成功的变体"
    },
    ThemeLibraryValidationFailed = {
        en: "theme validation failed: {errors}",
        zh: "主题校验失败：{errors}"
    },
    ThemeLibrarySnapshotCannotReload = {
        en: "imported snapshots cannot reload",
        zh: "导入的快照无法重新加载"
    },
    ThemeLibraryVariantOverflow = {
        en: "custom theme variant count overflow",
        zh: "自定义主题的变体数量溢出"
    },

    // Settings → Appearance: new-thread composer background.
    BackgroundTitle = { en: "New thread composer background", zh: "新会话输入框背景" },
    BackgroundHint = {
        en: "Add an image behind the composer on empty new threads.",
        zh: "在空白的新会话中为输入框添加背景图片。"
    },
    BackgroundSoftened = {
        en: "Softened automatically on frosted themes.",
        zh: "在磨砂主题上会自动柔化。"
    },
    BackgroundImageUnavailable = { en: "Image unavailable", zh: "图片不可用" },
    BackgroundChooseReplacement = {
        en: "Choose a replacement or remove it.",
        zh: "请选择替换图片或将其移除。"
    },
    BackgroundEffect = { en: "Background effect", zh: "背景效果" },
    BackgroundReplace = { en: "Replace image", zh: "替换图片" },
    BackgroundChoose = { en: "Choose image", zh: "选择图片" },
    BackgroundEffectNone = { en: "None", zh: "无" },
    BackgroundEffectDither = { en: "Dither", zh: "抖动" },
    BackgroundEffectAscii = { en: "ASCII", zh: "ASCII" },
    BackgroundEffectHalftone = { en: "Halftone", zh: "半调网点" },
    BackgroundEffectScanlines = { en: "Scanlines", zh: "扫描线" },
    BackgroundEffectNoneHint = { en: "Shows the original artwork.", zh: "显示原始图像。" },
    BackgroundEffectDitherHint = {
        en: "Rebuilds the artwork with a dithered color palette.",
        zh: "以抖动调色板重建图像。"
    },
    BackgroundEffectAsciiHint = {
        en: "Recreates the artwork with colored characters on black.",
        zh: "用彩色字符在黑色背景上重现图像。"
    },
    BackgroundEffectHalftoneHint = {
        en: "Recreates the artwork with colored print dots on black.",
        zh: "用彩色印刷网点在黑色背景上重现图像。"
    },
    BackgroundEffectScanlinesHint = {
        en: "Adds a pronounced horizontal display-line texture.",
        zh: "叠加明显的水平扫描线纹理。"
    },

    // Shared dialog actions.
    CommonBrowse = { en: "Browse…", zh: "浏览…" },
    CommonCancel = { en: "Cancel", zh: "取消" },
    CommonDone = { en: "Done", zh: "完成" },
    CommonRemove = { en: "Remove", zh: "移除" },
    CommonEdit = { en: "Edit", zh: "编辑" },
    // Shared by the shortcut recorder and the Appshots page.
    CommonPressEscapeToCancel = { en: "Press Escape to cancel.", zh: "按 Escape 键取消。" },

    // Sidebar.
    SidebarNoSessions = { en: "No sessions yet", zh: "还没有会话" },

    // Settings → Devices. `{time}` takes a [`relative_ago`] result.
    DevicesLastSeen = { en: "Last seen {time}", zh: "上次在线 {time}" },
    DevicesAdded = { en: "Added {time}", zh: "添加于 {time}" },
    // "Cursor SDK" is the product name; only the unknown-version stand-in moves.
    DevicesCursorSdk = { en: "Cursor SDK {version}", zh: "Cursor SDK {version}" },
    DevicesVersionUnknown = { en: "unknown (older engine)", zh: "未知（引擎版本较旧）" },

    // Composer. `{source}` takes a window title / app name; `{e}` and `{err}`
    // an engine payload; both stay untranslated.
    ComposerPlaceholder = { en: "Do anything…", zh: "做任何事…" },
    ComposerAttach = { en: "Attach", zh: "添加附件" },
    ComposerSendFailed = { en: "Send failed: {e}", zh: "发送失败：{e}" },
    ComposerSendFailedNoQueueId = {
        en: "Send failed: queue did not return an id",
        zh: "发送失败：队列未返回 id"
    },
    ComposerStopFailed = { en: "Stop failed: {err}", zh: "停止失败：{err}" },
    ComposerAnswerFailed = { en: "Answer failed: {err}", zh: "提交答复失败：{err}" },
    ComposerStageAttachmentFailed = {
        en: "Couldn't stage the attachment locally.",
        zh: "无法在本机暂存该附件。"
    },
    ComposerUploadAttachmentFailed = {
        en: "Couldn't upload the attachment — the device may be offline.",
        zh: "无法上传该附件 — 设备可能已离线。"
    },
    ComposerAppshotLimit = {
        en: "Remove an Appshot before adding another (96 MB staged Appshot limit).",
        zh: "请先移除一个 Appshot 再添加（暂存的 Appshot 上限为 96 MB）。"
    },
    ComposerAppshotPreview = { en: "Preview {source}", zh: "预览 {source}" },
    ComposerAppshotRemove = { en: "Remove {source}", zh: "移除 {source}" },
    ComposerFileMentionNoFiles = { en: "No files available", zh: "没有可用文件" },
    ComposerFileMentionNoMatches = { en: "No matching files", zh: "没有匹配的文件" },
    ComposerFileMentionOlderDevice = {
        en: "The session's device runs an older zeron — update it to search its files",
        zh: "该会话所在设备运行的 zeron 版本过旧 — 请更新后搜索其文件"
    },
    ComposerFileMentionFailed = { en: "File search failed", zh: "文件搜索失败" },
    ComposerSlashNoCommands = { en: "This agent has no slash commands", zh: "该智能体没有 slash 命令" },
    ComposerSlashNoMatches = { en: "No matching commands", zh: "没有匹配的命令" },
    ComposerSlashOlderDevice = {
        en: "The session's device runs an older zeron — update it to list commands",
        zh: "该会话所在设备运行的 zeron 版本过旧 — 请更新后列出命令"
    },
    ComposerSlashFailed = { en: "Couldn't load this agent's commands", zh: "无法加载该智能体的命令" },
    // Shared by the file-search and command popups.
    ComposerDeviceUnreachable = {
        en: "The session's device is unreachable",
        zh: "无法连接到该会话所在设备"
    },
    ComposerQueuedMessageRemoved = {
        en: "The queued message was removed; your edit remains in the composer",
        zh: "队列中的消息已被移除；你的修改仍保留在输入框中"
    },
    ComposerQueueUnsupported = {
        en: "Update the chat's engine to queue messages during a response.",
        zh: "请更新会话的引擎，以便在回复过程中排队消息。"
    },
    ComposerQuestionPlaceholderPick = {
        en: "Type your own answer, or pick an option above",
        zh: "输入你自己的答复，或在上方选择一个选项"
    },
    ComposerQuestionPlaceholderBlank = {
        en: "Type your own answer, or leave this blank to use the selected option",
        zh: "输入你自己的答复，或留空以使用所选选项"
    },
    ComposerQuestionMultiSelect = { en: "Select one or more options.", zh: "请选择一个或多个选项。" },
    ComposerWizardNext = { en: "Next", zh: "下一步" },
    ComposerWizardSubmit = { en: "Submit", zh: "提交" },
    ComposerSendWhenOnline = {
        en: "Offline — messages will send when you're back online.",
        zh: "离线 — 恢复联网后消息会自动发送。"
    },
    ComposerSendWhenReconnected = {
        en: "Messages will send once the connection recovers.",
        zh: "连接恢复后消息会自动发送。"
    },
    // Severity labels for the composer's failure notice. The severity is
    // recorded as a flag, never inferred from the message copy.
    ComposerNoticeWarning = { en: "Warning", zh: "警告" },
    ComposerNoticeError = { en: "Error", zh: "错误" },

    // Changes pane empty states. `{base}` takes the branch name, untranslated.
    DiffNoUncommittedChanges = { en: "No uncommitted changes", zh: "没有未提交的改动" },
    DiffNoChangesVs = { en: "No changes vs {base}", zh: "与 {base} 相比没有改动" },
    DiffNoBranchChanges = { en: "No branch changes", zh: "分支没有改动" },
    DiffNoChangesThisTurn = { en: "No changes this turn", zh: "本轮没有改动" },
    DiffNoCommitsFound = { en: "No commits found", zh: "没有找到提交" },
    DiffEmptyCommit = { en: "Empty commit", zh: "空提交" },

    // Relative timestamps. `{n}` takes the count, formatted per locale.
    RelativeNeverSeen = { en: "never seen", zh: "从未在线" },
    RelativeJustNow = { en: "just now", zh: "刚刚" },
    RelativeMinutesAgo = { en: "{n}m ago", zh: "{n} 分钟前" },
    RelativeHoursAgo = { en: "{n}h ago", zh: "{n} 小时前" },
    RelativeDaysAgo = { en: "{n}d ago", zh: "{n} 天前" },
    // The short form the sidebar, command palette and archived list show in a
    // narrow row slot (`zeron_proto::view::compact_age` supplies the bucket).
    RelativeCompactNow = { en: "now", zh: "刚刚" },
    RelativeCompactMinutes = { en: "{n}m", zh: "{n} 分钟" },
    RelativeCompactHours = { en: "{n}h", zh: "{n} 小时" },
    RelativeCompactDays = { en: "{n}d", zh: "{n} 天" },
    RelativeCompactWeeks = { en: "{n}w", zh: "{n} 周" },
    RelativeCompactMonths = { en: "{n}mo", zh: "{n} 个月" },
    RelativeCompactYears = { en: "{n}y", zh: "{n} 年" },

    // Shell → sidebar view menu and space filter (spaces.rs).
    SidebarViewOptions = { en: "Sidebar view options", zh: "侧边栏视图选项" },
    SidebarViewOrganize = { en: "Organize", zh: "组织" },
    SidebarViewSort = { en: "Sort", zh: "排序" },
    SidebarViewShow = { en: "Show", zh: "显示" },
    SidebarViewLayout = { en: "Layout", zh: "布局" },
    SidebarViewByDevice = { en: "By device", zh: "按设备" },
    SidebarViewByProject = { en: "By project", zh: "按项目" },
    SidebarViewInOneList = { en: "In one list", zh: "合并为一个列表" },
    SidebarViewLastUpdated = { en: "Last updated", zh: "最近更新" },
    SidebarViewCreated = { en: "Created", zh: "创建时间" },
    SidebarViewBranch = { en: "Branch", zh: "分支" },
    SidebarViewPullRequest = { en: "Pull request", zh: "拉取请求" },
    // The pull-request badge's hover card (change_requests.rs): the badge itself
    // draws only the number, so the state's label lives here.
    ChangeRequestTooltipTitle = {
        en: "PR {number} · {state}",
        zh: "PR {number} · {state}"
    },
    ChangeRequestStateOpen = { en: "Open", zh: "开放" },
    ChangeRequestStateMerged = { en: "Merged", zh: "已合并" },
    ChangeRequestStateClosed = { en: "Closed", zh: "已关闭" },
    // Zeron's term for a coding-agent runtime; kept as written.
    SidebarViewHarness = { en: "Harness", zh: "Harness" },
    SidebarViewProjectIcon = { en: "Project icon", zh: "项目图标" },
    SidebarViewLocation = { en: "Location", zh: "位置" },
    SidebarViewCompactMode = { en: "Compact mode", zh: "紧凑模式" },
    SpacesAllProjects = { en: "All projects", zh: "所有项目" },
    SpacesNewProject = { en: "New project…", zh: "新建项目…" },
    SpacesSearchProjects = { en: "Search projects…", zh: "搜索项目…" },
    SidebarArchived = { en: "Archived", zh: "已归档" },
    SidebarArchivedCount = { en: "Archived ({n})", zh: "已归档（{n}）" },
    SidebarPinned = { en: "Pinned", zh: "已固定" },
    SidebarPinnedCount = { en: "Pinned ({n})", zh: "已固定（{n}）" },
    SidebarSessions = { en: "Sessions", zh: "会话" },
    SidebarSessionsCount = { en: "Sessions ({n})", zh: "会话（{n}）" },
    SidebarGroupCount = { en: "{group} ({n})", zh: "{group}（{n}）" },
    SidebarShowMore = { en: "Show {n} more", zh: "再显示 {n} 个" },
    SidebarDropToUnpin = { en: "Drop here to unpin", zh: "拖到此处取消固定" },

    // Shell → sidebar pin writes (sidebar_pins.rs, spaces.rs, shell.rs).
    SidebarPinsStillSyncing = { en: "Pins are still syncing", zh: "固定项仍在同步" },
    SidebarPinsNotConfirmed = {
        en: "The engine did not confirm the saved pins",
        zh: "引擎未确认已保存的固定项"
    },
    SidebarPinsEngineOffline = {
        en: "Engine not connected. Pins were not changed.",
        zh: "引擎未连接，固定项没有改变。"
    },
    SidebarPinsAwaitingConfirmation = {
        en: "Waiting for the engine to confirm the previous pin change.",
        zh: "正在等待引擎确认上一次固定项更改。"
    },
    SidebarPinsSaveFailed = { en: "Couldn't save pins: {error}", zh: "无法保存固定项：{error}" },
    SidebarPinsUnconfirmed = {
        en: "Couldn't confirm pins. Queued edits were cancelled; waiting for the engine before allowing more pin changes.",
        zh: "无法确认固定项，已取消排队中的更改；在引擎确认之前不再接受新的固定项更改。"
    },
    SidebarPinsLimit = {
        en: "You can pin up to 200 sessions",
        zh: "最多可固定 200 个会话"
    },
    SidebarPinsInvalid = {
        en: "Sidebar pins must be non-empty and unique",
        zh: "侧边栏固定项必须非空且不重复"
    },

    // Shell → Add-space palette (spaces.rs).
    AddSpaceSearchDevices = { en: "Search devices…", zh: "搜索设备…" },
    AddSpaceSearchLocations = { en: "Search locations…", zh: "搜索位置…" },
    AddSpaceSearchFolders = { en: "Search folders…", zh: "搜索文件夹…" },
    AddSpaceHome = { en: "Home", zh: "主目录" },
    AddSpaceDeviceNotConnected = { en: "Device is not connected", zh: "设备未连接" },
    AddSpaceNoDevices = { en: "No devices found", zh: "未找到设备" },
    AddSpaceNoLocations = { en: "No locations found", zh: "未找到位置" },
    AddSpaceNoFoldersHere = { en: "No folders here", zh: "这里没有文件夹" },
    AddSpaceNoFoldersMatch = { en: "No folders match", zh: "没有匹配的文件夹" },
    AddSpaceLoadingLocations = { en: "Loading locations…", zh: "正在加载位置…" },
    AddSpaceAddProject = { en: "Add project", zh: "添加项目" },
    AddSpaceAdding = { en: "Adding…", zh: "正在添加…" },
    SpaceRenamePlaceholder = { en: "Project name", zh: "项目名称" },
    SpaceMenuRename = { en: "Rename…", zh: "重命名…" },
    SpaceMenuRemove = { en: "Remove…", zh: "移除…" },
    SpaceRenameTitle = { en: "Rename project", zh: "重命名项目" },
    SpaceRenameAction = { en: "Rename", zh: "重命名" },
    SpaceRemoveTitle = { en: "Remove project?", zh: "移除项目？" },
    SpaceRemoveUnknownProject = { en: "this project", zh: "该项目" },
    SpaceRemoveUnknownDevice = { en: "its device", zh: "其设备" },
    // `{name}` and `{device}` take data; `{count}` selects the row.
    SpaceRemoveConfirmOne = {
        en: "Removing “{name}” permanently deletes its 1 session on {device}. This can’t be undone.",
        zh: "移除“{name}”将永久删除其在 {device} 上的 1 个会话，且无法撤销。"
    },
    SpaceRemoveConfirmMany = {
        en: "Removing “{name}” permanently deletes its {count} sessions on {device}. This can’t be undone.",
        zh: "移除“{name}”将永久删除其在 {device} 上的 {count} 个会话，且无法撤销。"
    },

    // Command palette (command_palette.rs).
    CommandPalettePlaceholder = { en: "Search commands and chats…", zh: "搜索命令和会话…" },
    CommandNewChat = { en: "New chat", zh: "新建会话" },
    CommandOpenSettings = { en: "Open settings", zh: "打开设置" },
    CommandThemeSystem = { en: "Switch to system theme", zh: "切换到跟随系统主题" },
    CommandThemeLight = { en: "Switch to light theme", zh: "切换到浅色主题" },
    CommandThemeDark = { en: "Switch to dark theme", zh: "切换到深色主题" },
    CommandNoResults = { en: "No results", zh: "没有结果" },
    CommandNoResultsHint = {
        en: "Try a command, chat title, project, or device.",
        zh: "试试命令、会话标题、项目或设备。"
    },

    // Palette key hints (command_palette.rs, spaces.rs). Key caps stay as written.
    PaletteHintNavigate = { en: "Navigate", zh: "导航" },
    PaletteHintOpen = { en: "Open", zh: "打开" },
    PaletteHintSelect = { en: "Select", zh: "选择" },
    PaletteHintClose = { en: "Close", zh: "关闭" },

    // Settings → Archived (archived.rs).
    ArchivedTitle = { en: "Archived sessions", zh: "已归档会话" },
    ArchivedSubtitle = {
        en: "Hidden from the sidebar, never deleted. Unarchiving puts a session back on its device.",
        zh: "已从侧边栏隐藏，但不会被删除。取消归档会让会话回到它所在的设备。"
    },
    ArchivedEmpty = { en: "Nothing archived", zh: "没有已归档的会话" },
    ArchivedEmptyHint = {
        en: "Right-click a session in the sidebar to archive it.",
        zh: "在侧边栏右键点击会话即可将其归档。"
    },
    ArchivedUnarchiveFailed = { en: "Unarchive failed: {message}", zh: "取消归档失败：{message}" },
    ArchivedUnarchiving = { en: "Unarchiving…", zh: "正在取消归档…" },
    CommonUnarchive = { en: "Unarchive", zh: "取消归档" },

    // Shared actions and fallbacks. `Untitled` titles fill a missing chat title.
    CommonNewProject = { en: "New project", zh: "新建项目" },
    CommonRetry = { en: "Retry", zh: "重试" },
    CommonDevicesMenu = { en: "Devices", zh: "设备" },
    CommonClose = { en: "Close", zh: "关闭" },
    CommonRefresh = { en: "Refresh", zh: "刷新" },
    SessionUntitled = { en: "New session", zh: "新会话" },
    SessionUntitledArchived = { en: "Untitled session", zh: "未命名会话" },
    DeviceUnknown = { en: "Unknown device", zh: "未知设备" },

    // Shell → settings sidebar and shared actions (shell.rs).
    SettingsTitle = { en: "Settings", zh: "设置" },
    SettingsSectionDevices = { en: "Devices", zh: "设备" },
    SettingsSectionHarnesses = { en: "Agents", zh: "智能体" },
    SettingsSectionAgents = { en: "Accounts", zh: "账户" },
    SettingsSectionFiles = { en: "Files", zh: "文件" },
    SettingsSectionNotifications = { en: "Notifications", zh: "通知" },
    SettingsSectionShortcuts = { en: "Shortcuts", zh: "快捷键" },
    // Zeron's name for its screenshot surfaces; kept as written.
    SettingsSectionAppshots = { en: "Appshots", zh: "Appshots" },
    CommonBack = { en: "Back", zh: "返回" },
    CommonLater = { en: "Later", zh: "稍后" },
    CommonContinue = { en: "Continue", zh: "继续" },
    CommonDelete = { en: "Delete", zh: "删除" },
    CommonSignOut = { en: "Sign out", zh: "退出登录" },

    // Shell → session rows and connection pill (shell.rs).
    ChatStatusFailed = { en: "Failed", zh: "失败" },
    ChatStatusQueued = { en: "Queued", zh: "排队中" },
    ChatStatusWorking = { en: "Working", zh: "进行中" },
    ChatStatusInput = { en: "Input", zh: "等待输入" },
    ChatStatusDone = { en: "Done", zh: "完成" },
    // The word behind an idle row's status corner, which shows the elapsed
    // time instead; it reaches the accessibility tree and nothing else.
    ChatStatusIdle = { en: "Idle", zh: "空闲" },
    ChatActionArchive = { en: "Archive", zh: "归档" },
    ChatActionPin = { en: "Pin", zh: "固定" },
    ChatActionUnpin = { en: "Unpin", zh: "取消固定" },
    // A compact row's corner is its remote mark or its actions menu, so the
    // accessibility label names the slot rather than a status.
    ChatCornerRemoteSession = { en: "Remote session", zh: "远程会话" },
    ChatCornerSessionActions = { en: "Session actions", zh: "会话操作" },
    ConnectionOffline = { en: "Offline — sends are saved", zh: "离线 — 消息会先保存在本机" },
    ConnectionReconnecting = { en: "Reconnecting…", zh: "正在重新连接…" },

    // Shell → sidebar account/scope lines (shell.rs).
    SidebarSyncReadyAfterRestart = { en: "Sync ready after restart", zh: "重启后即可同步" },
    SidebarLocalOnly = { en: "Local only", zh: "仅本地" },
    SidebarStoredOnDevice = { en: "Stored on this device", zh: "保存在本机" },
    SidebarDevelopment = { en: "Development", zh: "开发模式" },
    SidebarLocalDevRuntime = { en: "Local development runtime", zh: "本地开发运行时" },
    // The build channel the synced client is running (a stage name, so it stays
    // "Alpha"; the Chinese row only marks it as a release stage).
    SidebarAlphaBuild = { en: "Alpha", zh: "Alpha 版" },
    SidebarAuthDisabled = { en: "Authentication disabled", zh: "已禁用身份认证" },
    SidebarNotSignedIn = { en: "Not signed in", zh: "未登录" },

    // Shell → project badge tooltip (project_icon.rs). A session with no space
    // has no project name, so the badge's hover card names the home directory.
    ProjectIconHome = { en: "Home", zh: "主目录" },

    // Shell → update strip. `{version}` takes the release, `{message}` a payload.
    UpdateAvailable = { en: "Update available — v{version}", zh: "有可用更新 — v{version}" },
    UpdateDownloading = { en: "Downloading v{version}…", zh: "正在下载 v{version}…" },
    UpdateReadyRestart = { en: "Update ready — restart to apply", zh: "更新已就绪 — 重启后生效" },
    UpdateFailed = { en: "Update failed: {message}", zh: "更新失败：{message}" },
    UpdateAvailableAdvisory = {
        en: "Update available — v{version} · run `zeron update`",
        zh: "有可用更新 — v{version} · 运行 `zeron update`"
    },

    // Shell → notices and runtime errors. Interpolated payloads stay as written.
    NoticeConversationLinkCopied = { en: "Zeron conversation link copied", zh: "已复制 Zeron 会话链接" },
    NoticeConversationLinkNotReady = { en: "Conversation link is not ready yet", zh: "会话链接尚未就绪" },
    NoticeCopied = { en: "{label} copied", zh: "已复制{label}" },
    NoticeHarnessSessionIdCopied = { en: "Harness session ID copied", zh: "已复制 Harness 会话 ID" },
    ErrorEngineNotConnected = { en: "Engine not connected", zh: "引擎未连接" },
    ErrorCancelSignInFailed = { en: "Could not cancel sign-in: {err}", zh: "无法取消登录：{err}" },
    ErrorSignInFailed = { en: "Sign in failed: {err}", zh: "登录失败：{err}" },
    ErrorSignOutFailed = { en: "Sign out failed: {err}", zh: "退出登录失败：{err}" },
    ErrorSyncedWorkspaceNotReady = {
        en: "The synced workspace did not come up — restart to finish.",
        zh: "同步工作区未能启动 — 请重启以完成。"
    },
    ErrorImportStreamEnded = { en: "The import stream ended before it finished.", zh: "导入流在完成前中断。" },
    ErrorStopRemoteEngine = {
        en: "Could not stop the remote engine: {err}. Run `zeron daemon stop`, then quit and reopen Zeron.",
        zh: "无法停止远程引擎：{err}。请运行 `zeron daemon stop`，然后退出并重新打开 Zeron。"
    },
    ErrorDaemonStopTimeout = {
        en: "The daemon did not finish stopping within {seconds} seconds.",
        zh: "守护进程未能在 {seconds} 秒内停止。"
    },
    ImportSummaryOneFailure = {
        en: "{imported} imported, 1 failure: {first}",
        zh: "已导入 {imported} 个，1 项失败：{first}"
    },
    ImportSummaryManyFailures = {
        en: "{imported} imported, {count} failures — first: {first}",
        zh: "已导入 {imported} 个，{count} 项失败 — 首个错误：{first}"
    },
    ImportSummaryUnknownError = { en: "unknown error", zh: "未知错误" },

    // Shell → switch wizard's description of local work. Counts pick a row;
    // `LocalWorkOnly` carries the English article that Chinese does not need.
    LocalWorkSessionsOne = { en: "1 session", zh: "1 个会话" },
    LocalWorkSessionsMany = { en: "{n} sessions", zh: "{n} 个会话" },
    LocalWorkProjectsOne = { en: "1 project", zh: "1 个项目" },
    LocalWorkProjectsMany = { en: "{n} projects", zh: "{n} 个项目" },
    LocalWorkOnly = { en: "the {phrase}", zh: "{phrase}" },
    LocalWorkBoth = { en: "the {sessions} and {projects}", zh: "{sessions}和{projects}" },

    // Shell → notifications (sound/banner pings).
    NotifyRunFinished = { en: "Run finished", zh: "运行完成" },
    NotifyWaitingInput = { en: "Waiting on your input", zh: "等待你的输入" },
    NotifyRunFailed = { en: "Run failed", zh: "运行失败" },
    NotifyConnectionUnavailable = { en: "Connection unavailable", zh: "连接不可用" },
    NotifyDeviceOffline = { en: "Your device is offline", zh: "你的设备已离线" },
    NotifyReconnecting = { en: "Zeron is trying to reconnect", zh: "Zeron 正在尝试重新连接" },

    // Shell → user menu (shell.rs).
    MenuEnableSync = { en: "Enable sync", zh: "启用同步" },
    MenuSyncInProgress = { en: "Sync setup in progress", zh: "正在设置同步" },
    MenuFinishSyncSetup = { en: "Finish sync setup", zh: "完成同步设置" },

    // Shell → sync lifecycle dialogs (shell.rs). `{email}`, `{phrase}`, `{n}`,
    // `{skipped}`, `{current}`, and `{total}` take data.
    SyncStoppingEngine = { en: "Stopping engine…", zh: "正在停止引擎…" },
    SyncStopDaemonAndQuit = { en: "Stop daemon and quit", zh: "停止守护进程并退出" },
    SyncEnableBody = {
        en: "Finish signing in in your browser. Zeron will keep using this local workspace until you quit and reopen.",
        zh: "请在浏览器中完成登录。Zeron 会继续使用这个本地工作区，直到你退出并重新打开。"
    },
    SyncOpenBrowserAgain = { en: "Open browser again", zh: "再次打开浏览器" },
    SyncCancelingTitle = { en: "Canceling sync setup…", zh: "正在取消同步设置…" },
    SyncCancelingBody = {
        en: "Removing the partial sign-in before returning to your local workspace.",
        zh: "正在清除未完成的登录，然后返回本地工作区。"
    },
    SyncSwitchOfferWithWork = {
        en: "You're signed in as {email}. Bring {phrase} from this device into your synced workspace, or start it fresh.",
        zh: "你已登录为 {email}。可以将这台设备上的{phrase}带入你的同步工作区，或从空白开始。"
    },
    SyncSwitchOfferSignedIn = {
        en: "You're signed in as {email}. Zeron can switch to your synced workspace now.",
        zh: "你已登录为 {email}。Zeron 现在可以切换到你的同步工作区。"
    },
    SyncSwitchOfferWithWorkNoEmail = {
        en: "Bring {phrase} from this device into your synced workspace, or start it fresh.",
        zh: "可以将这台设备上的{phrase}带入你的同步工作区，或从空白开始。"
    },
    SyncSwitchOfferPlain = {
        en: "Zeron can switch to your synced workspace now.",
        zh: "Zeron 现在可以切换到你的同步工作区。"
    },
    SyncStartFresh = { en: "Start fresh", zh: "从空白开始" },
    SyncBringMyWork = { en: "Bring my work", zh: "带入我的数据" },
    SyncSwitchNow = { en: "Switch now", zh: "立即切换" },
    SyncReadyTitle = { en: "Sync is ready", zh: "同步已就绪" },
    SyncSwitchingTitle = { en: "Switching to your synced workspace…", zh: "正在切换到你的同步工作区…" },
    SyncSwitchingBodyImport = {
        en: "Handing the engine over to your account. Your local sessions come along next.",
        zh: "正在将引擎交给你的账户，接下来会迁移本地会话。"
    },
    SyncSwitchingBody = { en: "Handing the engine over to your account.", zh: "正在将引擎交给你的账户。" },
    SyncImportLooking = { en: "Looking for local sessions…", zh: "正在查找本地会话…" },
    SyncImportProgress = {
        en: "Importing session {current} of {total}",
        zh: "正在导入第 {current} 个会话，共 {total} 个"
    },
    SyncImportTitle = { en: "Bringing your work over", zh: "正在迁移你的数据" },
    SyncImportDoneTitle = { en: "You're all set", zh: "全部完成" },
    SyncImportDoneReady = { en: "Your synced workspace is ready.", zh: "你的同步工作区已就绪。" },
    SyncImportDoneMovedOne = {
        en: "{n} session moved into your synced workspace.",
        zh: "{n} 个会话已迁入你的同步工作区。"
    },
    SyncImportDoneMovedMany = {
        en: "{n} sessions moved into your synced workspace.",
        zh: "{n} 个会话已迁入你的同步工作区。"
    },
    SyncImportDoneImportedOne = {
        en: "{n} session imported, {skipped} already present.",
        zh: "已导入 {n} 个会话，{skipped} 个已存在。"
    },
    SyncImportDoneImportedMany = {
        en: "{n} sessions imported, {skipped} already present.",
        zh: "已导入 {n} 个会话，{skipped} 个已存在。"
    },
    SyncImportFailedTitle = { en: "Import didn't finish", zh: "导入未完成" },
    SyncImportFailedBody = {
        en: "Anything already imported is kept; retrying only copies what's missing.",
        zh: "已导入的内容会保留；重试只会补齐缺失的部分。"
    },
    SyncRetryImport = { en: "Retry import", zh: "重试导入" },
    SyncRestartTitle = { en: "Sync needs a restart", zh: "同步需要重启" },
    SyncRestartBodyDaemon = {
        en: "Zeron is using a background daemon. Stop it and quit Zeron, then reopen to start the synced workspace. Existing local sessions stay on this device and will not be uploaded.",
        zh: "Zeron 正在使用后台守护进程。请停止它并退出 Zeron，然后重新打开以启动同步工作区。现有本地会话会保留在这台设备上，不会被上传。"
    },
    SyncRestartBody = {
        en: "Quit and reopen Zeron to start the synced workspace. Existing local sessions stay on this device and will not be uploaded.",
        zh: "请退出并重新打开 Zeron 以启动同步工作区。现有本地会话会保留在这台设备上，不会被上传。"
    },
    SyncSignOutTitle = { en: "Sign out?", zh: "退出登录？" },
    SyncSignOutBody = {
        en: "Zeron will remove your credentials, close the synced workspace, and continue in local mode.",
        zh: "Zeron 将移除你的凭据、关闭同步工作区，并继续以本地模式运行。"
    },
    SyncSigningOutTitle = { en: "Signing out…", zh: "正在退出登录…" },
    SyncSigningOutBody = {
        en: "Removing account credentials and closing the synced workspace.",
        zh: "正在移除账户凭据并关闭同步工作区。"
    },

    // Shell → signed-out restart card (shell.rs).
    SignedOutTitle = { en: "Signed out", zh: "已退出登录" },
    SignedOutBody = {
        en: "Zeron removed your credentials but could not finish closing the previous synced workspace. Retry before continuing in local mode.",
        zh: "Zeron 已移除你的凭据，但未能完成关闭之前的同步工作区。请先重试，再继续以本地模式运行。"
    },
    SignedOutRetryLocalMode = { en: "Retry local mode", zh: "重试本地模式" },

    // Shell → sign-in gate (shell.rs).
    GateSignInTitle = { en: "Log in to Zeron", zh: "登录 Zeron" },
    GateSignInBody = {
        en: "This opens your browser to finish logging in — you'll come right back.",
        zh: "这会在浏览器中打开登录页面 — 完成后会回到这里。"
    },
    GateSignInButton = { en: "Log in", zh: "登录" },

    // Shell → workspace creation gate (shell.rs). `{email}` takes the account.
    OrgGateTitle = { en: "Create your workspace", zh: "创建工作区" },
    OrgGateBodyWithEmail = {
        en: "Zeron is organized around workspaces — create one for yourself or your team. Signed in as {email}.",
        zh: "Zeron 以工作区为核心 — 为自己或团队创建一个。当前登录：{email}。"
    },
    OrgGateBody = {
        en: "Zeron is organized around workspaces — create one for yourself or your team.",
        zh: "Zeron 以工作区为核心 — 为自己或团队创建一个。"
    },
    OrgGateNamePlaceholder = { en: "Workspace name", zh: "工作区名称" },
    OrgGateMemberships = { en: "Or continue in a workspace you belong to", zh: "或继续使用你已加入的工作区" },
    OrgGateCreate = { en: "Create", zh: "创建" },
    OrgGateCreating = { en: "Creating…", zh: "正在创建…" },
    OrgGateCancelSyncSetup = { en: "Cancel sync setup", zh: "取消同步设置" },
    OrgGateUseDifferentAccount = { en: "Use a different account", zh: "使用其他账户" },
    OrgGateNameRequired = { en: "Enter a workspace name", zh: "请输入工作区名称" },

    // Shell → session context menu and dialogs (shell.rs). `{title}` takes data.
    ChatMenuRename = { en: "Rename…", zh: "重命名…" },
    ChatMenuDelete = { en: "Delete…", zh: "删除…" },
    ChatMenuZeronLink = { en: "Zeron conversation link", zh: "Zeron 会话链接" },
    ChatMenuHarnessSessionId = { en: "Harness session ID", zh: "Harness 会话 ID" },
    RenameSessionTitle = { en: "Rename session", zh: "重命名会话" },
    RenameSessionPlaceholder = { en: "Session title", zh: "会话标题" },
    DeleteSessionTitle = { en: "Delete session?", zh: "删除会话？" },
    DeleteSessionBody = {
        en: "“{title}” will be permanently deleted. This can’t be undone.",
        zh: "“{title}”将被永久删除，且无法撤销。"
    },

    // Shell → composer/transcript affordances (shell.rs).
    ComposerDropToAttach = { en: "Drop to attach", zh: "拖放到此处以添加附件" },
    TranscriptScrollToBottom = { en: "Scroll to bottom", zh: "滚动到底部" },
    ComposerSending = { en: "Sending…", zh: "正在发送…" },

    // Shell → onboarding canvas (shell.rs).
    OnboardingAddProject = { en: "Add a project to get started", zh: "添加一个项目以开始使用" },
    OnboardingProjectHint = {
        en: "A project is a folder on one of your devices.",
        zh: "项目是你某个设备上的一个文件夹。"
    },
    OnboardingAddProjectAction = { en: "Add a project", zh: "添加项目" },

    // Shell → right-pane surfaces (shell.rs). `{name}` takes a tab title.
    SurfaceFiles = { en: "Files", zh: "文件" },
    SurfaceBrowser = { en: "Browser", zh: "浏览器" },
    SurfaceTerminal = { en: "Terminal", zh: "终端" },
    SurfaceDiffs = { en: "Diffs", zh: "差异" },
    SurfaceHistory = { en: "History", zh: "历史" },
    SurfaceFileFallback = { en: "File", zh: "文件" },
    TabUnsavedChanges = { en: "{name}, unsaved changes", zh: "{name}，有未保存的改动" },

    // Shell → composer pickers (pickers.rs). `{shown}`, `{total}`, and `{name}`
    // take data.
    PickerSearch = { en: "Search…", zh: "搜索…" },
    PickerSearchRefs = { en: "Search refs…", zh: "搜索分支…" },
    PickerSearchModels = { en: "Search models…", zh: "搜索模型…" },
    PickerNoDevicesMatch = { en: "No devices match.", zh: "没有匹配的设备。" },
    PickerNoProjectsOnDevice = {
        en: "No projects on this device.",
        zh: "该设备上没有项目。"
    },
    PickerNoProjectsMatch = { en: "No projects match.", zh: "没有匹配的项目。" },
    PickerNoProjectOptOut = { en: "Don't work in a project", zh: "不使用项目" },
    PickerNoProject = { en: "No project", zh: "无项目" },
    PickerNoProjectSelected = { en: "No project selected", zh: "未选择项目" },
    PickerThisDevice = { en: "This device", zh: "本机" },
    PickerDeviceYou = { en: "You", zh: "你" },
    PickerNoRef = { en: "No ref", zh: "无分支" },
    PickerNoRefsFound = { en: "No refs found.", zh: "没有找到分支。" },
    PickerRefTagCurrent = { en: "current", zh: "当前" },
    PickerRefTagWorktree = { en: "worktree", zh: "工作树" },
    PickerRefSwitching = { en: "switching…", zh: "正在切换…" },
    PickerRefsShownOfTotal = {
        en: "Showing {shown} of {total} refs",
        zh: "显示 {shown}/{total} 个分支"
    },
    PickerSelectRef = { en: "Select ref", zh: "选择分支" },
    // `{name}` takes the picked ref.
    PickerRefFrom = { en: "From {name}", zh: "基于 {name}" },
    PickerCheckoutNewWorktree = { en: "New worktree", zh: "新建工作树" },
    PickerCheckoutCurrentWorktree = { en: "Current worktree", zh: "当前工作树" },
    PickerCheckoutCurrentCheckout = { en: "Current checkout", zh: "当前检出" },
    PickerCheckoutWorktree = { en: "Worktree", zh: "工作树" },
    PickerCheckoutLocal = { en: "Local checkout", zh: "本地检出" },
    PickerNoAgents = { en: "No agents available", zh: "没有可用的智能体" },
    PickerNoAgentsHint = {
        en: "Enable an installed agent in Settings → Agents, or install an agent CLI.",
        zh: "请在 设置 → 智能体 中启用已安装的智能体，或安装智能体 CLI。"
    },
    PickerNoModels = { en: "No models found", zh: "没有找到模型" },
    PickerNoStarredModels = {
        en: "No starred models yet — hit a row's star",
        zh: "还没有收藏的模型 — 点击某一行上的星标"
    },
    PickerReasoning = { en: "Reasoning", zh: "推理" },
    PickerContextWindow = { en: "Context Window", zh: "上下文窗口" },
    PickerDefaultBadge = { en: "Default", zh: "默认" },
    // Reasoning ladder labels. `Ultra`, `Ultracode`, and `Ultrathink` are the
    // product's own tier names, so they read the same in both locales.
    PickerReasoningMinimal = { en: "Minimal", zh: "最少" },
    PickerReasoningLow = { en: "Low", zh: "低" },
    PickerReasoningMedium = { en: "Medium", zh: "中" },
    PickerReasoningHigh = { en: "High", zh: "高" },
    PickerReasoningXHigh = { en: "X-High", zh: "超高" },
    PickerReasoningMax = { en: "Max", zh: "最高" },
    PickerReasoningUltra = { en: "Ultra", zh: "Ultra" },
    PickerReasoningUltracode = { en: "Ultracode", zh: "Ultracode" },
    PickerReasoningUltrathink = { en: "Ultrathink", zh: "Ultrathink" },

    // Queue (queue.rs). `{owner}` takes a device id, `{name}` an attachment
    // name, `{n}` and `{labels}` the attachment count and its name list.
    QueueSendNow = { en: "Send now", zh: "立即发送" },
    QueueSendNowInterrupt = { en: "Send now (interrupt)", zh: "立即发送（中断当前回复）" },
    QueueWaitingForProvider = {
        en: "Waiting for provider capabilities",
        zh: "等待提供方能力就绪"
    },
    QueueSaveToQueue = { en: "Save to queue", zh: "保存到队列" },
    QueueRemoving = { en: "Removing…", zh: "正在移除…" },
    QueueSaving = { en: "Saving…", zh: "正在保存…" },
    QueueEditingInComposer = { en: "Editing in composer", zh: "正在输入框中编辑" },
    QueueEditingOnDevice = { en: "Editing on {owner}", zh: "正在 {owner} 上编辑" },
    QueueNeedsReview = { en: "Needs review", zh: "待确认" },
    QueueAttachmentSummary = { en: "{n} attachments · {labels}", zh: "{n} 个附件 · {labels}" },
    QueueMoreAttachmentsHint = {
        en: "{n} more attachments; edit message to view all",
        zh: "还有 {n} 个附件；编辑消息可查看全部"
    },
    // Zeron's name for its screenshot surfaces; kept as written.
    QueueAppshotLabel = { en: "{name} Appshot", zh: "{name} Appshot" },
    QueueImageName = { en: "Image", zh: "图片" },
    QueuePreviewImage = { en: "Open attachment preview", zh: "打开附件预览" },
    QueueImageLoadFailed = {
        en: "Could not load the image. Try opening it again.",
        zh: "无法加载该图片。请重新打开。"
    },
    QueueReorderFailed = { en: "Couldn't reorder the queue", zh: "无法重新排序队列" },
    QueueSendFailed = { en: "Couldn't send that message", zh: "无法发送该消息" },
    QueueHostNoSafeRemoval = {
        en: "The chat host does not support safe queue removal",
        zh: "会话主机不支持安全地移除队列消息"
    },
    QueueHostNoActions = {
        en: "The chat host does not support queue actions",
        zh: "会话主机不支持队列操作"
    },
    QueueHostNeedsUpdate = {
        en: "Update the chat host to edit queued messages safely",
        zh: "请更新会话主机，以便安全地编辑排队消息"
    },
    QueueAttachmentLoadFailed = {
        en: "Couldn't load the queued attachments or Appshot context. Check the connection and update the chat host.",
        zh: "无法加载排队的附件或 Appshot 上下文。请检查连接并更新会话主机。"
    },
    QueueInvalidLease = {
        en: "The chat host returned an invalid edit lease",
        zh: "会话主机返回了无效的编辑租约"
    },
    QueueEditedElsewhere = {
        en: "That queued message is being edited on another device",
        zh: "该排队消息正在另一台设备上编辑"
    },
    QueueMessageGone = {
        en: "That queued message is no longer available",
        zh: "该排队消息已不存在"
    },
    QueueConnectToEdit = {
        en: "Connect to the chat host to edit this message",
        zh: "请连接到会话主机以编辑该消息"
    },
    QueueLeaseLost = {
        en: "The edit lease was lost; your text is still in the editor",
        zh: "编辑租约已丢失；你的文字仍保留在编辑器中"
    },
    QueueConflictKeptLocally = {
        en: "This message changed on another device; your edit was kept locally",
        zh: "该消息在另一台设备上发生了改动；你的编辑已保留在本机"
    },
    QueueRemovedKeptLocally = {
        en: "The queued message was removed; your edit was kept locally",
        zh: "排队消息已被移除；你的编辑已保留在本机"
    },
    QueueLeaseChangedKept = {
        en: "The edit lease changed; your text is still in the editor",
        zh: "编辑租约已变更；你的文字仍保留在编辑器中"
    },
    QueueUnreachableKept = {
        en: "Couldn't reach the chat host; your edit is still in the editor",
        zh: "无法连接到会话主机；你的编辑仍保留在编辑器中"
    },
    QueueEditProtectionExpired = {
        en: "Edit protection expired; review this message before sending",
        zh: "编辑保护已过期；发送前请检查该消息"
    },
    QueueAlreadyLeft = {
        en: "That message had already left the queue",
        zh: "该消息已离开队列"
    },
    QueueRemoveFailed = { en: "Couldn't remove the message", zh: "无法移除该消息" },

    // Attachments (attachments.rs). `{name}` takes the file name.
    AttachmentLoadingImage = { en: "Loading image…", zh: "正在加载图片…" },
    AttachmentUnsupportedImage = {
        en: "{name} is not a supported image.",
        zh: "{name} 不是支持的图片格式。"
    },
    AttachmentUnreadable = { en: "{name} could not be read.", zh: "无法读取 {name}。" },
    AttachmentTooLarge = {
        en: "{name} is too large (24 MB max).",
        zh: "{name} 过大（上限 24 MB）。"
    },

    // Settings → Background image (settings.rs).
    BackgroundImageUnsupported = {
        en: "This background image is unsupported or damaged. Choose a valid image such as PNG or JPEG.",
        zh: "该背景图片不受支持或已损坏，请选择 PNG 或 JPEG 等有效图片。"
    },
    BackgroundSaveFailedRestart = {
        en: "Unable to save the image. Restart Zeron and try again.",
        zh: "无法保存图片，请重启 Zeron 后重试。"
    },
    BackgroundSaveFailedPermissions = {
        en: "Unable to save the image. Check folder permissions and try again.",
        zh: "无法保存图片，请检查文件夹权限后重试。"
    },
    BackgroundRemoveFailedRestart = {
        en: "Unable to remove the image. Restart Zeron and try again.",
        zh: "无法移除图片，请重启 Zeron 后重试。"
    },
    BackgroundRemoveFailedPermissions = {
        en: "Unable to remove the image. Check folder permissions and try again.",
        zh: "无法移除图片，请检查文件夹权限后重试。"
    },

    // Transcript (transcript.rs). `{n}`, `{base}`, `{what}`, `{size}`, `{name}`,
    // `{source}`, `{title}`, and `{percent}` take data and are never translated.

    // Transcript: user-message expander.
    TranscriptShowMore = { en: "Show more", zh: "显示更多" },
    TranscriptShowLess = { en: "Show less", zh: "收起" },
    TranscriptExpandMessage = { en: "Expand message", zh: "展开消息" },
    TranscriptCollapseMessage = { en: "Collapse message", zh: "收起消息" },

    // Transcript: working trailer under the live reply.
    TranscriptNotDeliveredRetry = { en: "Not delivered — click to retry", zh: "未送达 — 点击重试" },
    TranscriptQueuedWillSend = {
        en: "Queued — will send automatically",
        zh: "已排队 — 将自动发送"
    },

    // Transcript: working-indicator flavour words, one per rotation slot. The
    // English list rotates as whimsical gerunds; Chinese names the same idea.
    TranscriptFlavourZeroning = { en: "Zeroning", zh: "Zeron 中" },
    TranscriptFlavourThinking = { en: "Thinking", zh: "思考中" },
    TranscriptFlavourPondering = { en: "Pondering", zh: "琢磨中" },
    TranscriptFlavourScheming = { en: "Scheming", zh: "盘算中" },
    TranscriptFlavourBrewing = { en: "Brewing", zh: "酝酿中" },
    TranscriptFlavourWeaving = { en: "Weaving", zh: "编织中" },
    TranscriptFlavourTinkering = { en: "Tinkering", zh: "摆弄中" },
    TranscriptFlavourMusing = { en: "Musing", zh: "沉思中" },
    TranscriptFlavourComposing = { en: "Composing", zh: "构思中" },
    TranscriptFlavourSifting = { en: "Sifting", zh: "筛选中" },
    TranscriptFlavourUntangling = { en: "Untangling", zh: "梳理中" },
    TranscriptFlavourDistilling = { en: "Distilling", zh: "提炼中" },
    TranscriptFlavourSketching = { en: "Sketching", zh: "起草中" },
    TranscriptFlavourPlotting = { en: "Plotting", zh: "谋划中" },
    TranscriptFlavourRiffing = { en: "Riffing", zh: "即兴发挥中" },
    TranscriptFlavourCombobulating = { en: "Combobulating", zh: "重整思路中" },
    TranscriptFlavourPercolating = { en: "Percolating", zh: "慢慢渗透中" },
    TranscriptFlavourMarinating = { en: "Marinating", zh: "入味中" },
    TranscriptFlavourNoodling = { en: "Noodling", zh: "随手涂画中" },
    TranscriptFlavourPuzzling = { en: "Puzzling", zh: "冥思苦想中" },
    TranscriptFlavourConjuring = { en: "Conjuring", zh: "施展中" },

    // Transcript: tool-group summary, chips, and blob affordances. `{what}` is
    // the detail noun from `TranscriptDetailDiff` / `TranscriptDetailOutput`.
    TranscriptThoughtProcess = { en: "Thought process", zh: "思考过程" },
    TranscriptThoughtTimes = { en: "Thought {n} times", zh: "思考 {n} 次" },
    TranscriptThoughtWithBase = { en: "Thought · {base}", zh: "思考 · {base}" },
    TranscriptThoughtTimesWithBase = {
        en: "Thought {n} times · {base}",
        zh: "思考 {n} 次 · {base}"
    },
    TranscriptShowFull = { en: "Show full {what}", zh: "查看完整{what}" },
    TranscriptShowFullSized = { en: "Show full {what} ({size})", zh: "查看完整{what}（{size}）" },
    TranscriptLoadingFull = { en: "Loading full {what}…", zh: "正在加载完整{what}…" },
    TranscriptLoadFullFailed = {
        en: "Couldn't load full {what} — tap to retry",
        zh: "无法加载完整{what} — 点击重试"
    },
    TranscriptDetailDiff = { en: "diff", zh: "差异" },
    TranscriptDetailOutput = { en: "output", zh: "输出" },
    TranscriptMoreLines = { en: "… {n} more lines", zh: "…还有 {n} 行" },
    TranscriptQuestion = { en: "Question", zh: "问题" },
    TranscriptAwaitingAnswer = { en: "Awaiting your answer…", zh: "等待你的回复…" },
    TranscriptSubagent = { en: "Subagent", zh: "子智能体" },

    // Transcript: generated images and Appshot thumbnails. `Appshot` is Zeron's
    // name for its screenshot surfaces; kept as written.
    TranscriptPreviewGeneratedImage = { en: "Preview generated image", zh: "预览生成的图片" },
    TranscriptLoadingGeneratedImage = { en: "Loading generated image…", zh: "正在加载生成的图片…" },
    TranscriptGeneratedImageUnavailable = {
        en: "Generated image unavailable",
        zh: "生成的图片不可用"
    },
    TranscriptPreviewAppshot = {
        en: "Preview {source} Appshot: {title}",
        zh: "预览 {source} Appshot：{title}"
    },
    TranscriptAppshotName = { en: "{name} · Appshot", zh: "{name} · Appshot" },
    TranscriptAppshotUploading = { en: "Uploading Appshot…", zh: "正在上传 Appshot…" },
    TranscriptAppshotLoading = { en: "Loading Appshot…", zh: "正在加载 Appshot…" },
    TranscriptAppshotUnavailable = { en: "Appshot unavailable", zh: "Appshot 不可用" },
    TranscriptUploadingPercent = { en: "Uploading {percent}%", zh: "正在上传 {percent}%" },
    TranscriptUploading = { en: "Uploading…", zh: "正在上传…" },

    // Changes → diff scope. `DiffScope::label_message` returns one of these;
    // the History scope reuses the surface tab's own label.
    DiffScopeWorkingTree = { en: "Working tree", zh: "工作树" },
    DiffScopeBranchChanges = { en: "Branch changes", zh: "分支改动" },
    DiffScopeLatestTurn = { en: "Latest turn", zh: "最近一轮" },
    // A git commit, named the same by the diff scope and the History column.
    CommonCommit = { en: "Commit", zh: "提交" },

    // Changes → header counts. `{files}` takes a [`count_files`] result, `{n}`
    // a raw count, and `{base}` the comparison ref — data either way.
    DiffUncommittedOne = { en: "{n} Uncommitted change", zh: "{n} 个未提交的改动" },
    DiffUncommittedMany = { en: "{n} Uncommitted changes", zh: "{n} 个未提交的改动" },
    DiffChangedFiles = { en: "{files} changed", zh: "{files}有改动" },
    DiffChangedFilesVs = { en: "{files} changed vs {base}", zh: "{files}有改动，对比 {base}" },
    DiffChangedFilesThisTurn = { en: "{files} changed this turn", zh: "本轮 {files}有改动" },
    DiffChangedFilesInCommit = { en: "{files} changed in this commit", zh: "该提交中 {files}有改动" },

    // Changes → file-level notice rows rendered in the diff body. `{from}`,
    // `{mode}`, `{max_lines}`, and `{total}` take data.
    DiffNoticeNewFile = { en: "New file", zh: "新文件" },
    DiffNoticeDeletedFile = { en: "Deleted file", zh: "已删除的文件" },
    DiffNoticeBinary = { en: "Binary file — contents not shown", zh: "二进制文件 — 不显示内容" },
    DiffNoticeRenamed = { en: "Renamed from {from}", zh: "重命名自 {from}" },
    DiffNoticeModeChanged = { en: "Mode changed to {mode}", zh: "权限模式变更为 {mode}" },
    DiffNoticeTruncated = {
        en: "Diff truncated — showing first {max_lines} of {total} lines",
        zh: "差异已截断 — 显示前 {max_lines} 行，共 {total} 行"
    },
    DiffFileBinaryBadge = { en: "BIN", zh: "二进制" },
    DiffPartialSnapshot = { en: "Partial snapshot", zh: "部分快照" },

    // Changes → states, errors, and pickers. `{err}` takes an engine payload.
    DiffPreparing = { en: "Preparing diff…", zh: "正在准备差异…" },
    DiffWatchInterrupted = { en: "Diff stream interrupted — retrying", zh: "差异流中断 — 正在重试" },
    DiffWatchUnavailable = { en: "Diff watch unavailable: {err}", zh: "差异监听不可用：{err}" },
    DiffNoTurnRecorded = {
        en: "No turn recorded yet — send a message first",
        zh: "尚未记录任何轮次 — 请先发送一条消息"
    },
    DiffDeviceOutdated = {
        en: "This chat's device is running an older Zeron — update it to view branch and turn diffs",
        zh: "该会话所在设备运行的 Zeron 版本过旧 — 请更新后查看分支和本轮差异"
    },
    DiffNoBranches = { en: "No branches", zh: "没有分支" },
    DiffNoMatchingBranches = { en: "No matching branches", zh: "没有匹配的分支" },
    DiffSearchBranches = { en: "Search branches…", zh: "搜索分支…" },
    DiffWrapLongLines = { en: "Wrap long lines", zh: "自动换行" },
    DiffRequestChange = { en: "Request a change…", zh: "请求改动…" },

    // History pane (history.rs). `{kind}`, `{name}`, `{base}`, `{ahead}`,
    // `{behind}`, `{n}`, and `{error}` take data.
    HistoryColumnAuthor = { en: "Author", zh: "作者" },
    HistoryColumnDate = { en: "Date", zh: "日期" },
    HistoryColumnSha = { en: "SHA", zh: "SHA" },
    HistoryRefBranch = { en: "Branch", zh: "分支" },
    HistoryRefRemoteBranch = { en: "Remote branch", zh: "远程分支" },
    HistoryRefTag = { en: "Tag", zh: "标签" },
    HistoryRefDescription = { en: "{kind}: {name}", zh: "{kind}：{name}" },
    HistoryAuthorUnknown = { en: "Unknown", zh: "未知" },
    HistoryCommitNoSubject = { en: "(no subject)", zh: "（无提交信息）" },
    HistoryAuthorDisplayName = { en: "Name", zh: "姓名" },
    CommonCopied = { en: "Copied", zh: "已复制" },
    HistorySearch = { en: "Search", zh: "搜索" },
    HistorySearchCommits = { en: "Search commits", zh: "搜索提交" },
    HistoryFetchAll = { en: "Fetch all", zh: "全部拉取" },
    HistoryFetching = { en: "Fetching…", zh: "正在拉取…" },
    HistoryFetchFailed = { en: "Fetch failed: {error}", zh: "拉取失败：{error}" },
    HistoryShowAllCommits = { en: "Show all commits", zh: "显示所有提交" },
    HistoryShowBranchTips = { en: "Show branch tips", zh: "显示分支末端" },
    HistoryAhead = { en: "{n} ahead", zh: "领先 {n}" },
    HistoryBehind = { en: "{n} behind", zh: "落后 {n}" },
    HistoryComparedWith = {
        en: "Compared with {base}: {ahead} ahead, {behind} behind",
        zh: "与 {base} 相比：领先 {ahead}，落后 {behind}"
    },
    HistoryExpandRef = { en: "Expand {name}", zh: "展开 {name}" },
    HistoryExpandRefHidden = {
        en: "Expand {name} ({n} hidden)",
        zh: "展开 {name}（隐藏 {n} 个）"
    },
    HistoryCollapseRef = { en: "Collapse {name}", zh: "折叠 {name}" },
    HistoryLoadMore = { en: "Load more", zh: "加载更多" },
    CommonLoading = { en: "Loading…", zh: "正在加载…" },
    HistoryLoadingHistory = { en: "Loading history…", zh: "正在加载历史…" },
    HistoryNoRepository = { en: "No repository selected", zh: "未选择仓库" },
    HistoryNoMatchingCommits = { en: "No matching commits", zh: "没有匹配的提交" },
    HistoryNoBranchTips = { en: "No branch tips found", zh: "没有找到分支末端" },

    // Terminal panel (terminal/panel.rs). `{n}` takes the tab number.
    TerminalTabTitle = { en: "Terminal {n}", zh: "终端 {n}" },
    TerminalSelectChat = { en: "Select a chat to open a terminal", zh: "选择一个会话以打开终端" },

    // Files browser, tree, and search (files/mod.rs, files/tree.rs,
    // files/search.rs, files/watch.rs). `{message}` takes a raw client
    // payload and `{n}` a count, so both stay as written.
    FilesSurfaceLabel = { en: "Workspace files", zh: "工作区文件" },
    FilesNoWorkspace = {
        en: "No workspace available for this chat.",
        zh: "该会话没有可用的工作区。"
    },
    FilesServiceStarting = {
        en: "Workspace service is still starting.",
        zh: "工作区服务仍在启动中。"
    },
    FilesServiceUnavailable = {
        en: "Workspace service is unavailable.",
        zh: "工作区服务不可用。"
    },
    FilesRefreshNow = { en: "Refresh now", zh: "立即刷新" },
    FilesRefreshNowLabel = { en: "Refresh workspace files now", zh: "立即刷新工作区文件" },
    FilesSearchPlaceholder = { en: "Search files", zh: "搜索文件" },
    FilesShowAllFiles = { en: "Show all files (even hidden)", zh: "显示所有文件（包括隐藏文件）" },
    FilesHideIgnoredFiles = { en: "Hide hidden and ignored files", zh: "隐藏隐藏和忽略的文件" },
    FilesTreeLabel = { en: "Workspace file tree", zh: "工作区文件树" },
    FilesTreeEmptyFolder = { en: "Empty folder", zh: "空文件夹" },
    FilesTreeLoadMore = { en: "Load more…", zh: "加载更多…" },
    FilesTreeErrorRetry = { en: "{message} — Retry", zh: "{message} — 重试" },
    FilesSearchResultsLabel = {
        en: "Fuzzy workspace file results",
        zh: "工作区文件模糊搜索结果"
    },
    FilesSearching = { en: "Searching…", zh: "正在搜索…" },
    FilesNoFilesFound = { en: "No files found.", zh: "没有找到文件。" },
    FilesSearchLimit = { en: "Showing the first {n} matches", zh: "只显示前 {n} 个匹配项" },
    FilesWatchDecodeFailed = {
        en: "File updates could not be decoded: {error}",
        zh: "无法解析文件更新：{error}"
    },
    FilesWatchInterrupted = { en: "File updates interrupted — retrying", zh: "文件更新中断 — 正在重试" },

    // Files preview, editor chrome, and comments (files/preview.rs). `{line}`
    // takes a line number.
    FilesAddCommentPlaceholder = { en: "Add a comment…", zh: "添加评论…" },
    FilesCommentOnLine = { en: "Comment on line {line}", zh: "在第 {line} 行添加评论" },
    FilesCommentOpenLine = { en: "Open comment on line {line}", zh: "打开第 {line} 行的评论" },
    FilesCommentSave = { en: "Save", zh: "保存" },
    FilesCommentCommit = { en: "Comment", zh: "评论" },
    FilesSaveFailed = { en: "Save failed", zh: "保存失败" },
    FilesSaveConflict = { en: "Save conflict", zh: "保存冲突" },
    FilesSaveConflictDetail = {
        en: "The file changed on disk. Your editor buffer was preserved.",
        zh: "文件在磁盘上发生了变化。编辑器中的内容已保留。"
    },
    FilesDeletedOnDisk = { en: "Deleted on disk", zh: "磁盘上已删除" },
    FilesDeletedOnDiskDetail = {
        en: "The file was removed on disk. Your editor buffer was preserved.",
        zh: "文件已从磁盘移除。编辑器中的内容已保留。"
    },
    FilesChangedOnDisk = { en: "Changed on disk", zh: "磁盘上已更改" },
    FilesChangedOnDiskDetail = {
        en: "The file changed on disk. Review it before saving.",
        zh: "文件在磁盘上发生了变化，保存前请先检查。"
    },
    FilesWorkspaceSwitchedSave = {
        en: "Workspace changed. Switch back to save, or discard these edits.",
        zh: "工作区已切换。请切回以保存，或放弃这些改动。"
    },
    FilesChangesNotSaved = { en: "Changes could not be saved safely.", zh: "无法安全保存改动。" },
    FilesSavingBeforeClose = { en: "Saving changes before closing…", zh: "正在关闭前保存改动…" },
    FilesKeepOpen = { en: "Keep Open", zh: "保持打开" },
    FilesDiscardChanges = { en: "Discard Changes", zh: "放弃改动" },
    FilesDiscardUnsavedChanges = { en: "Discard unsaved changes?", zh: "放弃未保存的改动？" },
    FilesChangedOutside = {
        en: "This file changed outside Zeron.",
        zh: "该文件已在 Zeron 之外发生变化。"
    },
    FilesKeepEditing = { en: "Keep Editing", zh: "继续编辑" },
    FilesReloadFromDisk = { en: "Reload from Disk", zh: "从磁盘重新加载" },
    FilesDiscardAndReload = { en: "Discard & Reload", zh: "放弃并重新加载" },
    FilesHideSidebar = { en: "Hide files sidebar", zh: "隐藏文件侧边栏" },
    FilesShowSidebar = { en: "Show files sidebar", zh: "显示文件侧边栏" },
    FilesSaveFile = { en: "Save file", zh: "保存文件" },
    FilesRevealInTree = { en: "Reveal file in tree", zh: "在文件树中显示" },
    FilesDisableWordWrap = { en: "Disable word wrap", zh: "关闭自动换行" },
    FilesEnableWordWrap = { en: "Enable word wrap", zh: "开启自动换行" },
    FilesShowMarkdownCode = { en: "Show Markdown code", zh: "显示 Markdown 源码" },
    FilesPreviewMarkdown = { en: "Preview Markdown", zh: "预览 Markdown" },
    FilesImagePreviewSuspended = {
        en: "Workspace changed. Image preview suspended.",
        zh: "工作区已切换。图片预览已暂停。"
    },
    FilesLoadingFile = { en: "Loading file…", zh: "正在加载文件…" },
    FilesCannotPreview = { en: "This file cannot be previewed.", zh: "无法预览该文件。" },
    FilesPreviewTruncated = {
        en: "Large file preview is truncated and read-only.",
        zh: "大文件预览已截断，且为只读。"
    },
    // Read-only reasons. `WorkspaceReadOnlyReason` maps to one of these ids;
    // no second copy of the English text lives in code.
    FilesReadOnlyBinary = { en: "Binary files cannot be previewed.", zh: "无法预览二进制文件。" },
    FilesReadOnlyEncoding = {
        en: "This file encoding is not supported.",
        zh: "不支持该文件的编码。"
    },
    FilesReadOnlySymlink = { en: "Symlink targets are read-only.", zh: "符号链接指向的文件为只读。" },
    FilesReadOnlyPermission = { en: "Permission denied.", zh: "没有访问权限。" },
    FilesReadOnlyTooLarge = { en: "This file is too large to preview.", zh: "该文件过大，无法预览。" },
    FilesReadOnlyMixedLineEndings = {
        en: "Files with mixed line endings are read-only.",
        zh: "换行符混用的文件为只读。"
    },

    // Files image and Markdown previews (files/image_preview.rs,
    // files/markdown_preview.rs). Raw client payloads stay as written; the
    // `Loading image…` placeholder reuses `AttachmentLoadingImage`.
    FilesImageRemoved = {
        en: "This image was removed from the workspace.",
        zh: "该图片已从工作区移除。"
    },
    FilesImageCheckoutUnavailable = {
        en: "Workspace checkout identity unavailable",
        zh: "无法获取工作区检出标识"
    },
    FilesImageTimedOut = { en: "Image preview timed out", zh: "图片预览超时" },
    FilesImageTooLarge = { en: "Image exceeds preview memory limit", zh: "图片超出预览内存上限" },
    FilesEnlargeImage = { en: "Enlarge image", zh: "放大图片" },
    FilesOpenImageLink = { en: "Open image link", zh: "打开图片链接" },
    FilesMermaidDiagram = { en: "Mermaid diagram", zh: "Mermaid 图表" },
    FilesRenderingDiagram = { en: "Rendering diagram…", zh: "正在渲染图表…" },
    FilesDiagramPreviewLimit = {
        en: "Document diagram preview limit reached",
        zh: "已达到文档图表预览上限"
    },
    FilesImagePreviewLimit = {
        en: "Document image preview limit reached",
        zh: "已达到文档图片预览上限"
    },
    FilesMediaPreviewLimit = {
        en: "Document media preview memory limit reached",
        zh: "已达到文档媒体预览内存上限"
    },
    FilesImageConnectionUnavailable = {
        en: "Workspace image connection unavailable",
        zh: "工作区图片连接不可用"
    },
    FilesImageOutsideWorkspace = { en: "Image path is outside the workspace", zh: "图片路径位于工作区之外" },
    FilesLoadingPreview = { en: "Loading preview…", zh: "正在加载预览…" },

    // Markdown rendering (markdown/render.rs, markdown/link_interaction.rs):
    // code-block actions and the link context menu. The transient copy feedback
    // reuses `CommonCopied`.
    MarkdownShowDiagram = { en: "Show diagram", zh: "显示图表" },
    MarkdownShowSource = { en: "Show source", zh: "显示源码" },
    MarkdownUseHorizontalScrolling = { en: "Use horizontal scrolling", zh: "使用横向滚动" },
    MarkdownFitContent = { en: "Fit content", zh: "适应内容宽度" },
    MarkdownOpenInZeron = { en: "Open in Zeron", zh: "在 Zeron 中打开" },
    MarkdownOpenInExternalBrowser = { en: "Open in external browser", zh: "在外部浏览器中打开" },
    MarkdownCopyLinkAddress = { en: "Copy link address", zh: "复制链接地址" },
    MarkdownOpenLinksInZeron = { en: "Open links in Zeron", zh: "在 Zeron 中打开链接" },
    MarkdownOpenLinksInZeronChecked = {
        en: "Open links in Zeron, checked",
        zh: "在 Zeron 中打开链接，已勾选"
    },
    MarkdownOpenLinksInZeronUnchecked = {
        en: "Open links in Zeron, unchecked",
        zh: "在 Zeron 中打开链接，未勾选"
    },
    MarkdownDiagramTooComplex = {
        en: "Diagram exceeds preview complexity limit",
        zh: "图表超出预览复杂度上限"
    },
    MarkdownDiagramTooLarge = {
        en: "Diagram output exceeds preview size limit",
        zh: "图表输出超出预览大小上限"
    },
    MarkdownDiagramRenderFailed = { en: "Diagram could not be rendered", zh: "无法渲染该图表" },

    // Settings → Keyboard shortcuts (settings.rs, shortcuts.rs). `{n}` takes the
    // jump slot ordinal; `Appshot` is Zeron's name for its screenshot surfaces.
    ShortcutCaptureAppshot = { en: "Capture Appshot", zh: "截取 Appshot" },
    ShortcutSaveFile = { en: "Save file", zh: "保存文件" },
    ShortcutBrowserReload = { en: "Reload browser page", zh: "重新加载浏览器页面" },
    ShortcutToggleSidebar = { en: "Toggle left sidebar", zh: "切换左侧边栏" },
    ShortcutToggleChanges = { en: "Toggle right sidebar", zh: "切换右侧边栏" },
    ShortcutToggleTerminal = { en: "Toggle terminal", zh: "切换终端" },
    ShortcutNewSession = { en: "New session", zh: "新建会话" },
    ShortcutNewProject = { en: "New project", zh: "新建项目" },
    ShortcutOpenModelPicker = { en: "Open model picker", zh: "打开模型选择器" },
    ShortcutNextSession = { en: "Next session", zh: "下一个会话" },
    ShortcutPrevSession = { en: "Previous session", zh: "上一个会话" },
    ShortcutArchiveSession = { en: "Archive session", zh: "归档会话" },
    ShortcutJumpSession = { en: "Jump to session {n}", zh: "跳到第 {n} 个会话" },

    // Settings → Notifications (notifications.rs). No placeholders; the page
    // header reuses `SettingsSectionNotifications`.
    NotificationsSubtitle = {
        en: "Choose which session events can play a sound, and when desktop notifications appear.",
        zh: "选择哪些会话事件可以播放提示音，以及何时显示桌面通知。"
    },
    NotificationsSessionSounds = { en: "Session sounds", zh: "会话提示音" },
    NotificationsSessionSoundsDescription = {
        en: "Allow sounds for the selected session events below.",
        zh: "允许下方选中的会话事件播放提示音。"
    },
    NotificationsTaskCompleted = { en: "Task completed", zh: "任务完成" },
    NotificationsTaskCompletedDescription = {
        en: "Play a sound when an agent finishes a run.",
        zh: "智能体完成一次运行时播放提示音。"
    },
    NotificationsTaskCompletedSound = { en: "Task completed sound", zh: "任务完成提示音" },
    NotificationsInputRequired = { en: "Input required", zh: "需要输入" },
    NotificationsInputRequiredDescription = {
        en: "Play a sound when an agent needs your response.",
        zh: "智能体需要你回复时播放提示音。"
    },
    NotificationsInputRequiredSound = { en: "Input required sound", zh: "需要输入提示音" },
    NotificationsErrorsAndDisconnections = { en: "Errors and disconnections", zh: "错误与断连" },
    NotificationsErrorsAndDisconnectionsDescription = {
        en: "Play a sound when a run fails or the connection remains unavailable.",
        zh: "运行失败或连接持续不可用时播放提示音。"
    },
    NotificationsErrorsAndDisconnectionsSound = {
        en: "Errors and disconnections sound",
        zh: "错误与断连提示音"
    },
    NotificationsDesktopNotifications = { en: "Desktop notifications", zh: "桌面通知" },
    NotificationsDesktopDescription = {
        en: "Show a system banner on the same events, so pings reach you while Zeron is in the background.",
        zh: "在相同事件上显示系统横幅，让 Zeron 处于后台时也能收到提醒。"
    },
    NotificationsBackgroundOnly = { en: "Only when in the background", zh: "仅在后台时" },
    NotificationsBackgroundOnlyDescription = {
        en: "Skip the banner while a Zeron window is focused.",
        zh: "Zeron 窗口处于焦点时不显示横幅。"
    },
    NotificationsBackgroundOnlyLabel = {
        en: "Only notify when Zeron is in the background",
        zh: "仅在 Zeron 处于后台时通知"
    },
    NotificationsUnavailableParentOff = {
        en: "Unavailable while its parent setting is off",
        zh: "父级设置关闭时不可用"
    },

    // Settings → Files page (settings/files.rs); the `Files*` prefix belongs to
    // the files browser. The header reuses `SettingsSectionFiles`, and the delay
    // pills keep their numeric `s` / `ms` suffixes. No placeholders.
    FilesSettingsSubtitle = {
        en: "Control how workspace files are displayed and saved while you edit.",
        zh: "控制编辑时工作区文件的显示与保存方式。"
    },
    FilesSettingsAutosave = { en: "Autosave", zh: "自动保存" },
    FilesSettingsAutosaveDescription = {
        en: "Save edited workspace files to disk automatically.",
        zh: "自动将编辑过的工作区文件保存到磁盘。"
    },
    FilesSettingsAutosaveDelay = { en: "Autosave delay", zh: "自动保存延迟" },
    FilesSettingsAutosaveDelayDescription = {
        en: "Save files after editing has been idle for this long.",
        zh: "编辑闲置达到该时长后保存文件。"
    },
    FilesSettingsWordWrap = { en: "Word wrap", zh: "自动换行" },
    FilesSettingsWordWrapDescription = {
        en: "Wrap long lines in every workspace file.",
        zh: "对每个工作区文件中的过长行自动换行。"
    },
    FilesSettingsShowAllFiles = { en: "Show all files", zh: "显示所有文件" },
    FilesSettingsShowAllFilesDescription = {
        en: "Include hidden and ignored files in every file tree.",
        zh: "在每个文件树中包含隐藏和被忽略的文件。"
    },

    // Settings → Appshots page (settings/appshots.rs). `Appshot` is Zeron's name
    // for its screenshot surfaces and stays as written; the header reuses
    // `SettingsSectionAppshots`. No placeholders.
    AppshotsCapture = { en: "Capture Appshots", zh: "启用 Appshot 截图" },
    AppshotsCaptureDescription = {
        en: "Captures are staged for review and never sent automatically.",
        zh: "截图会先暂存待确认，绝不会自动发送。"
    },
    AppshotsCaptureSound = { en: "Capture sound", zh: "截图提示音" },
    AppshotsCaptureSoundDescription = {
        en: "Play a sound when an Appshot is ready.",
        zh: "Appshot 就绪时播放提示音。"
    },
    AppshotsPreferredShortcut = { en: "Preferred shortcut", zh: "首选快捷键" },
    AppshotsGlobalShortcut = { en: "Global shortcut", zh: "全局快捷键" },
    AppshotsDestination = { en: "Destination", zh: "目标位置" },
    AppshotsDestinationAutomaticDescription = {
        en: "Use the open session, or the new-session composer when no session is open.",
        zh: "使用当前打开的会话；没有打开的会话时使用新会话输入框。"
    },
    AppshotsDestinationLastSessionDescription = {
        en: "Use the open session, or return to the last session used for an Appshot.",
        zh: "使用当前打开的会话；否则回到上次用于 Appshot 的会话。"
    },
    AppshotsDestinationNewSessionDescription = {
        en: "Stage captures in a new-session composer, keeping existing drafts intact.",
        zh: "在新会话输入框中暂存截图，不影响已有的草稿。"
    },
    AppshotsWindowCapture = { en: "Window capture", zh: "窗口截图" },
    AppshotsApplicationText = { en: "Application text", zh: "应用文本" },
    AppshotsAllowWindowCapture = { en: "Allow window capture", zh: "允许窗口截图" },
    AppshotsEnableTextCapture = { en: "Enable text capture", zh: "启用文本截取" },
    AppshotsOpenSystemSettings = { en: "Open System Settings", zh: "打开系统设置" },
    AppshotsCheckAgain = { en: "Check again", zh: "再次检查" },
    AppshotsCheckPermissionsAgain = { en: "Check permissions again", zh: "再次检查权限" },
    AppshotsPermissionRefreshHint = {
        en: "Changed a permission? Check again after returning to Zeron.",
        zh: "改过权限设置？回到 Zeron 后再检查一次。"
    },

    // Settings → Keyboard shortcuts page copy (settings/shortcuts.rs).
    // `{shortcut}`, `{combo}`, `{owner}`, and `{modifiers}` take data. Group
    // names reuse `SettingsSectionFiles`, `SurfaceBrowser`, and
    // `SettingsSectionAppshots`; the group keys themselves stay English.
    ShortcutsTitle = { en: "Keyboard shortcuts", zh: "键盘快捷键" },
    ShortcutsSubtitle = {
        en: "Click a binding, then press the key combination you want to use. Changes apply immediately and stay on this device.",
        zh: "点击某个绑定，然后按下你想使用的按键组合。更改会立即生效，并且只保存在本机。"
    },
    ShortcutsRestoreDefaults = { en: "Restore defaults", zh: "恢复默认设置" },
    ShortcutsEscapeStopsAgent = {
        en: "Stop active agent with Escape",
        zh: "用 Escape 停止活动智能体"
    },
    ShortcutsEscapeStopsAgentDescription = {
        en: "When no dialog, menu, picker, or terminal handles Escape, stop the agent in the active session.",
        zh: "当没有对话框、菜单、选择器或终端处理 Escape 时，停止当前会话中的智能体。"
    },
    ShortcutsSendMessagesWith = { en: "Send messages with", zh: "发送消息的按键" },
    ShortcutsSendBehaviorDescription = {
        en: "Choose whether Enter sends immediately or starts a new paragraph. Cmd/Ctrl+Enter always submits; with an empty composer it sends the most recently queued message. Shift+Enter always inserts a line break.",
        zh: "选择 Enter 是立即发送还是开始新段落。Cmd/Ctrl+Enter 始终提交；输入框为空时，它会发送最近排队的消息。Shift+Enter 始终插入换行。"
    },
    ShortcutsPressKeys = { en: "Press keys…", zh: "请按下按键…" },
    ShortcutsMustBeUnique = { en: "Shortcuts must be unique.", zh: "快捷键不能重复。" },
    ShortcutsUseKeyWithModifier = {
        en: "Use {modifiers} with a letter, number, function key or navigation key.",
        zh: "请将 {modifiers} 与字母、数字、功能键或导航键配合使用。"
    },
    ShortcutsModifierKeysMac = {
        en: "Control, Option or Command",
        zh: "Control、Option 或 Command"
    },
    ShortcutsModifierKeysOther = { en: "Control or Alt", zh: "Control 或 Alt" },
    ShortcutsComboReserved = {
        en: "{combo} is reserved for the composer.",
        zh: "{combo} 已保留给输入框。"
    },
    ShortcutsComboAlreadyAssigned = {
        en: "{combo} is already assigned to {owner}.",
        zh: "{combo} 已分配给 {owner}。"
    },
    ShortcutsResetAria = { en: "Reset {shortcut} shortcut", zh: "重置 {shortcut} 快捷键" },
    ShortcutsChangeAria = {
        en: "Change {shortcut} shortcut: {combo}",
        zh: "更改 {shortcut} 快捷键：{combo}"
    },
    ShortcutsGroupPanels = { en: "Panels", zh: "面板" },
    ShortcutsGroupSessions = { en: "Sessions", zh: "会话" },
    ShortcutsGroupProjects = { en: "Projects", zh: "项目" },
    ShortcutsGroupJumpToSession = { en: "Jump to session", zh: "跳转到会话" },
    ShortcutsCaptureAppshotDescription = {
        en: "Capture the focused application from anywhere on your desktop.",
        zh: "在桌面任意位置截取当前获得焦点的应用。"
    },
    ShortcutsSaveFileDescription = {
        en: "Save the active workspace file.",
        zh: "保存当前工作区文件。"
    },
    ShortcutsBrowserReloadDescription = {
        en: "Reload the focused browser tab.",
        zh: "重新加载当前的浏览器标签页。"
    },
    ShortcutsToggleSidebarDescription = {
        en: "Show or hide sessions and settings navigation.",
        zh: "显示或隐藏会话与设置导航。"
    },
    ShortcutsToggleChangesDescription = {
        en: "Show or hide the right sidebar for the current session.",
        zh: "显示或隐藏当前会话的右侧边栏。"
    },
    ShortcutsToggleTerminalDescription = {
        en: "Show or hide the terminal for the current session.",
        zh: "显示或隐藏当前会话的终端。"
    },
    ShortcutsNewSessionDescription = {
        en: "Open a blank session canvas to start a new session.",
        zh: "打开空白会话画布以开始新会话。"
    },
    ShortcutsNewProjectDescription = {
        en: "Open the new project dialog.",
        zh: "打开新建项目对话框。"
    },
    ShortcutsOpenModelPickerDescription = {
        en: "Open the model picker for the current session.",
        zh: "为当前会话打开模型选择器。"
    },
    ShortcutsNextSessionDescription = {
        en: "Select the next session in the sidebar, wrapping at the end.",
        zh: "选择侧边栏中的下一个会话，到末尾后回到开头。"
    },
    ShortcutsPrevSessionDescription = {
        en: "Select the previous session in the sidebar, wrapping at the start.",
        zh: "选择侧边栏中的上一个会话，到开头后跳到末尾。"
    },
    ShortcutsArchiveSessionDescription = {
        en: "Move the current session to the archived shelf.",
        zh: "将当前会话移到归档区。"
    },
    ShortcutsJumpSessionDescription = {
        en: "Open the session at this place in the sidebar list.",
        zh: "打开侧边栏列表中该位置的会话。"
    },

    // Settings → Appshots capability copy (crates/ui/src/appshots.rs): the
    // destination and status-badge labels plus the per-platform setup,
    // shortcut, capture, and text descriptions. `Appshot` is Zeron's name for
    // its screenshot surfaces and stays as written; the header and row titles
    // reuse `SettingsSectionAppshots` and the `Appshots*` rows above.
    AppshotsBadgeChecking = { en: "Checking", zh: "检查中" },
    AppshotsBadgeReady = { en: "Ready", zh: "就绪" },
    AppshotsBadgeRequired = { en: "Required", zh: "需要授权" },
    AppshotsBadgeSetUp = { en: "Set up", zh: "需要设置" },
    AppshotsBadgeSelectWindow = { en: "Select window", zh: "选择窗口" },
    AppshotsBadgeUnavailable = { en: "Unavailable", zh: "不可用" },
    AppshotsDestinationAutomatic = { en: "Automatic", zh: "自动" },
    AppshotsDestinationLastSession = { en: "Last session", zh: "上次会话" },
    AppshotsDestinationNewSession = { en: "New session", zh: "新会话" },
    AppshotsSetupMacOs = {
        en: "Set up once, one permission at a time. Screen Recording captures the window; Accessibility optionally adds off-screen application text.",
        zh: "一次设置，逐个授权。屏幕录制用于截取窗口；辅助功能可选择性地补充屏幕外的应用文本。"
    },
    AppshotsSetupWayland = {
        en: "Your desktop owns capture and shortcut consent. Zeron checks each portal capability separately and explains any required fallback.",
        zh: "截图与快捷键授权由你的桌面环境管理。Zeron 会分别检查每项门户能力，并说明所需的回退方式。"
    },
    AppshotsSetupX11 = {
        en: "X11 normally needs no capture permission. Zeron prefers an active-window screenshot portal when available and otherwise uses native X11 capture.",
        zh: "X11 通常不需要截图权限。Zeron 在可用时优先使用活动窗口截图门户，否则使用原生 X11 截图。"
    },
    AppshotsSetupUnsupported = {
        en: "This platform does not currently provide an Appshot capture backend.",
        zh: "当前平台尚未提供 Appshot 截图后端。"
    },
    AppshotsShortcutUnavailable = {
        en: "This shortcut is unavailable. Choose a different key combination.",
        zh: "该快捷键不可用。请选择其他按键组合。"
    },
    AppshotsShortcutGlobal = {
        en: "The shortcut works while another application has focus.",
        zh: "其他应用获得焦点时该快捷键仍然有效。"
    },
    AppshotsShortcutWaylandPortal = {
        en: "Your desktop portal controls the binding. Confirm changes in its shortcut settings.",
        zh: "绑定由你的桌面门户控制。请在它的快捷键设置中确认更改。"
    },
    AppshotsShortcutWaylandManual = {
        en: "Bind `zeron appshot` in your desktop's Keyboard Shortcuts settings.",
        zh: "在桌面的“键盘快捷键”设置中为 `zeron appshot` 绑定按键。"
    },
    AppshotsShortcutUnsupported = {
        en: "This platform has no Appshot shortcut backend.",
        zh: "该平台没有 Appshot 快捷键后端。"
    },
    AppshotsCaptureMacOs = {
        en: "Screen Recording lets Zeron capture the frontmost window. macOS may request one restart.",
        zh: "屏幕录制权限让 Zeron 可以截取最前面的窗口。macOS 可能要求重启一次。"
    },
    AppshotsCaptureX11 = {
        en: "Zeron uses native X11 capture when active-window portal capture is unavailable. Obscured or protected windows may be incomplete.",
        zh: "活动窗口门户截图不可用时，Zeron 使用原生 X11 截图。被遮挡或受保护的窗口可能不完整。"
    },
    AppshotsCaptureWaylandActiveWindow = {
        en: "Your screenshot portal supports the active-window target. A system consent surface may appear.",
        zh: "你的截图门户支持活动窗口目标。可能会出现系统授权界面。"
    },
    AppshotsCaptureWaylandPicker = {
        en: "Your portal requires choosing a window for each capture.",
        zh: "你的门户要求每次截图都选择一个窗口。"
    },
    AppshotsCaptureUnsupported = {
        en: "Active-window capture is unavailable on this platform.",
        zh: "该平台不支持活动窗口截图。"
    },
    AppshotsSemanticMacOs = {
        en: "Accessibility adds visible and off-screen application text. Screenshots work without it.",
        zh: "辅助功能权限可补充可见及屏幕外的应用文本。没有该权限也能截图。"
    },
    AppshotsSemanticWayland = {
        en: "This portal does not identify the captured window, so Appshots include the screenshot only.",
        zh: "该门户无法识别被截取的窗口，因此 Appshot 只包含截图。"
    },
    AppshotsSemanticX11 = {
        en: "Native X11 captures can include AT-SPI text when the process and window can be matched uniquely. Portal captures include the screenshot only.",
        zh: "当进程与窗口可以唯一匹配时，原生 X11 截图可包含 AT-SPI 文本。门户截图只包含截图。"
    },
    AppshotsSemanticUnsupported = {
        en: "Semantic application text is unavailable on this platform.",
        zh: "该平台不支持语义应用文本。"
    },

    // Settings → Agents (harnesses.rs). {cli} takes a CLI name, {err} an
    // engine payload; both stay as written.
    HarnessesBlurbClaudeCode = {
        en: "Anthropic's coding agent, driven through the Claude Code CLI.",
        zh: "Anthropic 的编码智能体，通过 Claude Code CLI 驱动。"
    },
    HarnessesBlurbCodex = {
        en: "OpenAI's coding agent, driven through the Codex CLI.",
        zh: "OpenAI 的编码智能体，通过 Codex CLI 驱动。"
    },
    HarnessesBlurbCursor = {
        en: "Cursor's coding agent, driven through the cursor-agent CLI.",
        zh: "Cursor 的编码智能体，通过 cursor-agent CLI 驱动。"
    },
    HarnessesBlurbDevin = {
        en: "Cognition's Devin agent (devin CLI).",
        zh: "Cognition 的 Devin 智能体（devin CLI）。"
    },
    HarnessesBlurbGrok = {
        en: "xAI's Grok Build agent (grok CLI).",
        zh: "xAI 的 Grok Build 智能体（grok CLI）。"
    },
    HarnessesBlurbHermes = {
        en: "Nous Research's Hermes Agent (hermes CLI).",
        zh: "Nous Research 的 Hermes Agent（hermes CLI）。"
    },
    HarnessesBlurbPi = {
        en: "The pi coding agent (pi CLI).",
        zh: "pi 编码智能体（pi CLI）。"
    },
    HarnessesBlurbOpencode = {
        en: "SST's opencode agent (opencode CLI).",
        zh: "SST 的 opencode 智能体（opencode CLI）。"
    },
    HarnessesBlurbAntigravity = {
        en: "Google's Antigravity agent (Antigravity ACP server).",
        zh: "Google 的 Antigravity 智能体（Antigravity ACP 服务器）。"
    },
    HarnessesBlurbMock = {
        en: "Scripted test harness.",
        zh: "脚本化的测试 harness。"
    },
    HarnessesPendingStarting = { en: "Preparing Antigravity…", zh: "正在准备 Antigravity…" },
    HarnessesPendingInstalling = { en: "Installing Antigravity…", zh: "正在安装 Antigravity…" },
    HarnessesPendingAuthenticating = {
        en: "Finish signing in in your browser.",
        zh: "请在浏览器中完成登录。"
    },
    HarnessesPendingEnabling = { en: "Enabling Antigravity…", zh: "正在启用 Antigravity…" },
    HarnessesFailureStarting = { en: "Setup failed", zh: "设置失败" },
    HarnessesFailureInstalling = { en: "Installation failed", zh: "安装失败" },
    HarnessesFailureAuthenticating = { en: "Sign-in failed", zh: "登录失败" },
    HarnessesFailureEnabling = { en: "Enable failed", zh: "启用失败" },
    HarnessesSessionTitles = { en: "Session titles", zh: "会话标题" },
    HarnessesSessionTitlesSubtitle = {
        en: "Choose the agent and model for automatic titles on this device. Claude Code and Codex support restricted title generation.",
        zh: "选择在本机自动生成标题所用的智能体与模型。Claude Code 和 Codex 支持受限的标题生成。"
    },
    HarnessesTitleSettingsLoading = { en: "Loading title settings…", zh: "正在加载标题设置…" },
    HarnessesTitleModelAutomaticCheapest = {
        en: "Automatic (cheapest model)",
        zh: "自动（最便宜的模型）"
    },
    HarnessesTitleHarnessAutomatic = {
        en: "Automatic (session agent when supported)",
        zh: "自动（会话智能体，受支持时）"
    },
    HarnessesTitleModel = { en: "Title model", zh: "标题模型" },
    HarnessesTitleHarness = { en: "Title harness", zh: "标题智能体" },
    HarnessesTitleAutomatic = { en: "Automatic", zh: "自动" },
    HarnessesSignInOtherDevice = {
        en: "Turn this agent on from its own device to sign in.",
        zh: "请在该智能体所在的设备上启用它才能登录。"
    },
    HarnessesSignInStartFailed = { en: "Sign-in failed to start: {err}", zh: "登录启动失败：{err}" },
    HarnessesUnknownError = { en: "Unknown error", zh: "未知错误" },
    HarnessesEngineUnavailable = { en: "Engine unavailable", zh: "引擎不可用" },
    HarnessesNotInstalledEnabled = {
        en: "{cli} CLI not installed — turn it off or install it",
        zh: "{cli} CLI 未安装 — 请将其关闭或安装它"
    },
    HarnessesNotInstalledEnable = {
        en: "Install the {cli} CLI to enable",
        zh: "安装 {cli} CLI 后才能启用"
    },
    HarnessesSubtitle = {
        en: "Choose which coding agents the composer offers. The setting is per device — switch devices in the header. Agents whose CLI isn't installed on a device can't be enabled there.",
        zh: "选择输入框提供哪些编码智能体。该设置按设备生效 — 可在页首切换设备。CLI 未安装在某设备上的智能体无法在该设备启用。"
    },

    // Settings → Accounts (accounts.rs). {provider} takes a provider name,
    // {name} a product name and {cli} a CLI name; {err} an engine payload — all
    // stay as written. The page header reuses `SettingsSectionAgents`, the
    // device switcher rows `PickerThisDevice` / `PickerDeviceYou` /
    // `CommonDevicesMenu`, and the plain actions `CommonCancel`, `CommonClose`,
    // `CommonRefresh`, and `ErrorEngineNotConnected`.
    AccountsAddProviderTitle = { en: "Add {provider} account", zh: "添加 {provider} 账户" },
    AccountsConnectProviderTitle = { en: "Connect {provider}", zh: "连接 {provider}" },
    AccountsSubtitle = {
        en: "The Claude Code, Codex, and Cursor logins on this device. Zeron detects the live session, keeps each account backed up, and can swap between them.",
        zh: "本机上已有的 Claude Code、Codex 和 Cursor 登录。Zeron 会识别当前会话，为每个账户保留备份，并可在它们之间切换。"
    },
    AccountsAuthCodePlaceholder = { en: "Paste the authorization code", zh: "粘贴授权码" },
    AccountsResets = { en: "resets {time}", zh: "重置于 {time}" },
    AccountsUsagePercent = { en: "{percent}% used", zh: "已用 {percent}%" },
    AccountsUnknownAccount = { en: "Unknown account", zh: "未知账户" },
    AccountsBadgeActive = { en: "Active", zh: "当前使用" },
    AccountsSwitch = { en: "Switch", zh: "切换" },
    AccountsSwitching = { en: "Switching…", zh: "正在切换…" },
    AccountsUsageUnavailable = { en: "Usage unavailable", zh: "用量不可用" },
    AccountsCredentialsUnavailable = { en: "Credentials unavailable", zh: "凭据不可用" },
    AccountsPasteCodeBody = {
        en: "A browser window opened. Sign in to the account you want to add, approve access, then paste the code Anthropic shows you below. Your current login is untouched until you switch.",
        zh: "浏览器窗口已打开。请登录要添加的账户、批准访问，然后把 Anthropic 显示给你的验证码粘贴到下方。在你切换之前，当前登录不受影响。"
    },
    AccountsReopenAuthorization = { en: "Reopen the authorization page", zh: "重新打开授权页面" },
    AccountsVerifying = { en: "Verifying…", zh: "正在验证…" },
    AccountsAddAccount = { en: "Add account", zh: "添加账户" },
    AccountsBrowserBodyCursor = {
        en: "Finish signing in to Cursor in your browser. This mints a zeron-named API key you can revoke any time from Cursor's dashboard — it is separate from `cursor-agent login`.",
        zh: "请在浏览器中完成 Cursor 登录。这会生成一个以 zeron 命名的 API 密钥，你随时可以在 Cursor 控制台中撤销 — 它与 `cursor-agent login` 相互独立。"
    },
    AccountsBrowserBodyGeneric = {
        en: "Finish signing in to OpenAI in your browser. The new login is captured in an isolated profile — your current session is untouched until you switch.",
        zh: "请在浏览器中完成 OpenAI 登录。新登录会保存在一个隔离的配置文件中 — 在你切换之前，当前会话不受影响。"
    },
    AccountsReopenSignIn = { en: "Reopen the sign-in page", zh: "重新打开登录页面" },
    AccountsWaitingForBrowser = { en: "Waiting for the browser…", zh: "正在等待浏览器…" },
    AccountsLoginStartFailed = { en: "Login failed to start: {err}", zh: "登录启动失败：{err}" },
    AccountsLoginFailed = { en: "Login failed", zh: "登录失败" },
    AccountsPollFailed = { en: "Poll failed: {err}", zh: "轮询失败：{err}" },
    AccountsPollMalformed = { en: "Poll failed: malformed reply", zh: "轮询失败：回复格式错误" },
    AccountsClickToRetry = { en: "Click to retry", zh: "点击重试" },
    AccountsEmptyCursor = {
        en: "{name} isn't connected on this device — connect it to run Cursor sessions.",
        zh: "{name} 尚未在此设备上连接 — 连接后即可运行 Cursor 会话。"
    },
    AccountsEmptyGeneric = {
        en: "No {name} login detected on this device — sign in with \u{201C}{cli}\u{201D} or add an account.",
        zh: "未在此设备上检测到 {name} 登录 — 请使用\u{201C}{cli}\u{201D}登录，或添加账户。"
    },
    AccountsFooterNote = {
        en: "Switching rewrites the CLI\u{2019}s stored login, so new agent sessions use the selected account immediately. On macOS, an already-running Claude Code can hold the previous login for up to ~30 seconds (Keychain cache).",
        zh: "切换会改写 CLI 保存的登录，因此新的智能体会话会立即使用所选账户。在 macOS 上，已在运行的 Claude Code 可能还会沿用之前的登录约 30 秒（Keychain 缓存）。"
    },
    // Appshot capture errors (appshots.rs). `{width}`, `{height}`, `{platform}`,
    // and `{err}` take verbatim values.
    AppshotErrorPermissionRequired = {
        en: "Window capture permission is required. Open Zeron Settings → Appshots for the platform-specific recovery step.",
        zh: "需要窗口截图权限。请打开 Zeron 设置 → Appshots，查看适用于当前平台的恢复步骤。"
    },
    AppshotErrorCancelled = { en: "Appshot capture cancelled.", zh: "已取消 Appshot 截图。" },
    AppshotErrorSelfCapture = {
        en: "Switch to another app to capture an Appshot.",
        zh: "请切换到其他应用后再截取 Appshot。"
    },
    AppshotErrorNoEligibleWindow = {
        en: "No application window is available to capture.",
        zh: "没有可截图的应用程序窗口。"
    },
    AppshotErrorShortcutUnavailable = {
        en: "The Appshot shortcut could not be registered because another app may be using it.",
        zh: "无法注册 Appshot 快捷键，可能已被其他应用占用。"
    },
    AppshotErrorUnavailable = {
        en: "Appshots are not available on this platform.",
        zh: "此平台不支持 Appshots。"
    },
    AppshotErrorDimensionsUnsupported = {
        en: "The captured window dimensions ({width}×{height}) are not supported.",
        zh: "截取的窗口尺寸（{width}×{height}）不受支持。"
    },
    AppshotErrorTooLarge = { en: "The captured window is too large.", zh: "截取的窗口过大。" },
    AppshotErrorBudgetExceeded = {
        en: "The captured window ({width}×{height}) exceeds Zeron's capture budget.",
        zh: "截取的窗口（{width}×{height}）超出 Zeron 的截图上限。"
    },
    AppshotErrorDecodeFailed = {
        en: "Could not decode the captured window: {err}",
        zh: "无法解码截取的窗口：{err}"
    },
    AppshotErrorImageLimit = {
        en: "The captured window is larger than Zeron's 24 MB image limit.",
        zh: "截取的窗口超过 Zeron 的 24 MB 图像上限。"
    },
    AppshotErrorInvalidPng = {
        en: "The captured window is not a valid PNG image.",
        zh: "截取的窗口不是有效的 PNG 图像。"
    },
    AppshotErrorEncodeFailed = {
        en: "Could not encode the captured window: {err}",
        zh: "无法编码截取的窗口：{err}"
    },
    AppshotErrorPlatformPixelBuffer = {
        en: "{platform} returned an invalid pixel buffer.",
        zh: "{platform} 返回了无效的像素缓冲区。"
    },
    AppshotErrorPlatformEncodeFailed = {
        en: "{platform} Appshot encoding failed: {err}",
        zh: "{platform} Appshot 编码失败：{err}"
    },
    // Appshot platform backends (appshots/macos.rs, appshots/linux/*): copy that
    // reaches the user through `CaptureError`. Platform and protocol names
    // (X11, ScreenCaptureKit, Zeron, URI) stay untranslated, and `{err}` takes
    // the verbatim platform error. `AppshotErrorImageLimit` above is reused by
    // macOS, whose 24 MB check has the same copy and meaning.
    AppshotErrorMacWindowUnavailable = {
        en: "The application window could not be captured.",
        zh: "无法截取应用程序窗口。"
    },
    AppshotErrorMacEmptyImage = {
        en: "The captured window was empty.",
        zh: "截取的窗口内容为空。"
    },
    AppshotErrorScreenshotWidthBudget = {
        en: "Screenshot width exceeds the capture budget",
        zh: "截图宽度超出截图上限"
    },
    AppshotErrorScreenshotHeightBudget = {
        en: "Screenshot height exceeds the capture budget",
        zh: "截图高度超出截图上限"
    },
    AppshotErrorScreenshotEncodeFailed = {
        en: "The screenshot could not be encoded.",
        zh: "无法编码截图。"
    },
    AppshotErrorActivationSocket = {
        en: "Could not create Appshot activation socket: {err}",
        zh: "无法创建 Appshot 激活套接字：{err}"
    },
    AppshotErrorActivationUnreachable = {
        en: "Could not reach a running Zeron instance for Appshot capture: {err}",
        zh: "无法连接到正在运行的 Zeron 实例以执行 Appshot 截图：{err}"
    },
    AppshotErrorPortalUnavailable = {
        en: "Screenshot portal unavailable: {err}",
        zh: "截图门户不可用：{err}"
    },
    AppshotErrorPortalUnsupported = {
        en: "This screenshot portal does not support window-only capture. Update your desktop portal to use Appshots.",
        zh: "此截图门户不支持仅截取窗口。请更新桌面门户以使用 Appshot。"
    },
    AppshotErrorPortalImageUri = {
        en: "Invalid portal image URI: {err}",
        zh: "门户图像 URI 无效：{err}"
    },
    AppshotErrorPortalNonFileUri = {
        en: "Screenshot portal returned a non-file URI.",
        zh: "截图门户返回了非文件 URI。"
    },
    AppshotErrorPortalInspect = {
        en: "Could not inspect portal screenshot: {err}",
        zh: "无法检查门户截图：{err}"
    },
    AppshotErrorPortalImageLimit = {
        en: "The portal screenshot is larger than Zeron's 24 MB image limit.",
        zh: "门户截图超过 Zeron 的 24 MB 图像上限。"
    },
    AppshotErrorPortalOpen = {
        en: "Could not open portal screenshot: {err}",
        zh: "无法打开门户截图：{err}"
    },
    AppshotErrorPortalRead = {
        en: "Could not read portal screenshot: {err}",
        zh: "无法读取门户截图：{err}"
    },
    AppshotErrorPortalSizeChanged = {
        en: "The portal screenshot changed size while it was being read.",
        zh: "读取门户截图时其大小发生了变化。"
    },
    AppshotErrorPortalFailed = {
        en: "Screenshot portal failed: {err}",
        zh: "截图门户失败：{err}"
    },
    AppshotErrorX11UnknownVisual = {
        en: "X11 returned an unknown visual.",
        zh: "X11 返回了未知的 visual。"
    },
    AppshotErrorX11WindowChanged = {
        en: "The active X11 window changed during capture; try again.",
        zh: "截图期间活动的 X11 窗口发生了变化，请重试。"
    },
    AppshotErrorX11CaptureFailed = {
        en: "X11 Appshot capture failed: {err}",
        zh: "X11 Appshot 截图失败：{err}"
    },
    // Files client errors (files/client.rs). The text after the colon is a
    // verbatim engine or serde payload, so `{err}` is never translated.
    FilesClientEncodeFailed = {
        en: "workspace request could not be encoded: {err}",
        zh: "工作区请求编码失败：{err}"
    },
    FilesClientDecodeFailed = {
        en: "workspace response was invalid: {err}",
        zh: "工作区响应无效：{err}"
    },
    FilesClientTransportFailed = {
        en: "workspace connection unavailable: {err}",
        zh: "工作区连接不可用：{err}"
    },
    FilesClientRequestFailed = {
        en: "workspace request failed: {err}",
        zh: "工作区请求失败：{err}"
    },
    // Conversation deep links: the parse failures in links.rs and the two
    // notices state.rs raises once a parsed link cannot be opened.
    LinksNotConversationLink = { en: "not a Zeron conversation link", zh: "不是 Zeron 会话链接" },
    LinksMissingWorkspaceLocator = {
        en: "missing workspace locator",
        zh: "缺少工作区定位信息"
    },
    LinksInvalidConversationId = { en: "invalid conversation id", zh: "会话 ID 无效" },
    LinksInvalidUrlEscape = { en: "invalid URL escape", zh: "无效的 URL 转义序列" },
    LinksInvalidUtf8 = { en: "invalid UTF-8 in URL", zh: "URL 中的 UTF-8 无效" },
    ChatMenuHarnessLink = { en: "Codex conversation link", zh: "Codex 会话链接" },
    StateDeepLinkOtherWorkspace = {
        en: "This conversation link belongs to another workspace",
        zh: "此会话链接属于其他工作区"
    },
    StateDeepLinkMissing = {
        en: "The linked conversation was not found",
        zh: "未找到所链接的会话"
    },
    // Devices page chrome and its rename flow (settings/devices.rs).
    CommonRename = { en: "Rename", zh: "重命名" },
    DevicesSubtitleLocal = {
        en: "Manage device details stored in this local workspace.",
        zh: "管理存储在此本地工作区中的设备详情。"
    },
    DevicesSubtitleSynced = {
        en: "Manage device names and inspect synced device metadata.",
        zh: "管理设备名称并查看已同步的设备元数据。"
    },
    DevicesSubtitleDefault = {
        en: "Manage device names for this workspace.",
        zh: "管理此工作区的设备名称。"
    },
    DevicesRenameTitle = { en: "Rename device", zh: "重命名设备" },
    DevicesRenamePlaceholder = { en: "Device name", zh: "设备名称" },
    DevicesEmpty = { en: "No devices registered", zh: "尚无已注册设备" },
    DevicesRenameFailed = { en: "Rename failed: {err}", zh: "重命名失败：{err}" },
    // Image decode limits that reach the preview panes (image_media.rs); the
    // caller maps them into `MediaFailure`, and crate payloads stay `Detail`.
    FilesImageSizeLimit = { en: "Image exceeds preview size limit", zh: "图片超出预览大小上限" },
    FilesImageSvgSizeLimit = {
        en: "Prepared SVG exceeds preview size limit",
        zh: "预处理后的 SVG 超出预览大小上限"
    },
    // Comment widget aria labels (comment_ui.rs).
    FilesAddComment = { en: "Add comment", zh: "添加评论" },
    FilesEditComment = { en: "Edit comment", zh: "编辑评论" },
    // Context-usage card (context_usage.rs). The numbers are formatted by the
    // caller and inserted verbatim.
    ContextUsageTitle = { en: "Context window", zh: "上下文窗口" },
    ContextUsageRemaining = {
        en: "{used} / {total} tokens\n{remaining} tokens remaining",
        zh: "{used} / {total} tokens\n剩余 {remaining} tokens"
    },
    ContextUsageUsed = {
        en: "{used} tokens used\nContext limit not reported",
        zh: "已使用 {used} tokens\n未上报上下文上限"
    },
    ContextUsageWaiting = {
        en: "{capacity} token capacity\nWaiting for context usage",
        zh: "{capacity} tokens 容量\n正在等待上下文用量"
    },
    ContextUsageNotReported = {
        en: "Context usage not reported by this harness yet",
        zh: "此 Harness 尚未上报上下文用量"
    },
    // Boot splash (loaders.rs).
    LoadersBootSplash = { en: "Setting up Zeron environment", zh: "正在准备 Zeron 运行环境" },

    // Browser pane, cross-platform chrome (browser/mod.rs, browser/view.rs).
    // `{name}` takes the dev-server name, `{error}` a platform payload.
    BrowserAddressPlaceholder = {
        en: "Website or localhost:3000",
        zh: "网站或 localhost:3000"
    },
    BrowserPreviewConnecting = { en: "Connecting to preview discovery…", zh: "正在连接预览发现服务…" },
    BrowserOpenFailed = { en: "Could not open this page: {error}", zh: "无法打开此页面：{error}" },
    BrowserPreviewsRemoteSubtitle = { en: "Running on your device", zh: "正在你的设备上运行" },
    BrowserPreviewsLocalSubtitle = { en: "Running locally", zh: "正在本地运行" },
    BrowserPreviewsSearching = { en: "Looking for dev servers…", zh: "正在查找开发服务器…" },
    BrowserPreviewsEmptyRemote = {
        en: "Start a dev server in this project on your other device. Its preview will appear here when that device is online.",
        zh: "在你的另一台设备上为该项目启动开发服务器。该设备在线后，其预览会显示在这里。"
    },
    BrowserPreviewsEmptyLocal = {
        en: "Start a dev server in this project. It will appear here automatically, ready to open.",
        zh: "在此项目中启动开发服务器。它会自动显示在这里，可直接打开。"
    },
    BrowserPreviewOpen = { en: "Open", zh: "打开" },
    BrowserPreviewOpenAria = { en: "Open {name} preview", zh: "打开 {name} 预览" },
    BrowserEnterAddressAria = { en: "Enter a website address", zh: "输入网站地址" },
    BrowserEnterAddressOr = { en: "Or enter a website address", zh: "或输入网站地址" },
    BrowserEnterAddress = { en: "Enter an address", zh: "输入地址" },
    BrowserLoadFailedTitle = { en: "Couldn’t load this page", zh: "无法加载此页面" },
    BrowserOpenedExternallyTitle = { en: "Opened in your browser", zh: "已在浏览器中打开" },
    BrowserEmptyTitle = { en: "Preview your work", zh: "预览你的成果" },
    BrowserEmptyExternalDescription = {
        en: "Open a website or local app in your default browser. Embedded browsing is available on macOS and Linux.",
        zh: "在默认浏览器中打开网站或本地应用。嵌入式浏览仅在 macOS 和 Linux 上可用。"
    },
    BrowserEmptyDescription = {
        en: "Preview your local app or keep a website beside your conversation.",
        zh: "预览本地应用，或在对话旁打开网站。"
    },
    BrowserRetryPageAria = { en: "Retry page", zh: "重试加载页面" },
    BrowserTryAgain = { en: "Try again", zh: "重试" },
    BrowserForward = { en: "Forward", zh: "前进" },
    BrowserReload = { en: "Reload page", zh: "重新加载页面" },
    BrowserGoAria = { en: "Go to address", zh: "转到该地址" },
    BrowserOpenExternal = { en: "Open in default browser", zh: "在默认浏览器中打开" },
    BrowserRemoteLoopbackHint = {
        en: "Localhost opens on this device. Open a detected preview from a new tab to reach your other device.",
        zh: "localhost 会在此设备上打开。如需访问另一台设备，请在新标签页中打开检测到的预览。"
    },
    BrowserExternalFooter = { en: "Opens in your default browser", zh: "将在默认浏览器中打开" },
    // Browser address and chat-link validation (browser/model.rs). The chat-link
    // failures never reach a pane today; they are carried as keys so a future
    // display site cannot resurrect the English.
    BrowserAddressEmpty = { en: "Enter a website or localhost address.", zh: "请输入网站地址或 localhost 地址。" },
    BrowserAddressInvalidCharacters = {
        en: "This address contains invalid characters.",
        zh: "此地址包含无效字符。"
    },
    BrowserAddressInvalid = {
        en: "Enter a valid website or localhost address.",
        zh: "请输入有效的网站地址或 localhost 地址。"
    },
    BrowserAddressSchemeUnsupported = {
        en: "Only http and https addresses are supported.",
        zh: "仅支持 http 和 https 地址。"
    },
    BrowserAddressCredentials = {
        en: "Use an address without an embedded username or password.",
        zh: "请使用不含内嵌用户名和密码的地址。"
    },
    BrowserLinkUnsupported = {
        en: "Only explicit http and https links are supported.",
        zh: "仅支持显式的 http 和 https 链接。"
    },
    BrowserLinkInvalid = { en: "This link contains an invalid address.", zh: "此链接包含无效地址。" },
    BrowserLinkInvalidEscape = { en: "This link contains an invalid escape.", zh: "此链接包含无效的转义序列。" },
    // Browser pane failures raised by the platform host (browser/macos.rs,
    // browser/linux/mod.rs). `{error}` takes an OS or runtime payload.
    BrowserHelperStopped = {
        en: "The browser helper stopped. Check that WebKitGTK 4.1 is installed, then reopen the tab.",
        zh: "浏览器的渲染进程已停止。请确认已安装 WebKitGTK 4.1，然后重新打开该标签页。"
    },
    BrowserCacheDirUnavailable = { en: "Could not locate the browser cache directory", zh: "无法找到浏览器缓存目录" },
    BrowserWebkitStartFailed = {
        en: "Could not start WebKitGTK: {error}. Install the WebKitGTK 4.1 runtime for your distribution.",
        zh: "无法启动 WebKitGTK：{error}。请为你的发行版安装 WebKitGTK 4.1 运行时。"
    },
    BrowserNotMainThread = { en: "Browser must be created on the main thread", zh: "浏览器必须在主线程上创建" },
    BrowserParentMissing = { en: "Browser parent is missing", zh: "缺少浏览器的父视图" },
    BrowserUnsupportedContent = {
        en: "This file can’t be previewed here. Open it in your default browser.",
        zh: "此文件无法在此预览。请在默认浏览器中打开。"
    },
    BrowserAddressUnreachable = {
        en: "Check the address and make sure your server is running, then try again.",
        zh: "请检查地址并确认服务器正在运行，然后重试。"
    },
    BrowserConnectionInterrupted = {
        en: "The connection was interrupted. Try loading this page again.",
        zh: "连接已中断。请重新加载此页面。"
    },
    BrowserPageStopped = {
        en: "The page stopped responding. Reload to continue.",
        zh: "页面已停止响应。请重新加载以继续。"
    },
    // WebKitGTK page context menu (browser/linux/helper.c actions, labelled by
    // browser/linux/mod.rs). `CommonBack`, `EditCopy`, `EditPaste` and
    // `MarkdownCopyLinkAddress` are shared with the other menus; the rest are
    // browser-scoped because their copy differs from the closest neutral row.
    BrowserMenuOpenLink = { en: "Open link in new tab", zh: "在新标签页中打开链接" },
    BrowserMenuSelectAll = { en: "Select all", zh: "全选" },
    BrowserMenuReload = { en: "Reload", zh: "重新加载" },

    // Tool chips and the tool-group summary. The classification lives in
    // `zeron_proto::view` (`ToolChip`, `ToolChipDetail`, `ToolSummarySegment`)
    // so both viewports group and name a tool identically; the copy lives here
    // because only this viewport renders in a locale. The English rows below
    // must stay byte-identical to `zeron_proto`'s own renderers — a UI test
    // asserts it.
    ToolChipRun = { en: "Run", zh: "运行" },
    ToolChipRead = { en: "Read", zh: "读取" },
    ToolChipWrite = { en: "Write", zh: "写入" },
    ToolChipEdit = { en: "Edit", zh: "编辑" },
    ToolChipPatch = { en: "Patch", zh: "补丁" },
    ToolChipSearch = { en: "Search", zh: "搜索" },
    // A glob is a path-pattern matcher; the term is kept as written.
    ToolChipGlob = { en: "Glob", zh: "Glob" },
    ToolChipFetch = { en: "Fetch", zh: "抓取" },
    ToolChipWeb = { en: "Web", zh: "联网搜索" },
    ToolChipTodo = { en: "Todo", zh: "待办" },
    // Protocol name and product term; kept as written.
    ToolChipMcp = { en: "MCP", zh: "MCP" },
    ToolChipAgent = { en: "Agent", zh: "子代理" },
    ToolChipTool = { en: "Tool", zh: "工具" },
    ToolChipWorkspace = { en: "workspace", zh: "工作区" },
    ToolChipInPath = { en: "{pattern} in {path}", zh: "在 {path} 中搜索 {pattern}" },
    ToolChipTodoProgress = { en: "{done}/{total} done", zh: "已完成 {done}/{total}" },
    // One row per segment wording, so the Chinese line reads as a sentence
    // rather than a translated verb glued to an untranslated noun.
    TranscriptSummaryRanCommandsOne = { en: "ran {n} command", zh: "已运行 {n} 个命令" },
    TranscriptSummaryRanCommandsMany = { en: "ran {n} commands", zh: "已运行 {n} 个命令" },
    TranscriptSummaryEditedFilesOne = { en: "edited {n} file", zh: "已编辑 {n} 个文件" },
    TranscriptSummaryEditedFilesMany = { en: "edited {n} files", zh: "已编辑 {n} 个文件" },
    TranscriptSummaryReadFilesOne = { en: "read {n} file", zh: "已读取 {n} 个文件" },
    TranscriptSummaryReadFilesMany = { en: "read {n} files", zh: "已读取 {n} 个文件" },
    TranscriptSummarySearchedOne = { en: "searched {n} time", zh: "已搜索 {n} 次" },
    TranscriptSummarySearchedMany = { en: "searched {n} times", zh: "已搜索 {n} 次" },
    TranscriptSummaryFetchedPagesOne = { en: "fetched {n} page", zh: "已抓取 {n} 个页面" },
    TranscriptSummaryFetchedPagesMany = { en: "fetched {n} pages", zh: "已抓取 {n} 个页面" },
    TranscriptSummaryUpdatedTodos = { en: "updated todos", zh: "已更新待办" },
    TranscriptSummaryCalledToolsOne = { en: "called {n} tool", zh: "已调用 {n} 个工具" },
    TranscriptSummaryCalledToolsMany = { en: "called {n} tools", zh: "已调用 {n} 个工具" },
    TranscriptSummaryFailed = { en: "{n} failed", zh: "{n} 个失败" },
    TranscriptElapsedSeconds = { en: "{n}s", zh: "{n}秒" },
    TranscriptElapsedMinutes = { en: "{n}m {s}s", zh: "{n}分{s}秒" },
    TranscriptElapsedHours = { en: "{n}h {m}m", zh: "{n}小时{m}分钟" },
    TranscriptElapsedDays = { en: "{n}d {h}h", zh: "{n}天{h}小时" },
    CountToolOne = { en: "{n} tool", zh: "{n} 个工具" },
    CountToolMany = { en: "{n} tools", zh: "{n} 个工具" },
}

/// Keys whose Simplified Chinese copy is not written yet. Every row here must
/// declare `zh: ""`, and a test asserts this list matches reality exactly — so a
/// message added without a translation fails immediately instead of rendering
/// English in a Chinese UI.
pub const UNTRANSLATED: &[MessageId] = &[];

/// Static copy for `locale`, falling back to English when Chinese is missing.
/// Never returns an empty string, a key name, or an unfilled placeholder.
pub fn translate(id: MessageId, locale: Locale) -> &'static str {
    match locale {
        Locale::En => id.english(),
        Locale::ZhCn => id.chinese().unwrap_or(id.english()),
    }
}

/// Map a persisted preference plus the machine's language tag to a supported
/// locale. Pure, so tests cover the mapping without touching the platform.
pub fn resolve(preference: LanguagePreference, system_locale: Option<&str>) -> Locale {
    match preference {
        LanguagePreference::English => Locale::En,
        LanguagePreference::SimplifiedChinese => Locale::ZhCn,
        LanguagePreference::System => system_locale.map_or(Locale::En, resolve_system_locale),
    }
}

/// Simplified Chinese tags (`zh`, `zh-CN`, `zh-Hans`, `zh-SG`) select `ZhCn`.
/// Traditional tags (`zh-Hant`, `zh-TW`, `zh-HK`, `zh-MO`) select English rather
/// than being downgraded to the wrong script; that decision lives here alone, so
/// adding `ZhTw` later is a local change.
fn resolve_system_locale(tag: &str) -> Locale {
    let mut subtags = tag.trim().split(['-', '_']).map(str::to_ascii_lowercase);
    if subtags.next().as_deref() != Some("zh") {
        return Locale::En;
    }
    if subtags.any(|subtag| matches!(subtag.as_str(), "hant" | "tw" | "hk" | "mo")) {
        Locale::En
    } else {
        Locale::ZhCn
    }
}

/// The installed locale and the choice behind it.
pub struct I18nState {
    preference: LanguagePreference,
    resolved: Locale,
}

impl Global for I18nState {}

/// Resolve `preference` against the machine's language and install the global.
/// Call once at boot, after `settings::init` and before the first window.
pub fn init(preference: LanguagePreference, cx: &mut App) {
    let resolved = resolve(preference, system_locale().as_deref());
    tracing::debug!(?preference, ?resolved, "i18n: initial");
    cx.set_global(I18nState {
        preference,
        resolved,
    });
}

/// The machine's language tag. The only platform locale query in the crate.
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

/// The locale every render path should read. English when i18n was never
/// initialized, so tests and tool paths cannot inherit the host language.
pub fn locale(cx: &App) -> Locale {
    cx.try_global::<I18nState>()
        .map(|state| state.resolved)
        .unwrap_or(Locale::En)
}

/// The persisted choice, defaulting to the shipped default.
pub fn preference(cx: &App) -> LanguagePreference {
    cx.try_global::<I18nState>()
        .map(|state| state.preference)
        .unwrap_or_default()
}

/// Switch the running UI to `preference`: store it, persist it, rebuild the menu
/// bar, and repaint every window. Does not restart the engine, rebuild windows,
/// or touch the theme and data profile.
pub fn set_preference(preference: LanguagePreference, cx: &mut App) {
    let resolved = resolve(preference, system_locale().as_deref());
    let unchanged = cx
        .try_global::<I18nState>()
        .is_some_and(|state| state.preference == preference && state.resolved == resolved);
    if unchanged {
        return;
    }
    cx.set_global(I18nState {
        preference,
        resolved,
    });
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.language = preference;
    });
    // Menu titles are native strings built at set time; the boot path sets them
    // after the window exists, and this rebuild keeps the same ordering.
    cx.set_menus(crate::app_menus::app_menus(resolved));
    cx.refresh_windows();
}

/// The translated name of one choice.
pub fn preference_label(preference: LanguagePreference, locale: Locale) -> &'static str {
    translate(
        match preference {
            LanguagePreference::System => MessageId::LanguageSystem,
            LanguagePreference::English => MessageId::LanguageEnglish,
            LanguagePreference::SimplifiedChinese => MessageId::LanguageSimplifiedChinese,
        },
        locale,
    )
}

/// Settings status line: the choice, plus what `System` currently resolves to
/// (shown as that language's own name, e.g. `System · 简体中文`).
pub fn preference_status(
    preference: LanguagePreference,
    resolved: Locale,
    locale: Locale,
) -> String {
    let label = preference_label(preference, locale);
    if preference == LanguagePreference::System {
        format!("{label} · {}", resolved.endonym())
    } else {
        label.to_string()
    }
}

/// Which counter a relative timestamp counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeUnit {
    Minutes,
    Hours,
    Days,
}

/// Fill a relative-time template. The count is formatted, never translated.
pub fn relative_ago(value: u64, unit: RelativeUnit, locale: Locale) -> String {
    let template = translate(
        match unit {
            RelativeUnit::Minutes => MessageId::RelativeMinutesAgo,
            RelativeUnit::Hours => MessageId::RelativeHoursAgo,
            RelativeUnit::Days => MessageId::RelativeDaysAgo,
        },
        locale,
    );
    template.replace("{n}", &value.to_string())
}

/// Fill the short relative-time form the narrow row slots use ("now", "5m").
/// The bucket is chosen by `zeron_proto::view::compact_age`; this only names it,
/// and `zeron_proto::view::format_time_ago` stays the English renderer.
pub fn compact_ago(age: CompactAge, locale: Locale) -> String {
    let (id, count) = match age {
        CompactAge::Now => (MessageId::RelativeCompactNow, None),
        CompactAge::Minutes(n) => (MessageId::RelativeCompactMinutes, Some(n)),
        CompactAge::Hours(n) => (MessageId::RelativeCompactHours, Some(n)),
        CompactAge::Days(n) => (MessageId::RelativeCompactDays, Some(n)),
        CompactAge::Weeks(n) => (MessageId::RelativeCompactWeeks, Some(n)),
        CompactAge::Months(n) => (MessageId::RelativeCompactMonths, Some(n)),
        CompactAge::Years(n) => (MessageId::RelativeCompactYears, Some(n)),
    };
    match count {
        Some(n) => fill(id, "{n}", &n.to_string(), locale),
        None => translate(id, locale).to_string(),
    }
}

/// Fill a single-placeholder template with an untranslated value (a branch name,
/// a file name, a status string).
pub fn fill(id: MessageId, placeholder: &str, value: &str, locale: Locale) -> String {
    translate(id, locale).replace(placeholder, value)
}

/// Fill several named placeholders in one template, for copy that assembles
/// more than one value (counts, theme roles, diagnostics). Values are inserted
/// verbatim and never translated.
pub fn fill_many(id: MessageId, values: &[(&str, &str)], locale: Locale) -> String {
    let mut copy = translate(id, locale).to_string();
    for (placeholder, value) in values {
        copy = copy.replace(*placeholder, value);
    }
    copy
}

/// `1 variant` / `3 variants`. English needs a plural form and Chinese does not,
/// so the count picks between two templates instead of a suffix glued on by the
/// caller — the import card and the theme library both render this.
pub fn variants(count: usize, locale: Locale) -> String {
    counted(
        MessageId::CountVariantOne,
        MessageId::CountVariantMany,
        count,
        locale,
    )
}

/// `1 file` / `3 files`. Shared by every surface that counts files (the diff
/// header strip first), so no caller assembles an English plural itself.
pub fn count_files(count: usize, locale: Locale) -> String {
    counted(
        MessageId::CountFileOne,
        MessageId::CountFileMany,
        count,
        locale,
    )
}

/// `1 commit` / `3 commits`, the same shape as [`count_files`].
pub fn count_commits(count: usize, locale: Locale) -> String {
    counted(
        MessageId::CountCommitOne,
        MessageId::CountCommitMany,
        count,
        locale,
    )
}

/// The one place an English plural is chosen: `count == 1` selects the singular
/// template, everything else the plural one. Chinese rows read the same.
pub fn counted(one: MessageId, many: MessageId, count: usize, locale: Locale) -> String {
    let id = if count == 1 { one } else { many };
    fill(id, "{n}", &count.to_string(), locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_has_english_copy() {
        for id in MessageId::ALL {
            assert!(
                !id.english().trim().is_empty(),
                "{id:?} must have English copy"
            );
        }
    }

    /// The allowlist and the table must agree in both directions: a message that
    /// lacks Chinese copy and is not listed fails, and a listed message that has
    /// been translated fails.
    #[test]
    fn untranslated_allowlist_matches_the_table() {
        let missing: Vec<MessageId> = MessageId::ALL
            .iter()
            .copied()
            .filter(|id| id.chinese().is_none())
            .collect();
        assert_eq!(missing, UNTRANSLATED, "update UNTRANSLATED with the table");
    }

    #[test]
    fn chinese_copy_falls_back_to_english_never_to_nothing() {
        for id in MessageId::ALL.iter().copied() {
            for locale in [Locale::En, Locale::ZhCn] {
                let copy = translate(id, locale);
                assert!(!copy.trim().is_empty(), "{id:?} in {locale:?}");
                assert!(!copy.contains("MessageId"), "{id:?} leaked a key name");
            }
        }
    }

    #[test]
    fn preferences_serialize_stably_and_tolerate_unknown_values() {
        for (preference, json) in [
            (LanguagePreference::System, "\"system\""),
            (LanguagePreference::English, "\"english\""),
            (
                LanguagePreference::SimplifiedChinese,
                "\"simplifiedChinese\"",
            ),
        ] {
            assert_eq!(serde_json::to_string(&preference).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<LanguagePreference>(json).unwrap(),
                preference
            );
        }
        for unknown in ["\"fr\"", "\"zh-CN\"", "\"\"", "null"] {
            let parsed = serde_json::from_str::<LanguagePreference>(unknown)
                .unwrap_or_else(|err| panic!("{unknown} must not break the settings file: {err}"));
            assert_eq!(parsed, LanguagePreference::System, "{unknown}");
        }
    }

    #[test]
    fn system_locale_tags_map_to_simplified_or_english() {
        for tag in ["zh", "zh-CN", "zh-Hans", "zh-Hans-CN", "zh-SG", "zh_CN"] {
            assert_eq!(
                resolve(LanguagePreference::System, Some(tag)),
                Locale::ZhCn,
                "{tag}"
            );
        }
        for tag in ["en-US", "zh-Hant", "zh-TW", "zh-HK", "zh-MO", "ja-JP", "fr"] {
            assert_eq!(
                resolve(LanguagePreference::System, Some(tag)),
                Locale::En,
                "{tag}"
            );
        }
        assert_eq!(resolve(LanguagePreference::System, None), Locale::En);
    }

    #[test]
    fn pinned_preferences_ignore_the_system() {
        for system in [Some("zh-CN"), Some("en-US"), None] {
            assert_eq!(resolve(LanguagePreference::English, system), Locale::En);
            assert_eq!(
                resolve(LanguagePreference::SimplifiedChinese, system),
                Locale::ZhCn
            );
        }
    }

    #[test]
    fn relative_time_fills_the_count_without_translating_it() {
        assert_eq!(relative_ago(5, RelativeUnit::Minutes, Locale::En), "5m ago");
        assert_eq!(
            relative_ago(5, RelativeUnit::Minutes, Locale::ZhCn),
            "5 分钟前"
        );
        assert_eq!(relative_ago(2, RelativeUnit::Days, Locale::ZhCn), "2 天前");
        assert_eq!(translate(MessageId::RelativeJustNow, Locale::ZhCn), "刚刚");
    }

    #[test]
    fn status_line_names_the_resolved_locale_only_for_system() {
        assert_eq!(
            preference_status(LanguagePreference::System, Locale::ZhCn, Locale::En),
            "System · 简体中文"
        );
        assert_eq!(
            preference_status(LanguagePreference::System, Locale::En, Locale::ZhCn),
            "跟随系统 · English"
        );
        assert_eq!(
            preference_status(LanguagePreference::English, Locale::En, Locale::ZhCn),
            "English"
        );
        assert_eq!(
            preference_status(
                LanguagePreference::SimplifiedChinese,
                Locale::ZhCn,
                Locale::En
            ),
            "Simplified Chinese"
        );
    }

    #[test]
    fn default_follows_the_system_language() {
        // The shipped default: a machine set to Simplified Chinese opens in
        // Chinese, and anything else opens in English (see `resolve`).
        assert_eq!(LanguagePreference::default(), LanguagePreference::System);
        assert_eq!(
            resolve(LanguagePreference::default(), Some("zh-CN")),
            Locale::ZhCn
        );
        assert_eq!(
            resolve(LanguagePreference::default(), Some("en-US")),
            Locale::En
        );
        assert_eq!(resolve(LanguagePreference::default(), None), Locale::En);
    }

    /// Every language the Appearance page offers has a label in both locales.
    #[test]
    fn every_language_choice_is_labelled() {
        assert_eq!(LanguagePreference::ALL.len(), 3);
        for preference in LanguagePreference::ALL {
            for locale in [Locale::En, Locale::ZhCn] {
                assert!(!preference_label(preference, locale).is_empty());
            }
            assert!(!preference.id().is_empty());
        }
    }

    #[test]
    fn uninitialized_locale_is_english() {
        let cx = gpui::TestAppContext::single();
        cx.update(|cx| {
            assert_eq!(locale(cx), Locale::En);
            assert_eq!(preference(cx), LanguagePreference::default());
        });
    }

    #[test]
    fn plural_count_picks_an_english_template_and_no_chinese_suffix() {
        assert_eq!(variants(1, Locale::En), "1 variant");
        assert_eq!(variants(2, Locale::En), "2 variants");
        assert_eq!(variants(1, Locale::ZhCn), "1 个变体");
        assert_eq!(variants(3, Locale::ZhCn), "3 个变体");
    }

    #[test]
    fn counted_nouns_pick_one_template_per_number() {
        assert_eq!(count_files(1, Locale::En), "1 file");
        assert_eq!(count_files(2, Locale::En), "2 files");
        assert_eq!(count_files(0, Locale::En), "0 files");
        assert_eq!(count_files(1, Locale::ZhCn), "1 个文件");
        assert_eq!(count_files(3, Locale::ZhCn), "3 个文件");
        assert_eq!(count_commits(1, Locale::En), "1 commit");
        assert_eq!(count_commits(4, Locale::En), "4 commits");
        assert_eq!(count_commits(1, Locale::ZhCn), "1 个提交");
        assert_eq!(count_commits(4, Locale::ZhCn), "4 个提交");
    }

    /// The pin rejections `crates/proto` classifies keep that crate's wording in
    /// English and name the same rejection in Chinese.
    #[test]
    fn pin_rejections_render_proto_english_and_chinese() {
        let rows = [
            (
                zeron_proto::SidebarPinRejection::NotUnique,
                MessageId::SidebarPinsInvalid,
                "侧边栏固定项必须非空且不重复",
            ),
            (
                zeron_proto::SidebarPinRejection::Limit,
                MessageId::SidebarPinsLimit,
                "最多可固定 200 个会话",
            ),
        ];
        for (rejection, id, chinese) in rows {
            assert_eq!(translate(id, Locale::En), rejection.english());
            assert_eq!(translate(id, Locale::ZhCn), chinese);
        }
    }

    #[test]
    fn fill_many_substitutes_every_value_without_translating_it() {
        let copy = fill_many(
            MessageId::ThemeReportWarning,
            &[("{message}", "unknown color key")],
            Locale::ZhCn,
        );
        assert_eq!(copy, "警告 · unknown color key");
        let summary = fill_many(
            MessageId::ThemeReportSummary,
            &[
                ("{mapped}", "3"),
                ("{adjusted}", "1"),
                ("{inferred}", "0"),
                ("{unsupported}", "0"),
                ("{warnings}", "2"),
                ("{validation}", "1"),
            ],
            Locale::En,
        );
        assert!(summary.starts_with("3 mapped · 1 adjusted"));
        assert!(summary.ends_with("2 warnings · 1 validation"));
    }
}
