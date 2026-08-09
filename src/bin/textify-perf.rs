use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use textify::performance::{CorpusSpec, generate_corpus, measure_corpus, peak_rss_bytes};

fn main() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "textify-perf-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    ));
    let generated = generate_corpus(&root, CorpusSpec::production())?;
    let measurements = measure_corpus(&generated)?;

    println!("Textify core performance corpus: {}", root.display());
    println!("fixture\tbytes\tmode\topen_ms\tsave_ms");
    for measurement in measurements {
        println!(
            "{}\t{}\t{:?}\t{:.2}\t{:.2}",
            measurement.name,
            measurement.bytes,
            measurement.mode,
            measurement.open.as_secs_f64() * 1_000.0,
            measurement.save.as_secs_f64() * 1_000.0,
        );
    }
    if let Some(bytes) = peak_rss_bytes() {
        println!("peak_rss_mib: {:.2}", bytes as f64 / (1024.0 * 1024.0));
    }
    println!("100-tab session: {}", generated.session_path.display());
    Ok(())
}
