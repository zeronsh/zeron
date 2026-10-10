use crate::{Recognizer, resample};
use anyhow::Result;
use parakeet_rs::{TimedToken, TranscriptionResult};

const RATE: f32 = resample::MODEL_RATE as f32;
const TICK: usize = resample::MODEL_RATE as usize;
const COMMIT_AFTER: usize = 12 * TICK;
const FORCE_AFTER: usize = 20 * TICK;
const RIGHT_CONTEXT_SECS: f32 = 2.0;
const PAUSE_SECS: f32 = 0.3;

pub struct Stream {
    converter: resample::Converter,
    pcm: Vec<f32>,
    head: usize,
    decoded: usize,
    committed: String,
}

impl Stream {
    pub fn new(rate: u32) -> Result<Self> {
        Ok(Self {
            converter: resample::Converter::new(rate)?,
            pcm: Vec::new(),
            head: 0,
            decoded: 0,
            committed: String::new(),
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        self.converter.push(samples, &mut self.pcm)
    }

    pub fn tick(&mut self, model: &mut Recognizer) -> Result<Option<String>> {
        self.tick_with(&mut |window| model.decode(window))
    }

    pub fn finish(self, model: &mut Recognizer) -> Result<String> {
        self.finish_with(&mut |window| model.decode(window))
    }

    pub(crate) fn tick_with(
        &mut self,
        decode: &mut impl FnMut(&[f32]) -> Result<TranscriptionResult>,
    ) -> Result<Option<String>> {
        if self.pcm.len() - self.decoded < TICK {
            return Ok(None);
        }
        self.decoded = self.pcm.len();
        let window = &self.pcm[self.head..];
        let hyp = hypothesis(window, decode)?;
        let cut = (window.len() > COMMIT_AFTER)
            .then(|| {
                cut(
                    &hyp.tokens,
                    window.len() as f32 / RATE,
                    window.len() > FORCE_AFTER,
                )
            })
            .flatten();
        Ok(Some(match cut {
            Some((at, secs)) => {
                let (done, rest) = hyp.tokens.split_at(at);
                self.head += (secs * RATE) as usize;
                append(&mut self.committed, &text(done));
                join(&self.committed, &text(rest))
            }
            None => join(&self.committed, &hyp.text),
        }))
    }

    pub(crate) fn finish_with(
        mut self,
        decode: &mut impl FnMut(&[f32]) -> Result<TranscriptionResult>,
    ) -> Result<String> {
        self.converter.finish(&mut self.pcm)?;
        let hyp = hypothesis(&self.pcm[self.head..], decode)?;
        Ok(join(&self.committed, &hyp.text))
    }
}

fn hypothesis(
    window: &[f32],
    decode: &mut impl FnMut(&[f32]) -> Result<TranscriptionResult>,
) -> Result<TranscriptionResult> {
    if window.len() < TICK / 5 || window.iter().all(|s| s.abs() < 0.0001) {
        return Ok(TranscriptionResult {
            text: String::new(),
            tokens: Vec::new(),
        });
    }
    decode(window)
}

fn cut(tokens: &[TimedToken], secs: f32, force: bool) -> Option<(usize, f32)> {
    let limit = secs - RIGHT_CONTEXT_SECS;
    let mut sentence = None;
    let mut pause = None;
    let mut word = None;
    let mut speech_end = 0.0;
    for (i, pair) in tokens.windows(2).enumerate() {
        let (a, b) = (&pair[0], &pair[1]);
        if a.end > limit {
            break;
        }
        if !punctuation(a) {
            speech_end = a.end;
        }
        if !b.text.starts_with(' ') || punctuation(b) {
            continue;
        }
        let at = Some((i + 1, (a.end + b.start) / 2.0));
        word = at;
        if b.start - speech_end >= PAUSE_SECS {
            pause = at;
            if a.text.ends_with(['.', '?', '!']) {
                sentence = at;
            }
        }
    }
    sentence.or(pause).or(word.filter(|_| force))
}

fn punctuation(token: &TimedToken) -> bool {
    let text = token.text.trim();
    !text.is_empty() && text.chars().all(|c| c.is_ascii_punctuation())
}

fn text(tokens: &[TimedToken]) -> String {
    tokens
        .iter()
        .map(|t| t.text.as_str())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn append(committed: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !committed.is_empty() {
        committed.push(' ');
    }
    committed.push_str(piece);
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_owned(),
        (_, true) => a.to_owned(),
        _ => format!("{a} {b}"),
    }
}
