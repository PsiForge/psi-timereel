//! `timereel-bench`: decrypt and decode benchmark (ADR 0013). Build and run
//! with `cargo run --release -p timereel-bench`. Exits non-zero when a
//! threshold is exceeded.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use timereel_bench::{check, data, stats, Stats, Thresholds, MEASURED_RUNS, WARMUP_RUNS};
use timereel_crypto::{create, unlock_with_password, KdfParams, SecretString};
use timereel_store::CaptureTime;
use timereel_timeline::{ImageLoader, Item, Kind, Status, StoreDecoder};

/// Cold-jump runs: each one builds a fresh loader with empty caches.
const COLD_RUNS: usize = 50;

fn run(thresholds: &Thresholds) -> Result<Vec<String>, String> {
    let password = SecretString::from("bench-password-not-a-secret".to_string());
    let (keyring, _recovery, key) =
        create(&password, KdfParams::PRODUCTION).map_err(|e| e.to_string())?;
    let started = Instant::now();
    unlock_with_password(&keyring, &password).map_err(|e| e.to_string())?;
    let unlock = started.elapsed();

    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let (store, times) = data::build_store(dir.path(), &key.public_key())?;
    let identity = key.identity();

    let mut rows: Vec<(String, Stats)> = Vec::new();
    let mut failures = Vec::new();
    for (label, kind, limit) in [
        ("full decrypt+decode", Kind::Full, &thresholds.full),
        ("thumb decrypt+decode", Kind::Thumb, &thresholds.thumb),
    ] {
        for (si, time) in times.iter().enumerate() {
            let one = || -> Result<Duration, String> {
                let t = Instant::now();
                let bytes = match kind {
                    Kind::Full => store.read_image(identity, *time),
                    Kind::Thumb => store.read_thumb(identity, *time),
                }
                .map_err(|e| e.to_string())?;
                let img = timereel_image::decode(&bytes).map_err(|e| e.to_string())?;
                std::hint::black_box(&img);
                Ok(t.elapsed())
            };
            for _ in 0..WARMUP_RUNS {
                one()?;
            }
            let samples = (0..MEASURED_RUNS)
                .map(|_| one())
                .collect::<Result<Vec<_>, _>>()?;
            let name = format!("{label} [{}x{}]", data::SOURCES[si].0, data::SOURCES[si].1);
            let s = stats(&samples).ok_or("no samples")?;
            failures.extend(check(&name, &s, limit));
            rows.push((name, s));
        }
    }

    let mut cold = Vec::new();
    for i in 0..COLD_RUNS {
        let time: CaptureTime = times[i % times.len()];
        let decoder = Arc::new(StoreDecoder::new(store.clone(), identity.clone()));
        let loader = ImageLoader::new(decoder, || {});
        let t = Instant::now();
        let mut status = loader.request(Kind::Full, time);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match status {
                Status::Done(Item::Ready(_)) => break,
                Status::Done(Item::Failed(m)) => return Err(format!("cold jump failed: {m}")),
                Status::Closed => return Err("loader closed".into()),
                Status::Pending => {
                    if Instant::now() > deadline {
                        return Err("cold jump timed out".into());
                    }
                    std::thread::sleep(Duration::from_micros(100));
                    status = loader.request(Kind::Full, time);
                }
            }
        }
        cold.push(t.elapsed());
    }
    let s = stats(&cold).ok_or("no samples")?;
    failures.extend(check("cold jump (loader)", &s, &thresholds.cold_jump));
    rows.push(("cold jump (loader)".into(), s));

    println!(
        "{:<40} {:>10} {:>10} {:>10}",
        "measurement", "median ms", "p95 ms", "max ms"
    );
    for (name, s) in &rows {
        println!(
            "{:<40} {:>10.2} {:>10.2} {:>10.2}",
            name, s.median, s.p95, s.max
        );
    }
    println!(
        "{:<40} {:>10.2} {:>10} {:>10}   (reported only)",
        "argon2 unlock",
        unlock.as_secs_f64() * 1000.0,
        "-",
        "-"
    );
    Ok(failures)
}

fn main() -> ExitCode {
    let thresholds = match Thresholds::from_env() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    match run(&thresholds) {
        Ok(f) if f.is_empty() => {
            println!("OK: all thresholds met");
            ExitCode::SUCCESS
        }
        Ok(f) => {
            for m in f {
                eprintln!("FAIL: {m}");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
