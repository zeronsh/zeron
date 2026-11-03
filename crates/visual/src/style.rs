//! The style vocabulary a visual may use. Every value names something in the
//! active Zeron theme (a color token, a step on the transcript's type scale),
//! so a script can't paint a background, pick a font, or hard-code a color:
//! the renderer resolves tokens against whatever theme, surface (glass or
//! opaque), appearance, and text size the reader has.

use crate::VisualError;

/// A theme color a visual may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    /// Primary text.
    Text,
    /// Labels, axes, captions.
    Muted,
    /// Tertiary notes.
    Faint,
    /// The one thing to notice; also the first chart series.
    Accent,
    /// Text drawn on an accent mark.
    OnAccent,
    Success,
    Warning,
    Danger,
    /// Hairlines: rules, gridlines, axes.
    Border,
    /// Categorical series colors, `chart1` … `chart6` (0-based here).
    Chart(u8),
    /// A physical color (`0xRRGGBB`): only 3D scenes take these, for the
    /// materials of real objects (wood, foliage, a model's own paint).
    Rgb(u32),
}

pub const CHART_SERIES: u8 = 6;

/// Every token name a script may write, for error messages and the agent
/// guidance.
pub const TOKEN_NAMES: [&str; 15] = [
    "text",
    "muted",
    "faint",
    "accent",
    "on-accent",
    "success",
    "warning",
    "danger",
    "border",
    "chart1",
    "chart2",
    "chart3",
    "chart4",
    "chart5",
    "chart6",
];

impl Token {
    pub fn parse(name: &str) -> Result<Self, VisualError> {
        Ok(match name {
            "text" => Token::Text,
            "muted" => Token::Muted,
            "faint" => Token::Faint,
            "accent" => Token::Accent,
            "on-accent" => Token::OnAccent,
            "success" => Token::Success,
            "warning" => Token::Warning,
            "danger" => Token::Danger,
            "border" => Token::Border,
            other => match other
                .strip_prefix("chart")
                .and_then(|n| n.parse::<u8>().ok())
            {
                Some(n @ 1..=CHART_SERIES) => Token::Chart(n - 1),
                _ => {
                    return Err(VisualError::script(format!(
                        "unknown color '{other}': colors are theme tokens ({}), so a visual \
                         reads on glass and opaque, light and dark",
                        TOKEN_NAMES.join(", ")
                    )));
                }
            },
        })
    }

    /// The `index`th categorical series color (0-based, cycling).
    pub fn series(index: usize) -> Self {
        Token::Chart((index % CHART_SERIES as usize) as u8)
    }
}

/// A resolved-at-paint color: a token, optionally mixed toward a second
/// token, at an opacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub token: Token,
    pub mix: Option<(Token, f32)>,
    pub alpha: f32,
}

impl Color {
    pub const fn token(token: Token) -> Self {
        Self {
            token,
            mix: None,
            alpha: 1.0,
        }
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self {
            alpha: self.alpha * alpha.clamp(0.0, 1.0),
            ..self
        }
    }
}

/// A step on the transcript's type scale. Sizes are design pixels at the
/// 16px interface base; the renderer scales them with the reader's text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSize {
    Xs,
    Sm,
    #[default]
    Body,
    Lg,
    Xl,
    Xxl,
}

impl TextSize {
    pub fn parse(name: &str) -> Result<Self, VisualError> {
        Ok(match name {
            "xs" => TextSize::Xs,
            "sm" => TextSize::Sm,
            "body" | "md" | "base" => TextSize::Body,
            "lg" => TextSize::Lg,
            "xl" => TextSize::Xl,
            "2xl" => TextSize::Xxl,
            other => {
                return Err(VisualError::script(format!(
                    "unknown text size '{other}': use xs, sm, body, lg, xl, or 2xl (the \
                     transcript's type scale)"
                )));
            }
        })
    }

    /// (font size, line height) in design pixels at the 16px base, the same
    /// steps the transcript's markdown uses.
    pub fn metrics(self) -> (f32, f32) {
        match self {
            TextSize::Xs => (11.0, 16.0),
            TextSize::Sm => (12.0, 18.0),
            TextSize::Body => (14.0, 22.0),
            TextSize::Lg => (15.0, 22.0),
            TextSize::Xl => (16.0, 24.0),
            TextSize::Xxl => (19.0, 27.0),
        }
    }
}

/// Weights the interface font carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Weight {
    #[default]
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl Weight {
    pub fn from_number(weight: f64) -> Self {
        match weight as i64 {
            ..=449 => Weight::Regular,
            450..=549 => Weight::Medium,
            550..=649 => Weight::Semibold,
            _ => Weight::Bold,
        }
    }

    pub fn parse(name: &str) -> Result<Self, VisualError> {
        Ok(match name {
            "regular" | "normal" => Weight::Regular,
            "medium" => Weight::Medium,
            "semibold" => Weight::Semibold,
            "bold" => Weight::Bold,
            other => {
                return Err(VisualError::script(format!(
                    "unknown weight '{other}': use 400-700 or regular, medium, semibold, bold"
                )));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

impl Align {
    pub fn parse(name: &str) -> Result<Self, VisualError> {
        Ok(match name {
            "start" | "left" | "top" => Align::Start,
            "center" | "middle" => Align::Center,
            "end" | "right" | "bottom" => Align::End,
            other => {
                return Err(VisualError::script(format!(
                    "unknown alignment '{other}': use start, center, or end"
                )));
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    Between,
}

impl Justify {
    pub fn parse(name: &str) -> Result<Self, VisualError> {
        Ok(match name {
            "between" | "space-between" => Justify::Between,
            other => match Align::parse(other)? {
                Align::Start => Justify::Start,
                Align::Center => Justify::Center,
                Align::End => Justify::End,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_theme_tokens_only() {
        assert_eq!(Token::parse("chart1").unwrap(), Token::Chart(0));
        assert_eq!(Token::parse("chart6").unwrap(), Token::Chart(5));
        for bad in [
            "chart0",
            "chart7",
            "#ff0000",
            "red",
            "rgb(0,0,0)",
            "background",
        ] {
            let err = Token::parse(bad).unwrap_err().to_string();
            assert!(err.contains("theme tokens"), "{bad}: {err}");
        }
        assert_eq!(Token::series(7), Token::Chart(1));
    }

    #[test]
    fn sizes_follow_the_transcript_scale() {
        assert_eq!(TextSize::parse("body").unwrap().metrics(), (14.0, 22.0));
        assert_eq!(TextSize::parse("2xl").unwrap().metrics(), (19.0, 27.0));
        assert!(TextSize::parse("40px").is_err());
        assert_eq!(Weight::from_number(600.0), Weight::Semibold);
    }
}
