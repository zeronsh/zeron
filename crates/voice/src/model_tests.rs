//! The real model on real speech. Needs the download (740 MB): it uses
//! `ZERON_VOICE_MODEL_DIR` when set, else fetches the model into the temp folder.
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
#[ignore = "downloads the model"]
fn model_transcribes_english_and_french() {
    let dir = std::env::var_os("ZERON_VOICE_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zeron-voice").join(manifest().id));
    if !installed(&dir) {
        download(&dir, &AtomicBool::new(false), |_| {}).unwrap();
    }
    let started = Instant::now();
    let mut model = Recognizer::load(&dir).unwrap();
    println!("load: {:?}", started.elapsed());
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
        println!("{:?}: {text}", started.elapsed());
        let heard = text.to_lowercase().replace(',', "");
        for words in says {
            assert!(heard.contains(words), "{words:?} not in {text:?}");
        }
    }
}
