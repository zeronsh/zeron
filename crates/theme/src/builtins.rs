use std::collections::BTreeMap;
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use crate::{
    AccentRoles, Appearance, Color, SurfaceTreatment, TerminalPalette, ThemeColors, ThemeFamily,
    ThemeRegistry, ThemeSource, ThemeVariant,
};

pub fn builtin_registry() -> &'static ThemeRegistry {
    static REGISTRY: OnceLock<ThemeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| ThemeRegistry {
        families: vec![
            family("zeron", "Zeron", vec![zeron_light(), zeron_dark()]),
            family(
                "vscode-default",
                "VS Code Default",
                vec![vscode_light(), vscode_dark()],
            ),
            family(
                "catppuccin",
                "Catppuccin",
                vec![catppuccin_latte(), catppuccin_mocha()],
            ),
            family(
                "tokyo-night",
                "Tokyo Night",
                vec![tokyo_light(), tokyo_dark()],
            ),
            family("dracula", "Dracula", vec![dracula()]),
            family("github", "GitHub", vec![github_light(), github_dark()]),
            family("vercel", "Vercel", vec![vercel_light(), vercel_dark()]),
            family("claude", "Claude", vec![claude_light(), claude_dark()]),
            family("spotify", "Spotify", vec![spotify_dark()]),
            family("linear", "Linear", vec![linear_light(), linear_dark()]),
            family("ayu", "Ayu", vec![ayu_light(), ayu_dark(), ayu_mirage()]),
            family("gruvbox", "Gruvbox", vec![gruvbox_light(), gruvbox_dark()]),
            family(
                "rose-pine",
                "Rosé Pine",
                vec![rose_pine_dawn(), rose_pine_moon()],
            ),
            family("nord", "Nord", vec![nord()]),
            family("one-dark-pro", "One Dark Pro", vec![one_dark_pro()]),
            family("atom-one-dark", "Atom One Dark", vec![atom_one_dark()]),
            family(
                "night-owl",
                "Night Owl",
                vec![night_owl_light(), night_owl()],
            ),
            family(
                "winter-is-coming",
                "Winter is Coming",
                vec![winter_light(), winter_dark_blue()],
            ),
            family("palenight", "Palenight", vec![palenight()]),
            family("synthwave-84", "SynthWave '84", vec![synthwave_84()]),
            family(
                "shades-of-purple",
                "Shades of Purple",
                vec![shades_of_purple()],
            ),
            family("cobalt2", "Cobalt2", vec![cobalt2()]),
            family("andromeda", "Andromeda", vec![andromeda()]),
        ],
    })
}

fn family(id: &str, name: &str, variants: Vec<ThemeVariant>) -> ThemeFamily {
    ThemeFamily {
        id: id.into(),
        name: name.into(),
        variants,
    }
}

struct Seeds<'a> {
    id: &'a str,
    family_id: &'a str,
    name: &'a str,
    appearance: Appearance,
    treatment: SurfaceTreatment,
    background: &'a str,
    shell: &'a str,
    raised: &'a str,
    card: &'a str,
    text: &'a str,
    muted: &'a str,
    faint: &'a str,
    accent: &'a str,
    danger: &'a str,
    warning: &'a str,
    success: &'a str,
    terminal_background: &'a str,
    ansi: [&'a str; 16],
    syntax: [&'a str; 12],
    source: ThemeSource,
}

fn variant(seed: Seeds<'_>) -> ThemeVariant {
    let background = c(seed.background);
    let shell = c(seed.shell);
    let raised = c(seed.raised);
    let card = c(seed.card);
    let text = c(seed.text);
    let muted = c(seed.muted).ensure_contrast(background, 4.5);
    let faint = c(seed.faint);
    let danger = c(seed.danger);
    let warning = c(seed.warning);
    let success = c(seed.success);
    let accent = AccentRoles::derive(c(seed.accent), seed.appearance, background);
    let dark = seed.appearance.is_dark();
    let border_tone = if dark { Color::WHITE } else { Color::BLACK };
    let solid = if dark {
        Color::rgb(235, 235, 239)
    } else {
        Color::rgb(35, 35, 40)
    };
    let colors = ThemeColors {
        background,
        shell,
        raised,
        card,
        dialog: card.mix(raised, if dark { 0.18 } else { 0.04 }),
        overlay: card.mix(raised, if dark { 0.34 } else { 0.02 }),
        hover: border_tone.with_alpha(if dark { 0.11 } else { 0.06 }),
        active: accent.primary.with_alpha(if dark { 0.18 } else { 0.10 }),
        border: border_tone.with_alpha(if dark { 0.10 } else { 0.12 }),
        border_strong: border_tone.with_alpha(if dark { 0.18 } else { 0.22 }),
        text,
        text_muted: muted,
        text_faint: faint,
        solid,
        on_solid: solid.best_on_color(),
        danger,
        danger_muted: danger.mix(text, 0.28),
        warning,
        warning_muted: warning.mix(text, 0.25),
        success,
        success_muted: success.mix(text, 0.25),
        input: if dark { raised.with_alpha(0.72) } else { card },
        cursor: text.with_alpha(if dark { 0.40 } else { 0.55 }),
        diff_add: success,
        diff_delete: danger,
        diff_hunk: accent.primary.with_alpha(if dark { 0.08 } else { 0.07 }),
    };
    let terminal_background = c(seed.terminal_background);
    let mut variant = ThemeVariant {
        id: seed.id.into(),
        family_id: seed.family_id.into(),
        name: seed.name.into(),
        appearance: seed.appearance,
        recommended_surface_treatment: seed.treatment,
        colors,
        accent,
        syntax: syntax(seed.syntax),
        terminal: TerminalPalette {
            background: terminal_background,
            foreground: text.ensure_contrast(terminal_background, 4.5),
            selection: border_tone.with_alpha(if dark { 0.22 } else { 0.16 }),
            ansi: seed.ansi.map(c),
        },
        source: seed.source,
    };
    // Hash the checked-in resolved definition itself (with the hash field
    // blanked), not merely its source URL. This makes provenance sensitive to
    // curation edits as well as upstream revision changes.
    variant.source.asset_hash.clear();
    let encoded = serde_json::to_vec(&variant).expect("built-in theme serializes");
    variant.source.asset_hash = format!("sha256:{:x}", Sha256::digest(encoded));
    variant
}

fn syntax(colors: [&str; 12]) -> BTreeMap<String, Color> {
    let [
        comment,
        keyword,
        string,
        number,
        type_name,
        function,
        property,
        variable,
        punctuation,
        tag,
        attribute,
        invalid,
    ] = colors.map(c);
    BTreeMap::from([
        ("comment".into(), comment),
        ("keyword".into(), keyword),
        ("string".into(), string),
        ("stringSpecial".into(), attribute),
        ("escape".into(), attribute),
        ("number".into(), number),
        ("boolean".into(), number),
        ("type".into(), type_name),
        ("typeBuiltin".into(), type_name),
        ("constructor".into(), type_name),
        ("function".into(), function),
        ("functionBuiltin".into(), function),
        ("macro".into(), keyword),
        ("property".into(), property),
        ("constant".into(), number),
        ("variable".into(), variable),
        ("variableSpecial".into(), keyword),
        ("parameter".into(), variable),
        ("operator".into(), keyword),
        ("punctuation".into(), punctuation),
        ("tag".into(), tag),
        ("attribute".into(), attribute),
        ("label".into(), function),
        ("embedded".into(), punctuation),
        ("invalid".into(), invalid),
    ])
}

fn source(id: &str, format: &str, url: &str, revision: &str, license: &str) -> ThemeSource {
    ThemeSource {
        format: format.into(),
        url: url.into(),
        revision: revision.into(),
        license: license.into(),
        asset_hash: format!("pending:{id}"),
    }
}

fn c(value: &str) -> Color {
    value.parse().expect("built-in theme colors are valid")
}

const ANSI_DARK: [&str; 16] = [
    "#242424", "#f87171", "#4ade80", "#facc15", "#60a5fa", "#c084fc", "#22d3ee", "#d4d4d8",
    "#52525b", "#fca5a5", "#86efac", "#fde047", "#93c5fd", "#d8b4fe", "#67e8f9", "#fafafa",
];

const ANSI_LIGHT: [&str; 16] = [
    "#1f1f1f", "#dc2626", "#16a34a", "#b45309", "#2563eb", "#9333ea", "#0e7490", "#3f3f46",
    "#71717a", "#b91c1c", "#15803d", "#92400e", "#1d4ed8", "#7e22ce", "#155e75", "#18181b",
];

fn zeron_dark() -> ThemeVariant {
    variant(Seeds {
        id: "zeron-dark",
        family_id: "zeron",
        name: "Zeron Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Frosted,
        background: "#060606",
        shell: "#0d0d0d",
        raised: "#343438",
        card: "#0e0e0e",
        text: "#e8e8ea",
        muted: "#a9a9ae",
        faint: "#85858a",
        accent: "#8b7cf6",
        danger: "#f87171",
        warning: "#facc15",
        success: "#34d399",
        terminal_background: "#090909",
        ansi: ANSI_DARK,
        syntax: [
            "#92929a", "#8b7cf6", "#34d399", "#facc15", "#c084fc", "#60a5fa", "#f472b6", "#e8e8ea",
            "#a1a1aa", "#f472b6", "#22d3ee", "#f87171",
        ],
        source: source(
            "zeron-dark",
            "native",
            "https://github.com/zeronsh/comet",
            "d138049",
            "MIT",
        ),
    })
}

fn zeron_light() -> ThemeVariant {
    variant(Seeds {
        id: "zeron-light",
        family_id: "zeron",
        name: "Zeron Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Frosted,
        background: "#ffffff",
        shell: "#f3f3f5",
        raised: "#ededf0",
        card: "#ffffff",
        text: "#303035",
        muted: "#62626a",
        faint: "#797981",
        accent: "#5b43e8",
        danger: "#dc2626",
        warning: "#a16207",
        success: "#15803d",
        terminal_background: "#fafafa",
        ansi: ANSI_LIGHT,
        syntax: [
            "#6b7280", "#5b43e8", "#15803d", "#a16207", "#7e22ce", "#2563eb", "#be185d", "#303035",
            "#52525b", "#be185d", "#0e7490", "#b91c1c",
        ],
        source: source(
            "zeron-light",
            "native",
            "https://github.com/zeronsh/comet",
            "d138049",
            "MIT",
        ),
    })
}

fn vscode_dark() -> ThemeVariant {
    variant(Seeds {
        id: "vscode-dark-plus",
        family_id: "vscode-default",
        name: "Dark+",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#1e1e1e",
        shell: "#181818",
        raised: "#2a2d2e",
        card: "#252526",
        text: "#d4d4d4",
        muted: "#a8a8a8",
        faint: "#858585",
        accent: "#007acc",
        danger: "#f48771",
        warning: "#cca700",
        success: "#89d185",
        terminal_background: "#1e1e1e",
        ansi: [
            "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
            "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#e5e5e5",
        ],
        syntax: [
            "#6a9955", "#c586c0", "#ce9178", "#b5cea8", "#4ec9b0", "#dcdcaa", "#9cdcfe", "#d4d4d4",
            "#d4d4d4", "#569cd6", "#9cdcfe", "#f44747",
        ],
        source: source(
            "vscode-dark-plus",
            "vscode",
            "https://github.com/microsoft/vscode",
            "e33d147d4c0fa65ce17cb73ec9d798f064b4bf1f",
            "MIT",
        ),
    })
}

fn vscode_light() -> ThemeVariant {
    variant(Seeds {
        id: "vscode-light-plus",
        family_id: "vscode-default",
        name: "Light+",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#f3f3f3",
        raised: "#e8e8e8",
        card: "#ffffff",
        text: "#1f1f1f",
        muted: "#616161",
        faint: "#767676",
        accent: "#0078d4",
        danger: "#a1260d",
        warning: "#895503",
        success: "#008000",
        terminal_background: "#ffffff",
        ansi: ANSI_LIGHT,
        syntax: [
            "#008000", "#0000ff", "#a31515", "#098658", "#267f99", "#795e26", "#001080", "#1f1f1f",
            "#1f1f1f", "#800000", "#ff0000", "#cd3131",
        ],
        source: source(
            "vscode-light-plus",
            "vscode",
            "https://github.com/microsoft/vscode",
            "e33d147d4c0fa65ce17cb73ec9d798f064b4bf1f",
            "MIT",
        ),
    })
}

fn catppuccin_mocha() -> ThemeVariant {
    variant(Seeds {
        id: "catppuccin-mocha",
        family_id: "catppuccin",
        name: "Catppuccin Mocha",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#1e1e2e",
        shell: "#181825",
        raised: "#313244",
        card: "#242436",
        text: "#cdd6f4",
        muted: "#a6adc8",
        faint: "#9399b2",
        accent: "#cba6f7",
        danger: "#f38ba8",
        warning: "#f9e2af",
        success: "#a6e3a1",
        terminal_background: "#1e1e2e",
        ansi: [
            "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
            "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
        ],
        syntax: [
            "#6c7086", "#cba6f7", "#a6e3a1", "#fab387", "#f9e2af", "#89b4fa", "#94e2d5", "#cdd6f4",
            "#bac2de", "#f38ba8", "#f5c2e7", "#f38ba8",
        ],
        source: source(
            "catppuccin-mocha",
            "vscode",
            "https://github.com/catppuccin/vscode",
            "befc9e6fc41980f4241408f7049755d47c06ff45",
            "MIT",
        ),
    })
}

fn catppuccin_latte() -> ThemeVariant {
    variant(Seeds {
        id: "catppuccin-latte",
        family_id: "catppuccin",
        name: "Catppuccin Latte",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#eff1f5",
        shell: "#e6e9ef",
        raised: "#dce0e8",
        card: "#f7f8fa",
        text: "#4c4f69",
        muted: "#5c5f77",
        faint: "#6c6f85",
        accent: "#8839ef",
        danger: "#d20f39",
        warning: "#df8e1d",
        success: "#40a02b",
        terminal_background: "#eff1f5",
        ansi: [
            "#5c5f77", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#acb0be",
            "#6c6f85", "#d20f39", "#40a02b", "#df8e1d", "#1e66f5", "#ea76cb", "#179299", "#bcc0cc",
        ],
        syntax: [
            "#8c8fa1", "#8839ef", "#40a02b", "#fe640b", "#df8e1d", "#1e66f5", "#179299", "#4c4f69",
            "#6c6f85", "#d20f39", "#ea76cb", "#d20f39",
        ],
        source: source(
            "catppuccin-latte",
            "vscode",
            "https://github.com/catppuccin/vscode",
            "befc9e6fc41980f4241408f7049755d47c06ff45",
            "MIT",
        ),
    })
}

fn tokyo_dark() -> ThemeVariant {
    variant(Seeds {
        id: "tokyo-night",
        family_id: "tokyo-night",
        name: "Tokyo Night",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#1a1b26",
        shell: "#16161e",
        raised: "#292e42",
        card: "#1f2335",
        text: "#c0caf5",
        muted: "#9aa5ce",
        faint: "#7f87ad",
        accent: "#7aa2f7",
        danger: "#f7768e",
        warning: "#e0af68",
        success: "#9ece6a",
        terminal_background: "#1a1b26",
        ansi: [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ],
        syntax: [
            "#565f89", "#bb9af7", "#9ece6a", "#ff9e64", "#2ac3de", "#7aa2f7", "#73daca", "#c0caf5",
            "#9abdf5", "#f7768e", "#e0af68", "#f7768e",
        ],
        source: source(
            "tokyo-night",
            "vscode",
            "https://github.com/tokyo-night/tokyo-night-vscode-theme",
            "7c0f11eaef322f293621ca7befe462214b7ea468",
            "MIT",
        ),
    })
}

fn tokyo_light() -> ThemeVariant {
    variant(Seeds {
        id: "tokyo-night-light",
        family_id: "tokyo-night",
        name: "Tokyo Night Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#d5d6db",
        shell: "#cbccd1",
        raised: "#bfc0c6",
        card: "#e1e2e7",
        text: "#343b59",
        muted: "#485e7d",
        faint: "#5a6378",
        accent: "#34548a",
        danger: "#8c4351",
        warning: "#8f5e15",
        success: "#33635c",
        terminal_background: "#d5d6db",
        ansi: [
            "#0f0f14", "#8c4351", "#33635c", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#828594",
            "#4c505e", "#8c4351", "#33635c", "#8f5e15", "#34548a", "#5a4a78", "#0f4b6e", "#343b59",
        ],
        syntax: [
            "#6c6e75", "#5a4a78", "#33635c", "#965027", "#0f4b6e", "#34548a", "#166775", "#343b59",
            "#485e7d", "#8c4351", "#8f5e15", "#8c4351",
        ],
        source: source(
            "tokyo-night-light",
            "vscode",
            "https://github.com/tokyo-night/tokyo-night-vscode-theme",
            "7c0f11eaef322f293621ca7befe462214b7ea468",
            "MIT",
        ),
    })
}

fn dracula() -> ThemeVariant {
    variant(Seeds {
        id: "dracula",
        family_id: "dracula",
        name: "Dracula",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282a36",
        shell: "#21222c",
        raised: "#44475a",
        card: "#2d303e",
        text: "#f8f8f2",
        muted: "#c5c8c6",
        faint: "#9a9cab",
        accent: "#bd93f9",
        danger: "#ff5555",
        warning: "#f1fa8c",
        success: "#50fa7b",
        terminal_background: "#282a36",
        ansi: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
        syntax: [
            "#6272a4", "#ff79c6", "#f1fa8c", "#bd93f9", "#8be9fd", "#50fa7b", "#8be9fd", "#f8f8f2",
            "#f8f8f2", "#ff79c6", "#50fa7b", "#ff5555",
        ],
        source: source(
            "dracula",
            "vscode",
            "https://github.com/dracula/visual-studio-code",
            "1b9ecf4d7e0c8cc2e2e890a7a41ad1db5fff1e6c",
            "MIT",
        ),
    })
}

fn github_light() -> ThemeVariant {
    variant(Seeds {
        id: "github-light",
        family_id: "github",
        name: "GitHub Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#f6f8fa",
        raised: "#d0d7de",
        card: "#ffffff",
        text: "#1f2328",
        muted: "#656d76",
        faint: "#818b98",
        accent: "#0969da",
        danger: "#cf222e",
        warning: "#9a6700",
        success: "#1a7f37",
        terminal_background: "#ffffff",
        ansi: [
            "#24292f", "#cf222e", "#1a7f37", "#9a6700", "#0969da", "#8250df", "#1b7c83", "#6e7781",
            "#57606a", "#a40e26", "#116329", "#7d4e00", "#0550ae", "#6639ba", "#0a6b74", "#1f2328",
        ],
        syntax: [
            "#6e7781", "#cf222e", "#0a3069", "#0550ae", "#953800", "#8250df", "#116329", "#1f2328",
            "#57606a", "#116329", "#953800", "#cf222e",
        ],
        source: source(
            "github-light",
            "vscode",
            "https://github.com/primer/github-vscode-theme",
            "cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf",
            "MIT",
        ),
    })
}

fn github_dark() -> ThemeVariant {
    variant(Seeds {
        id: "github-dark",
        family_id: "github",
        name: "GitHub Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#010409",
        shell: "#000204",
        raised: "#161b22",
        card: "#0d1117",
        text: "#e6edf3",
        muted: "#8b949e",
        faint: "#6e7681",
        accent: "#2f81f7",
        danger: "#f85149",
        warning: "#d29922",
        success: "#3fb950",
        terminal_background: "#010409",
        ansi: [
            "#484f58", "#ff7b72", "#3fb950", "#d29922", "#58a6ff", "#bc8cff", "#39c5cf", "#b1bac4",
            "#6e7681", "#ffa198", "#56d364", "#e3b341", "#79c0ff", "#d2a8ff", "#56d4dd", "#f0f6fc",
        ],
        syntax: [
            "#8b949e", "#ff7b72", "#a5d6ff", "#79c0ff", "#ffa657", "#d2a8ff", "#7ee787", "#e6edf3",
            "#b1bac4", "#7ee787", "#ffa657", "#f85149",
        ],
        source: source(
            "github-dark",
            "vscode",
            "https://github.com/primer/github-vscode-theme",
            "cd78e5e4e7bcf132a6f428ae0f32264bb1b729cf",
            "MIT",
        ),
    })
}

fn vercel_light() -> ThemeVariant {
    variant(Seeds {
        id: "vercel-light",
        family_id: "vercel",
        name: "Vercel Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#fafafa",
        raised: "#f2f2f2",
        card: "#ffffff",
        text: "#171717",
        muted: "#666666",
        faint: "#737373",
        accent: "#171717",
        danger: "#c52b30",
        warning: "#a35200",
        success: "#18794e",
        terminal_background: "#ffffff",
        ansi: ANSI_LIGHT,
        syntax: [
            "#6e6e6e", "#d1242f", "#067a6e", "#0550ae", "#9a6700", "#6e40c9", "#0550ae", "#171717",
            "#666666", "#067a6e", "#9a6700", "#c52b30",
        ],
        source: source(
            "vercel-light",
            "native",
            "https://vercel.com/geist/colors",
            "zeron-curation-v1",
            "MIT",
        ),
    })
}

fn vercel_dark() -> ThemeVariant {
    variant(Seeds {
        id: "vercel-dark",
        family_id: "vercel",
        name: "Vercel Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#000000",
        shell: "#0a0a0a",
        raised: "#1a1a1a",
        card: "#111111",
        text: "#ededed",
        muted: "#bdbdbd",
        faint: "#808080",
        accent: "#ededed",
        danger: "#ff6166",
        warning: "#ffb224",
        success: "#46c496",
        terminal_background: "#000000",
        ansi: ANSI_DARK,
        syntax: [
            "#8f8f8f", "#f97583", "#62c9b8", "#79b8ff", "#ffb224", "#b392f0", "#79b8ff", "#ededed",
            "#a1a1a1", "#62c9b8", "#ffb224", "#ff6166",
        ],
        source: source(
            "vercel-dark",
            "native",
            "https://vercel.com/geist/colors",
            "zeron-curation-v1",
            "MIT",
        ),
    })
}

fn claude_light() -> ThemeVariant {
    variant(Seeds {
        id: "claude-light",
        family_id: "claude",
        name: "Claude Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#faf9f5",
        shell: "#f5f4ee",
        raised: "#e9e6dc",
        card: "#f5f4ef",
        text: "#3d3929",
        muted: "#6e6d68",
        faint: "#817e72",
        accent: "#c96442",
        danger: "#b33f3b",
        warning: "#956522",
        success: "#40734b",
        terminal_background: "#faf9f5",
        ansi: ANSI_LIGHT,
        syntax: [
            "#77746b", "#a64b32", "#40734b", "#8b5a26", "#806724", "#7756a3", "#486b87", "#3d3929",
            "#6e6d68", "#a64b32", "#806724", "#b33f3b",
        ],
        source: source(
            "claude-light",
            "native",
            "https://tweakcn.com/themes/cmdght103000n04lh3e2ae93r",
            "cmdght103000n04lh3e2ae93r@2025-07-27T22:39:29.615Z",
            "MIT",
        ),
    })
}

fn claude_dark() -> ThemeVariant {
    variant(Seeds {
        id: "claude-dark",
        family_id: "claude",
        name: "Claude Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#262624",
        shell: "#1f1e1d",
        raised: "#30302e",
        card: "#2c2c2b",
        text: "#f1f1ef",
        muted: "#b7b5a9",
        faint: "#969489",
        accent: "#d97757",
        danger: "#f07872",
        warning: "#deb36e",
        success: "#a5bf88",
        terminal_background: "#262624",
        ansi: [
            "#1b1b19", "#f07872", "#a5bf88", "#deb36e", "#96b9d0", "#c4ace0", "#8ac5bd", "#c3c0b6",
            "#817e72", "#f5a39b", "#bfd5a7", "#ead0a0", "#b2cfe0", "#d8c5ea", "#addbd4", "#f1f1ef",
        ],
        syntax: [
            "#969489", "#d97757", "#a5bf88", "#deb36e", "#d6c29b", "#c4ace0", "#96b9d0", "#f1f1ef",
            "#b7b5a9", "#d97757", "#8ac5bd", "#f07872",
        ],
        source: source(
            "claude-dark",
            "native",
            "https://tweakcn.com/themes/cmdght103000n04lh3e2ae93r",
            "cmdght103000n04lh3e2ae93r@2025-07-27T22:39:29.615Z",
            "MIT",
        ),
    })
}

fn spotify_dark() -> ThemeVariant {
    variant(Seeds {
        id: "spotify-dark",
        family_id: "spotify",
        name: "Spotify Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#121212",
        shell: "#000000",
        raised: "#282828",
        card: "#181818",
        text: "#ffffff",
        muted: "#b3b3b3",
        faint: "#8c8c8c",
        accent: "#1ed760",
        danger: "#f3727f",
        warning: "#f6c864",
        success: "#1ed760",
        terminal_background: "#121212",
        ansi: [
            "#181818", "#f3727f", "#1ed760", "#f6c864", "#76baff", "#c9a3ed", "#67d4cd", "#b3b3b3",
            "#727272", "#ff9ca5", "#76e69b", "#ffe08a", "#a4d2ff", "#dfc4ff", "#97ece6", "#ffffff",
        ],
        syntax: [
            "#8c8c8c", "#c9a3ed", "#76e69b", "#f6c864", "#67d4cd", "#76baff", "#1ed760", "#ffffff",
            "#b3b3b3", "#1ed760", "#f6c864", "#f3727f",
        ],
        source: source(
            "spotify-dark",
            "native",
            "https://developer.spotify.com/documentation/design",
            "zeron-curation-v1",
            "MIT",
        ),
    })
}

fn linear_light() -> ThemeVariant {
    variant(Seeds {
        id: "linear-light",
        family_id: "linear",
        name: "Linear Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fafafa",
        shell: "#f3f3f5",
        raised: "#ebebef",
        card: "#ffffff",
        text: "#23252a",
        muted: "#626570",
        faint: "#767985",
        accent: "#5e6ad2",
        danger: "#c4404f",
        warning: "#956800",
        success: "#27815b",
        terminal_background: "#fafafa",
        ansi: ANSI_LIGHT,
        syntax: [
            "#737680", "#6658b8", "#27815b", "#a05a2c", "#906b18", "#4169b2", "#a94770", "#23252a",
            "#626570", "#6658b8", "#906b18", "#c4404f",
        ],
        source: source(
            "linear-light",
            "native",
            "https://linear.app",
            "zeron-curation-v1",
            "MIT",
        ),
    })
}

fn linear_dark() -> ThemeVariant {
    variant(Seeds {
        id: "linear-dark",
        family_id: "linear",
        name: "Linear Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#101114",
        shell: "#0c0d10",
        raised: "#24252b",
        card: "#191a1f",
        text: "#eeeff4",
        muted: "#a0a3b1",
        faint: "#7d8191",
        accent: "#8b8ff5",
        danger: "#ee7184",
        warning: "#e8bd70",
        success: "#6ac59b",
        terminal_background: "#101114",
        ansi: [
            "#191a1f", "#ee7184", "#6ac59b", "#e8bd70", "#82b4f3", "#b7a0ed", "#79c9d5", "#c5c7d2",
            "#727687", "#f69aaa", "#92d8b5", "#f2d299", "#a6ccfa", "#cebbf6", "#a0dee6", "#eeeff4",
        ],
        syntax: [
            "#7d8191", "#b7a0ed", "#6ac59b", "#e8bd70", "#79c9d5", "#8b8ff5", "#82b4f3", "#eeeff4",
            "#a0a3b1", "#b7a0ed", "#e8bd70", "#ee7184",
        ],
        source: source(
            "linear-dark",
            "native",
            "https://linear.app",
            "zeron-curation-v1",
            "MIT",
        ),
    })
}

fn ayu_light() -> ThemeVariant {
    variant(Seeds {
        id: "ayu-light",
        family_id: "ayu",
        name: "Ayu Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fcfcfc",
        shell: "#f8f9fa",
        raised: "#e6e9ed",
        card: "#fafafa",
        text: "#5c6166",
        muted: "#828e9f",
        faint: "#969da8",
        accent: "#f29718",
        danger: "#e65050",
        warning: "#eba400",
        success: "#6cbf43",
        terminal_background: "#f8f9fa",
        ansi: [
            "#000000", "#f06b6c", "#6cbf43", "#e7a100", "#21a1e2", "#a176cb", "#4abc96", "#c7c7c7",
            "#686868", "#f07171", "#86b300", "#eba400", "#22a4e6", "#a37acc", "#4cbf99", "#d1d1d1",
        ],
        syntax: [
            "#adaeb1", "#fa8532", "#86b300", "#a37acc", "#55b4d4", "#f29718", "#f07171", "#5c6166",
            "#828e9f", "#55b4d4", "#e59645", "#e65050",
        ],
        source: source(
            "ayu-light",
            "vscode",
            "https://github.com/ayu-theme/vscode-ayu",
            "444ef92911cb75c3933c8003e3a7c79b6b6c914f",
            "MIT",
        ),
    })
}

fn ayu_dark() -> ThemeVariant {
    variant(Seeds {
        id: "ayu-dark",
        family_id: "ayu",
        name: "Ayu Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#10141c",
        shell: "#0d1017",
        raised: "#1b1f29",
        card: "#141821",
        text: "#bfbdb6",
        muted: "#8a9199",
        faint: "#5a6673",
        accent: "#e6b450",
        danger: "#d95757",
        warning: "#ffb454",
        success: "#70bf56",
        terminal_background: "#0d1017",
        ansi: [
            "#1b1f29", "#f06b73", "#70bf56", "#fdb04c", "#4fbfff", "#d0a1ff", "#93e2c8", "#c7c7c7",
            "#686868", "#f07178", "#aad94c", "#ffb454", "#59c2ff", "#d2a6ff", "#95e6cb", "#ffffff",
        ],
        syntax: [
            "#5a6673", "#ff8f40", "#aad94c", "#d2a6ff", "#39bae6", "#ffb454", "#f07178", "#bfbdb6",
            "#8a9199", "#39bae6", "#e6c08a", "#d95757",
        ],
        source: source(
            "ayu-dark",
            "vscode",
            "https://github.com/ayu-theme/vscode-ayu",
            "444ef92911cb75c3933c8003e3a7c79b6b6c914f",
            "MIT",
        ),
    })
}

fn ayu_mirage() -> ThemeVariant {
    variant(Seeds {
        id: "ayu-mirage",
        family_id: "ayu",
        name: "Ayu Mirage",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#242936",
        shell: "#1f2430",
        raised: "#343d4d",
        card: "#282e3b",
        text: "#cccac2",
        muted: "#8a94a6",
        faint: "#6e7c8f",
        accent: "#ffcc66",
        danger: "#ff6666",
        warning: "#ffcc66",
        success: "#87d96c",
        terminal_background: "#1f2430",
        ansi: [
            "#171b24", "#f28273", "#87d96c", "#fcca60", "#6acdff", "#ddbbff", "#93e2c8", "#c7c7c7",
            "#686868", "#f28779", "#d5ff80", "#ffcd66", "#73d0ff", "#dfbfff", "#95e6cb", "#ffffff",
        ],
        syntax: [
            "#6e7c8f", "#ffa659", "#d5ff80", "#dfbfff", "#5ccfe6", "#ffcd66", "#f28779", "#cccac2",
            "#8a94a6", "#5ccfe6", "#d9be98", "#ff6666",
        ],
        source: source(
            "ayu-mirage",
            "vscode",
            "https://github.com/ayu-theme/vscode-ayu",
            "444ef92911cb75c3933c8003e3a7c79b6b6c914f",
            "MIT",
        ),
    })
}

fn gruvbox_light() -> ThemeVariant {
    variant(Seeds {
        id: "gruvbox-light",
        family_id: "gruvbox",
        name: "Gruvbox Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fbf1c7",
        shell: "#f2e5bc",
        raised: "#d5c4a1",
        card: "#f9f5d7",
        text: "#3c3836",
        muted: "#665c54",
        faint: "#7c6f64",
        accent: "#076678",
        danger: "#9d0006",
        warning: "#b57614",
        success: "#79740e",
        terminal_background: "#fbf1c7",
        ansi: [
            "#ebdbb2", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#7c6f64",
            "#928374", "#9d0006", "#79740e", "#b57614", "#076678", "#8f3f71", "#427b58", "#3c3836",
        ],
        syntax: [
            "#928374", "#9d0006", "#79740e", "#8f3f71", "#b57614", "#076678", "#427b58", "#3c3836",
            "#665c54", "#9d0006", "#af3a03", "#cc241d",
        ],
        source: source(
            "gruvbox-light",
            "vscode",
            "https://github.com/jdinhify/vscode-theme-gruvbox",
            "ca3b8ad203e84a884ca33fb84b5795cf43032709",
            "MIT",
        ),
    })
}

fn gruvbox_dark() -> ThemeVariant {
    variant(Seeds {
        id: "gruvbox-dark",
        family_id: "gruvbox",
        name: "Gruvbox Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282828",
        shell: "#1d2021",
        raised: "#504945",
        card: "#32302f",
        text: "#ebdbb2",
        muted: "#bdae93",
        faint: "#928374",
        accent: "#8ec07c",
        danger: "#fb4934",
        warning: "#fabd2f",
        success: "#b8bb26",
        terminal_background: "#282828",
        ansi: [
            "#3c3836", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a", "#a89984",
            "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b", "#8ec07c", "#ebdbb2",
        ],
        syntax: [
            "#928374", "#fb4934", "#b8bb26", "#d3869b", "#fabd2f", "#83a598", "#8ec07c", "#ebdbb2",
            "#bdae93", "#fb4934", "#fe8019", "#fb4934",
        ],
        source: source(
            "gruvbox-dark",
            "vscode",
            "https://github.com/jdinhify/vscode-theme-gruvbox",
            "ca3b8ad203e84a884ca33fb84b5795cf43032709",
            "MIT",
        ),
    })
}

fn rose_pine_dawn() -> ThemeVariant {
    variant(Seeds {
        id: "rose-pine-dawn",
        family_id: "rose-pine",
        name: "Rosé Pine Dawn",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#faf4ed",
        shell: "#f2e9e1",
        raised: "#dfdad9",
        card: "#fffaf3",
        text: "#575279",
        muted: "#797593",
        faint: "#9893a5",
        accent: "#907aa9",
        danger: "#b4637a",
        warning: "#ea9d34",
        success: "#56949f",
        terminal_background: "#faf4ed",
        ansi: [
            "#f2e9e1", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
            "#797593", "#b4637a", "#286983", "#ea9d34", "#56949f", "#907aa9", "#d7827e", "#575279",
        ],
        syntax: [
            "#9893a5", "#286983", "#ea9d34", "#d7827e", "#56949f", "#b4637a", "#56949f", "#d7827e",
            "#797593", "#56949f", "#907aa9", "#b4637a",
        ],
        source: source(
            "rose-pine-dawn",
            "vscode",
            "https://github.com/rose-pine/vscode",
            "d8f5ebe8e096fa833e997c07eb7685ee1677a4ba",
            "MIT",
        ),
    })
}

fn rose_pine_moon() -> ThemeVariant {
    variant(Seeds {
        id: "rose-pine-moon",
        family_id: "rose-pine",
        name: "Rosé Pine Moon",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#232136",
        shell: "#1f1d2e",
        raised: "#393552",
        card: "#2a273f",
        text: "#e0def4",
        muted: "#908caa",
        faint: "#6e6a86",
        accent: "#c4a7e7",
        danger: "#eb6f92",
        warning: "#f6c177",
        success: "#9ccfd8",
        terminal_background: "#232136",
        ansi: [
            "#393552", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
            "#908caa", "#eb6f92", "#3e8fb0", "#f6c177", "#9ccfd8", "#c4a7e7", "#ea9a97", "#e0def4",
        ],
        syntax: [
            "#6e6a86", "#3e8fb0", "#f6c177", "#ea9a97", "#9ccfd8", "#eb6f92", "#9ccfd8", "#ea9a97",
            "#908caa", "#9ccfd8", "#c4a7e7", "#eb6f92",
        ],
        source: source(
            "rose-pine-moon",
            "vscode",
            "https://github.com/rose-pine/vscode",
            "d8f5ebe8e096fa833e997c07eb7685ee1677a4ba",
            "MIT",
        ),
    })
}

fn nord() -> ThemeVariant {
    variant(Seeds {
        id: "nord",
        family_id: "nord",
        name: "Nord",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#2e3440",
        shell: "#292e39",
        raised: "#434c5e",
        card: "#3b4252",
        text: "#d8dee9",
        muted: "#aeb6c4",
        faint: "#7f899b",
        accent: "#88c0d0",
        danger: "#bf616a",
        warning: "#ebcb8b",
        success: "#a3be8c",
        terminal_background: "#2e3440",
        ansi: [
            "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0",
            "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4",
        ],
        syntax: [
            "#616e88", "#81a1c1", "#a3be8c", "#b48ead", "#8fbcbb", "#88c0d0", "#d08770", "#d8dee9",
            "#aeb6c4", "#81a1c1", "#8fbcbb", "#bf616a",
        ],
        source: source(
            "nord",
            "vscode",
            "https://github.com/nordtheme/visual-studio-code",
            "8ead09822c02d0d49d0f764104505e5a34d3689f",
            "MIT",
        ),
    })
}

fn one_dark_pro() -> ThemeVariant {
    variant(Seeds {
        id: "one-dark-pro",
        family_id: "one-dark-pro",
        name: "One Dark Pro",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282c34",
        shell: "#21252b",
        raised: "#2c313a",
        card: "#21252b",
        text: "#abb2bf",
        muted: "#9da5b4",
        faint: "#7f848e",
        accent: "#528bff",
        danger: "#e05561",
        warning: "#d19a66",
        success: "#8cc265",
        terminal_background: "#282c34",
        ansi: [
            "#3f4451", "#e05561", "#8cc265", "#d18f52", "#4aa5f0", "#c162de", "#42b3c2", "#d7dae0",
            "#4f5666", "#ff616e", "#a5e075", "#f0a45d", "#4dc4ff", "#de73ff", "#4cd1e0", "#e6e6e6",
        ],
        syntax: [
            "#7f848e", "#c678dd", "#e06c75", "#c678dd", "#e5c07b", "#e5c07b", "#d19a66", "#abb2bf",
            "#abb2bf", "#c678dd", "#56b6c2", "#e05561",
        ],
        source: source(
            "one-dark-pro",
            "vscode",
            "https://github.com/Binaryify/OneDark-Pro",
            "e6ccf638d5b69aa38cd1005edb0ee7ba7ef6fedc",
            "MIT",
        ),
    })
}

fn atom_one_dark() -> ThemeVariant {
    variant(Seeds {
        id: "atom-one-dark",
        family_id: "atom-one-dark",
        name: "Atom One Dark",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#282c34",
        shell: "#21252b",
        raised: "#2c313a",
        card: "#21252b",
        text: "#abb2bf",
        muted: "#9da5b4",
        faint: "#5c6370",
        accent: "#528bff",
        danger: "#e06c75",
        warning: "#d19a66",
        success: "#98c379",
        terminal_background: "#282c34",
        ansi: [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#abb2bf",
            "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ],
        syntax: [
            "#5c6370", "#c678dd", "#98c379", "#d19a66", "#e5c07b", "#61afef", "#56b6c2", "#e06c75",
            "#abb2bf", "#e06c75", "#61afef", "#e06c75",
        ],
        source: source(
            "atom-one-dark",
            "vscode",
            "https://github.com/akamud/vscode-theme-onedark",
            "a8be970644982221f9b61fb1c4b3da74b4beab79",
            "MIT",
        ),
    })
}

fn night_owl() -> ThemeVariant {
    variant(Seeds {
        id: "night-owl",
        family_id: "night-owl",
        name: "Night Owl",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#011627",
        shell: "#01111d",
        raised: "#0b253a",
        card: "#071d2e",
        text: "#d6deeb",
        muted: "#8ca6ba",
        faint: "#637777",
        accent: "#82aaff",
        danger: "#ef5350",
        warning: "#ffeb95",
        success: "#22da6e",
        terminal_background: "#011627",
        ansi: [
            "#011627", "#ef5350", "#22da6e", "#c5e478", "#82aaff", "#c792ea", "#21c7a8", "#ffffff",
            "#575656", "#ef5350", "#22da6e", "#ffeb95", "#82aaff", "#c792ea", "#7fdbca", "#ffffff",
        ],
        syntax: [
            "#637777", "#82aaff", "#c5e478", "#f78c6c", "#ffcb8b", "#c5e478", "#bec5d4", "#d7dbe0",
            "#82aaff", "#7fdbca", "#f78c6c", "#ef5350",
        ],
        source: source(
            "night-owl",
            "vscode",
            "https://github.com/sdras/night-owl-vscode-theme",
            "cc291eba7976b20d7c66bde6883c27b902196b07",
            "MIT",
        ),
    })
}

fn night_owl_light() -> ThemeVariant {
    variant(Seeds {
        id: "night-owl-light",
        family_id: "night-owl",
        name: "Night Owl Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#fbfbfb",
        shell: "#f0f0f0",
        raised: "#d3e8f8",
        card: "#ffffff",
        text: "#403f53",
        muted: "#66657a",
        faint: "#7a8297",
        accent: "#4876d6",
        danger: "#de3d3b",
        warning: "#a67d00",
        success: "#08916a",
        terminal_background: "#f6f6f6",
        ansi: [
            "#403f53", "#de3d3b", "#08916a", "#e0af02", "#288ed7", "#d6438a", "#2aa298", "#93a1a1",
            "#403f53", "#de3d3b", "#08916a", "#daaa01", "#288ed7", "#d6438a", "#2aa298", "#403f53",
        ],
        syntax: [
            "#6f7896", "#4876d6", "#0c8f75", "#aa0982", "#111111", "#4876d6", "#111111", "#403f53",
            "#4876d6", "#111111", "#aa0982", "#de3d3b",
        ],
        source: source(
            "night-owl-light",
            "vscode",
            "https://github.com/sdras/night-owl-vscode-theme",
            "cc291eba7976b20d7c66bde6883c27b902196b07",
            "MIT",
        ),
    })
}

fn winter_dark_blue() -> ThemeVariant {
    variant(Seeds {
        id: "winter-dark-blue",
        family_id: "winter-is-coming",
        name: "Winter is Coming Dark Blue",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#011627",
        shell: "#01111d",
        raised: "#0b253a",
        card: "#071d2e",
        text: "#d6deeb",
        muted: "#8ca6ba",
        faint: "#5f7e97",
        accent: "#219fd5",
        danger: "#ef5350",
        warning: "#ffca28",
        success: "#22da6e",
        terminal_background: "#011627",
        ansi: [
            "#011627", "#ef5350", "#22da6e", "#f7ecb5", "#219fd5", "#c792ea", "#80cbc4", "#d6deeb",
            "#5f7e97", "#ef9a9a", "#addb67", "#ffcb6b", "#82aaff", "#c792ea", "#7fdbca", "#ffffff",
        ],
        syntax: [
            "#8095a8", "#c792ea", "#78bd65", "#8dec95", "#d29ffc", "#87aff4", "#7fdbca", "#cbcdd2",
            "#4fb4d8", "#6dbdfa", "#f7ecb5", "#ef5350",
        ],
        source: source(
            "winter-dark-blue",
            "vscode",
            "https://github.com/johnpapa/vscode-winteriscoming",
            "260547834cb6ac37dd5b8bb5842cc1c8d3164946",
            "MIT",
        ),
    })
}

fn winter_light() -> ThemeVariant {
    variant(Seeds {
        id: "winter-light",
        family_id: "winter-is-coming",
        name: "Winter is Coming Light",
        appearance: Appearance::Light,
        treatment: SurfaceTreatment::Opaque,
        background: "#ffffff",
        shell: "#f3f3f3",
        raised: "#e5e5e5",
        card: "#ffffff",
        text: "#1857a4",
        muted: "#62626a",
        faint: "#66768a",
        accent: "#2f86d2",
        danger: "#de3d3b",
        warning: "#9b6a00",
        success: "#08916a",
        terminal_background: "#ffffff",
        ansi: [
            "#011627", "#de3d3b", "#08916a", "#c5a332", "#236ebf", "#7c4dff", "#0097a7", "#7e8993",
            "#403f53", "#f76c6c", "#49d0b0", "#e7c244", "#4e8cdf", "#b15a91", "#00bcd4", "#403f53",
        ],
        syntax: [
            "#357b42", "#7942a8", "#87429a", "#174781", "#0444ac", "#b1108e", "#207b76", "#224555",
            "#4f7aa0", "#0444ac", "#b46a0f", "#c73532",
        ],
        source: source(
            "winter-light",
            "vscode",
            "https://github.com/johnpapa/vscode-winteriscoming",
            "260547834cb6ac37dd5b8bb5842cc1c8d3164946",
            "MIT",
        ),
    })
}

fn palenight() -> ThemeVariant {
    variant(Seeds {
        id: "palenight",
        family_id: "palenight",
        name: "Palenight",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#292d3e",
        shell: "#232635",
        raised: "#34384c",
        card: "#292d3e",
        text: "#eeffff",
        muted: "#a6accd",
        faint: "#697098",
        accent: "#82aaff",
        danger: "#ff5572",
        warning: "#ffcb6b",
        success: "#c3e88d",
        terminal_background: "#292d3e",
        ansi: [
            "#676e95", "#ff5572", "#a9c77d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
            "#676e95", "#ff5572", "#c3e88d", "#ffcb6b", "#82aaff", "#c792ea", "#89ddff", "#ffffff",
        ],
        syntax: [
            "#697098", "#c792ea", "#c3e88d", "#f78c6c", "#eeffff", "#ffcb6b", "#89ddff", "#bfc7d5",
            "#82aaff", "#89ddff", "#7986e7", "#ff5572",
        ],
        source: source(
            "palenight",
            "vscode",
            "https://github.com/whizkydee/vscode-palenight-theme",
            "6291efaace90855abe3d79025327ca41b9a3138c",
            "MIT",
        ),
    })
}

fn synthwave_84() -> ThemeVariant {
    variant(Seeds {
        id: "synthwave-84",
        family_id: "synthwave-84",
        name: "SynthWave '84",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#262335",
        shell: "#241b2f",
        raised: "#37294d",
        card: "#2a2139",
        text: "#ffffff",
        muted: "#b8b5c6",
        faint: "#848bbd",
        accent: "#ff7edb",
        danger: "#fe4450",
        warning: "#fede5d",
        success: "#72f1b8",
        terminal_background: "#262335",
        ansi: [
            "#242424", "#fe4450", "#72f1b8", "#f3e70f", "#03edf9", "#ff7edb", "#03edf9", "#d4d4d8",
            "#52525b", "#fe4450", "#72f1b8", "#fede5d", "#03edf9", "#ff7edb", "#03edf9", "#fafafa",
        ],
        syntax: [
            "#848bbd", "#72f1b8", "#f97e72", "#2ee2fa", "#fe4450", "#72f1b8", "#2ee2fa", "#f8f8f2",
            "#72f1b8", "#72f1b8", "#dd5500", "#fe4450",
        ],
        source: source(
            "synthwave-84",
            "vscode",
            "https://github.com/robb0wen/synthwave-vscode",
            "ecfa2fe1279f7233663fa3f98a96e6756000567b",
            "MIT",
        ),
    })
}

fn shades_of_purple() -> ThemeVariant {
    variant(Seeds {
        id: "shades-of-purple",
        family_id: "shades-of-purple",
        name: "Shades of Purple",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#2d2b55",
        shell: "#222244",
        raised: "#393668",
        card: "#1e1e3f",
        text: "#ffffff",
        muted: "#b8afea",
        faint: "#8d84c7",
        accent: "#fad000",
        danger: "#ec3a37",
        warning: "#fad000",
        success: "#3ad900",
        terminal_background: "#1e1e3f",
        ansi: [
            "#000000", "#ec3a37", "#3ad900", "#fad000", "#7857fe", "#ff2c70", "#80fcff", "#ffffff",
            "#5c5c61", "#ec3a37", "#3ad900", "#fad000", "#6943ff", "#fb94ff", "#80fcff", "#ffffff",
        ],
        syntax: [
            "#b362ff", "#ff628c", "#fb94ff", "#fad000", "#fb94ff", "#fad000", "#fad000", "#9effff",
            "#fad000", "#fad000", "#ff9d00", "#ec3a37",
        ],
        source: source(
            "shades-of-purple",
            "vscode",
            "https://github.com/ahmadawais/shades-of-purple-vscode",
            "e8eb49f33e5db05ceba6677367b33ddb27ad821c",
            "MIT with additional upstream condition; see THIRD_PARTY_NOTICES.md",
        ),
    })
}

fn cobalt2() -> ThemeVariant {
    variant(Seeds {
        id: "cobalt2",
        family_id: "cobalt2",
        name: "Cobalt2",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#193549",
        shell: "#15232d",
        raised: "#20455e",
        card: "#122738",
        text: "#ffffff",
        muted: "#b7c9d3",
        faint: "#7da1b5",
        accent: "#ffc600",
        danger: "#ff628c",
        warning: "#ffc600",
        success: "#3ad900",
        terminal_background: "#122738",
        ansi: [
            "#000000", "#ff628c", "#3ad900", "#ffc600", "#0088ff", "#fb94ff", "#80fcff", "#ffffff",
            "#0050a4", "#ff628c", "#3ad900", "#ffc600", "#0088ff", "#fb94ff", "#80fcff", "#ffffff",
        ],
        syntax: [
            "#0088ff", "#ff9d00", "#ffee80", "#ffc600", "#ff68b8", "#ffc600", "#9effff", "#ffffff",
            "#ffee80", "#ffc600", "#ffb454", "#f44542",
        ],
        source: source(
            "cobalt2",
            "vscode",
            "https://github.com/wesbos/cobalt2-vscode",
            "c4e9574372b85afad1682ed0fdd1ac0411c62512",
            "MIT",
        ),
    })
}

fn andromeda() -> ThemeVariant {
    variant(Seeds {
        id: "andromeda",
        family_id: "andromeda",
        name: "Andromeda",
        appearance: Appearance::Dark,
        treatment: SurfaceTreatment::Opaque,
        background: "#23262e",
        shell: "#1e2027",
        raised: "#2d313b",
        card: "#23262e",
        text: "#d5ced9",
        muted: "#a6a0aa",
        faint: "#746f77",
        accent: "#00e8c6",
        danger: "#ee5d43",
        warning: "#ff9f2e",
        success: "#96e072",
        terminal_background: "#23262e",
        ansi: [
            "#242424", "#ee5d43", "#96e072", "#ffe66d", "#7cb7ff", "#ff00aa", "#00e8c6", "#d4d4d8",
            "#52525b", "#ee5d43", "#96e072", "#ffe66d", "#7cb7ff", "#ff00aa", "#00e8c6", "#fafafa",
        ],
        syntax: [
            "#a0a1a7", "#ee5d43", "#96e072", "#f39c12", "#ffe66d", "#ffe66d", "#d5ced9", "#f39c12",
            "#96e072", "#f92672", "#ee5d43", "#ee5d43",
        ],
        source: source(
            "andromeda",
            "vscode",
            "https://github.com/EliverLara/Andromeda",
            "d1abb48c69493000aa0133a32d594eb25e523d4f",
            "MIT",
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapted_palettes_keep_shared_content_readable_in_interaction_states() {
        for variant in [
            github_dark(),
            vercel_light(),
            vercel_dark(),
            claude_light(),
            claude_dark(),
            spotify_dark(),
            linear_light(),
            linear_dark(),
        ] {
            let colors = &variant.colors;
            for surface in [
                colors.background,
                colors.shell,
                colors.raised,
                colors.card,
                colors.dialog,
                colors.overlay,
                colors.input.blend_over(colors.background),
            ] {
                for background in [
                    surface,
                    colors.hover.blend_over(surface),
                    colors.active.blend_over(surface),
                    variant.accent.selection.blend_over(surface),
                ] {
                    assert!(
                        colors.text.contrast(background) >= 4.5,
                        "{} primary text on {background}",
                        variant.id
                    );
                    assert!(
                        colors.text_muted.contrast(background) >= 3.0,
                        "{} muted text on {background}",
                        variant.id
                    );
                }
                assert!(
                    variant.accent.primary.contrast(surface) >= 3.0,
                    "{} accent on {surface}",
                    variant.id
                );
            }

            for (index, color) in variant.terminal.ansi.iter().enumerate() {
                if index % 8 != 0 {
                    assert!(
                        color.contrast(variant.terminal.background) >= 3.0,
                        "{} terminal ANSI slot {index}",
                        variant.id
                    );
                }
            }
        }
    }

    #[test]
    fn github_dark_keeps_near_black_surfaces_with_distinct_elevation() {
        let variant = github_dark();
        let colors = &variant.colors;
        assert!(Color::WHITE.contrast(colors.background) >= 20.0);
        assert!(colors.background.contrast(colors.shell) < 1.1);
        assert!(Color::WHITE.contrast(colors.card) < Color::WHITE.contrast(colors.background));
        assert!(Color::WHITE.contrast(colors.raised) < Color::WHITE.contrast(colors.card));
        assert_eq!(variant.terminal.background, colors.background);
        assert_eq!(
            variant.recommended_surface_treatment,
            SurfaceTreatment::Opaque
        );
    }
}
