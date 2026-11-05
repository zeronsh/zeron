//! The voice call caption's streaming veil (`zeron-veil`), exactly as the
//! desktop stage runs it. Offsets are UTF-16 so the platform can apply them to
//! its attributed strings directly.
use std::sync::Mutex;
use std::time::Instant;
use zeron_veil::CaptionVeil;

/// A veiled UTF-16 range of the caption and its opacity (0..1).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CaptionSpan {
    pub start: u32,
    pub end: u32,
    pub alpha: f32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CaptionFrame {
    /// The tail of the utterance to show (ellipsized in front when long).
    pub text: String,
    pub spans: Vec<CaptionSpan>,
    /// The previous utterance while it fades out, and its opacity.
    pub previous: Option<String>,
    pub previous_alpha: f32,
    /// Something is still fading: ask for the next frame.
    pub animating: bool,
}

/// One call's caption. Advance it whenever the caption changes and on every
/// display frame while the last frame was `animating`.
#[derive(uniffi::Object)]
pub struct CaptionFader {
    veil: Mutex<CaptionVeil>,
}

#[uniffi::export]
impl CaptionFader {
    #[uniffi::constructor]
    pub fn new(max_chars: u32) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            veil: Mutex::new(CaptionVeil::new(max_chars as usize)),
        })
    }

    pub fn frame(&self, item: Option<String>, text: String, reduced_motion: bool) -> CaptionFrame {
        let mut veil = self.veil.lock().unwrap();
        let frame = veil.advance(item.as_deref(), &text, Instant::now(), reduced_motion);
        let utf16 = |byte: usize| frame.text[..byte].encode_utf16().count() as u32;
        let spans = frame
            .spans
            .iter()
            .map(|(range, alpha)| CaptionSpan {
                start: utf16(range.start),
                end: utf16(range.end),
                alpha: *alpha,
            })
            .collect();
        let (previous, previous_alpha) = frame
            .previous
            .clone()
            .map_or((None, 0.0), |(text, alpha)| (Some(text), alpha));
        CaptionFrame {
            text: frame.text,
            spans,
            previous,
            previous_alpha,
            animating: veil.is_animating(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_utf16_offsets_into_the_shown_tail() {
        let fader = CaptionFader::new(160);
        let frame = fader.frame(Some("a".into()), "Hola 👋 ".into(), false);
        assert_eq!(frame.text, "Hola 👋");
        // "👋" is two UTF-16 units: the whole caption spans 7 of them.
        assert_eq!(
            frame.spans,
            vec![CaptionSpan {
                start: 0,
                end: 7,
                alpha: 0.0
            }]
        );
        assert!(frame.animating);
        let frame = fader.frame(Some("b".into()), "Sí".into(), false);
        assert_eq!(frame.previous.as_deref(), Some("Hola 👋"));
        assert_eq!(frame.previous_alpha, 1.0);
    }
}
