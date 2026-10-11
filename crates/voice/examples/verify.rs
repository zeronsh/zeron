use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("model directory"));
    let mut files: Vec<String> = args.collect();
    let accelerated = files
        .iter()
        .position(|a| a == "--gpu")
        .map(|i| files.remove(i))
        .is_some();
    if !zeron_voice::installed(&dir) {
        zeron_voice::download(&dir, &AtomicBool::new(false), |_| {})?;
    }
    let start = Instant::now();
    let mut model = zeron_voice::Recognizer::load(&dir, accelerated)?;
    println!(
        "load_ms={} accelerated={accelerated}",
        start.elapsed().as_millis()
    );
    for file in files {
        let mut wav = hound::WavReader::open(&file)?;
        let spec = wav.spec();
        anyhow::ensure!(
            spec.channels == 1 && spec.bits_per_sample == 16,
            "mono PCM16 fixture required"
        );
        let samples: Vec<f32> = wav
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.)
            .collect();
        let start = Instant::now();
        let text = model.transcribe(samples.clone(), spec.sample_rate)?;
        println!(
            "fixture={file} inference_ms={} transcript={text:?}",
            start.elapsed().as_millis()
        );
        let mut stream = zeron_voice::Stream::new(spec.sample_rate)?;
        let mut decode_ms = 0;
        for chunk in samples.chunks(spec.sample_rate as usize) {
            stream.push(chunk)?;
            let tick = Instant::now();
            if let Some(partial) = stream.tick(&mut model)? {
                decode_ms += tick.elapsed().as_millis();
                println!("  partial_ms={} {partial:?}", tick.elapsed().as_millis());
            }
        }
        let tick = Instant::now();
        let text = stream.finish(&mut model)?;
        println!(
            "streaming final_ms={} decode_ms={} transcript={text:?}",
            tick.elapsed().as_millis(),
            decode_ms + tick.elapsed().as_millis()
        );
    }
    Ok(())
}
