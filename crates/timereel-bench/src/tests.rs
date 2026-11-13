#![allow(clippy::unwrap_used)]
use super::*;

fn ms(v: &[u64]) -> Vec<Duration> {
    v.iter().map(|m| Duration::from_millis(*m)).collect()
}

#[test]
fn stats_median_p95_max() {
    let s = stats(&ms(&(1..=100).collect::<Vec<_>>())).unwrap();
    assert!((s.median - 50.5).abs() < 1e-9);
    assert!((s.p95 - 95.0).abs() < 1e-9);
    assert!((s.max - 100.0).abs() < 1e-9);
    assert!(stats(&[]).is_none());
}

#[test]
fn over_limit_sample_fails_and_under_limit_passes() {
    let t = Thresholds::default();
    let under = stats(&ms(&[10, 12, 15, 20, 30])).unwrap();
    assert!(check("full", &under, &t.full).is_empty());

    let slow_median = stats(&ms(&[50, 55, 60])).unwrap();
    assert_eq!(check("full", &slow_median, &t.full).len(), 1);

    let slow_tail = stats(&ms(&[1, 1, 1, 1, 1, 1, 1, 1, 1, 150])).unwrap();
    let msgs = check("full", &slow_tail, &t.full);
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("p95"));

    let thumb_slow = stats(&ms(&[6, 6, 6])).unwrap();
    assert!(!check("thumb", &thumb_slow, &t.thumb).is_empty());
    let cold_slow = stats(&ms(&[130, 130])).unwrap();
    assert!(!check("cold", &cold_slow, &t.cold_jump).is_empty());
    let cold_ok = stats(&ms(&[100, 110])).unwrap();
    assert!(check("cold", &cold_ok, &t.cold_jump).is_empty());
}

#[test]
fn defaults_match_adr_0013_and_env_overrides_apply() {
    let d = Thresholds::default();
    assert_eq!((d.full.median, d.full.p95), (Some(40.0), Some(100.0)));
    assert_eq!((d.thumb.median, d.thumb.p95), (Some(5.0), Some(10.0)));
    assert_eq!((d.cold_jump.median, d.cold_jump.p95), (None, Some(120.0)));
    let t = Thresholds::from_lookup(|k| (k == "TIMEREEL_BENCH_FULL_P95_MS").then(|| "7".into()))
        .unwrap();
    assert_eq!(t.full.p95, Some(7.0));
    assert!(Thresholds::from_lookup(|_| Some("abc".into())).is_err());
}

#[test]
fn synthetic_images_are_deterministic() {
    let a = synthetic_desktop(64, 36, 1).unwrap();
    let b = synthetic_desktop(64, 36, 1).unwrap();
    let c = synthetic_desktop(64, 36, 2).unwrap();
    assert_eq!(a.data(), b.data());
    assert_ne!(a.data(), c.data());
}
