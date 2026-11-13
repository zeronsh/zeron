//! The real models on real speech. Needs the downloads (740 MB and 1.8 GB):
//! `ZERON_VOICE_MODELS` is a folder of them (`<id>/`, as Zeron keeps them),
//! else they're fetched into the temp folder.
//!
//!     cargo test -p zeron-voice --release model_ -- --ignored --nocapture
use super::*;
use std::path::PathBuf;

fn clip(wav: &[u8]) -> (Vec<f32>, u32) {
    let wav = hound::WavReader::new(wav).unwrap();
    let rate = wav.spec().sample_rate;
    let samples = wav
        .into_samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.)
        .collect();
    (samples, rate)
}

#[test]
#[ignore = "downloads the models"]
fn model_each_transcribes_english_and_french() {
    let root = std::env::var_os("ZERON_VOICE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zeron-voice"));
    for m in models() {
        let dir = root.join(&m.id);
        if !m.installed(&dir) {
            m.download(&dir, &AtomicBool::new(false), |_| {}).unwrap();
        }
        let started = Instant::now();
        let mut model = Recognizer::load(&dir).unwrap();
        println!("{}: load {:?}", m.name, started.elapsed());
        // Someone whose system speaks English, then French: a model that
        // can't tell the language hears which, each time.
        model.speaks(&["en", "fr"]);
        for (wav, says) in [
            (
                &include_bytes!("../testdata/en-librispeech-1272-128104-0000.wav")[..],
                [
                    "quilter is the apostle of the middle classes",
                    "glad to welcome his gospel",
                ],
            ),
            (
                &include_bytes!("../testdata/fr-fleurs-1570.wav")[..],
                [
                    "pouvoir de déroger aux lois fiscales",
                    "droits de douane entre les états",
                ],
            ),
        ] {
            let (samples, rate) = clip(wav);
            let started = Instant::now();
            let text = model.transcribe(samples, rate).unwrap();
            println!("{}: {:?}: {text}", m.name, started.elapsed());
            let heard = text.to_lowercase().replace(',', "");
            for words in says {
                assert!(
                    heard.contains(words),
                    "{}: {words:?} not in {text:?}",
                    m.name
                );
            }
        }
    }
}
