//! Tests of the search filter, the filtered navigation and the progress.

#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use timereel_store::CaptureMeta;

use crate::*;

const MIN: i64 = 60_000;

fn meta(app: &str, title: &str) -> CaptureMeta {
    CaptureMeta {
        taken_at_ms: 0,
        tz_offset_min: 0,
        app_name: app.into(),
        window_title: title.into(),
        width: 16,
        height: 9,
        format: "webp".into(),
    }
}

fn t(i: i64) -> CaptureTime {
    CaptureTime(1_747_213_200_000 + i * 5 * MIN)
}

/// Serves `(app, title)` by capture number; number 3 cannot be read.
struct Table(Vec<(&'static str, &'static str)>);

impl MetaSource for Table {
    fn load(&self, time: CaptureTime) -> Result<CaptureMeta, String> {
        let i = ((time.0 - t(0).0) / (5 * MIN)) as usize;
        if i == 3 {
            return Err("unreadable".into());
        }
        let (app, title) = self.0[i];
        Ok(meta(app, title))
    }
}

fn loaded(rows: Vec<(&'static str, &'static str)>) -> (MetaStore, Vec<CaptureTime>) {
    let times: Vec<_> = (0..rows.len() as i64).map(t).collect();
    let store = MetaStore::new(Arc::new(Table(rows)), || {});
    store.queue(times.clone());
    let end = Instant::now() + Duration::from_secs(20);
    while !store.is_complete() {
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    (store, times)
}

#[test]
fn matching_ignores_case_including_non_ascii() {
    let m = meta("Terminal", "Größe ÄNDERN — Ärger");
    for q in ["terminal", "TERMINAL", "TeRmInAl", "größe", "GRÖßE"] {
        assert!(SearchFilter::new(q).matches(&m), "query {q}");
    }
    assert!(SearchFilter::new("ärger").matches(&m));
    assert!(SearchFilter::new("ÄRGER").matches(&m));
    assert!(SearchFilter::new("ändern").matches(&m));
    assert!(SearchFilter::new("GRÖßE").matches(&m));
    // Greek and Cyrillic work the same way.
    let m = meta("Σιγμα", "Привет Мир");
    assert!(SearchFilter::new("σιγμα").matches(&m));
    assert!(SearchFilter::new("ПРИВЕТ").matches(&m));
    assert!(SearchFilter::new("мир").matches(&m));
    assert!(!SearchFilter::new("world").matches(&m));
}

#[test]
fn matching_looks_in_the_app_and_in_the_title() {
    let m = meta("Firefox", "Inbox - Mail");
    assert!(SearchFilter::new("fire").matches(&m), "app only");
    assert!(SearchFilter::new("mail").matches(&m), "title only");
    assert!(
        SearchFilter::new("x - m").matches(&m),
        "substring inside the title"
    );
    assert!(
        !SearchFilter::new("firefox mail").matches(&m),
        "not across the fields"
    );
    assert!(!SearchFilter::new("chrome").matches(&m));
}

#[test]
fn an_empty_query_matches_everything() {
    let f = SearchFilter::new("");
    assert!(f.is_empty());
    assert!(f.matches(&meta("", "")));
    assert!(f.matches(&meta("A", "b")));
    // Also captures whose details are not loaded or cannot be read.
    let (store, times) = loaded(vec![("a", "x"), ("b", "y"), ("c", "z"), ("d", "w")]);
    assert_eq!(store.matching(&times, &f), times);
    assert!(store.matches(CaptureTime(1), &f), "unknown capture too");
    assert!(!SearchFilter::new(" ").is_empty(), "a space is a query");
}

#[test]
fn a_filter_never_prints_its_query() {
    let text = format!("{:?}", SearchFilter::new("secret-project"));
    assert!(!text.contains("secret"), "{text}");
}

#[test]
fn only_loaded_readable_captures_match_a_query() {
    let (store, times) = loaded(vec![
        ("Code", "a"),
        ("Mail", "b"),
        ("Code", "c"),
        ("Code", "d"),
    ]);
    let f = SearchFilter::new("code");
    // Number 3 is unreadable, so it does not match.
    assert_eq!(store.matching(&times, &f), vec![times[0], times[2]]);
    assert!(!store.matches(CaptureTime(1), &f), "not loaded yet");
}

#[test]
fn filtered_next_and_prev_skip_captures_that_do_not_match() {
    let rows = vec![
        ("Code", "a"),
        ("Mail", "b"),
        ("Mail", "c"),
        ("Code", "d"),
        ("Mail", "e"),
        ("Code", "f"),
    ];
    let (store, times) = loaded(rows);
    // Number 3 is unreadable, so it does not match: the matches are 0 and 5.
    let f = SearchFilter::new("code");
    let index = Index::from_times(times.clone());
    let keep = |c| store.matches(c, &f);
    assert_eq!(index.next_matching(times[0], keep), Some(times[5]));
    assert_eq!(index.prev_matching(times[5], keep), Some(times[0]));
    assert_eq!(
        index.next_matching(times[5], keep),
        None,
        "no match after the last"
    );
    assert_eq!(
        index.prev_matching(times[0], keep),
        None,
        "no match before the first"
    );
    // From a capture that does not match itself.
    assert_eq!(index.next_matching(times[1], keep), Some(times[5]));
    assert_eq!(index.prev_matching(times[4], keep), Some(times[0]));
    // Between two indexed times.
    let between = CaptureTime(times[1].0 + 1);
    assert_eq!(index.prev_matching(between, keep), Some(times[0]));
    // With everything matching, it is the plain next and prev.
    let all = |_| true;
    assert_eq!(index.next_matching(times[1], all), index.next(times[1]));
    assert_eq!(index.prev_matching(times[1], all), index.prev(times[1]));
}

#[test]
fn filtered_navigation_visits_each_match_in_order() {
    let rows = vec![
        ("A", "x"),
        ("B", "x"),
        ("A", "x"),
        ("C", "x"),
        ("A", "x"),
        ("B", "x"),
        ("A", "x"),
    ];
    let (store, times) = loaded(rows);
    let f = SearchFilter::new("a");
    let index = Index::from_times(times.clone());
    let mut seen = vec![times[0]];
    while let Some(n) = index.next_matching(*seen.last().unwrap(), |c| store.matches(c, &f)) {
        seen.push(n);
    }
    assert_eq!(seen, vec![times[0], times[2], times[4], times[6]]);
    let mut back = vec![times[6]];
    while let Some(p) = index.prev_matching(*back.last().unwrap(), |c| store.matches(c, &f)) {
        back.push(p);
    }
    back.reverse();
    assert_eq!(back, seen);
}

#[test]
fn progress_counts_the_captures_dealt_with() {
    let (store, times) = loaded(vec![("a", "x"), ("b", "y"), ("c", "z"), ("d", "w")]);
    assert_eq!(store.progress(), (4, 4));
    assert!(store.is_complete());
    // Known times count as done as well.
    store.queue(times.clone());
    let end = Instant::now() + Duration::from_secs(20);
    while !store.is_complete() {
        assert!(Instant::now() < end, "timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(store.progress(), (8, 8));
}

#[test]
fn lock_empties_the_meta_store_and_its_search_results() {
    let (store, times) = loaded(vec![
        ("Code", "a"),
        ("Code", "b"),
        ("Code", "c"),
        ("Code", "d"),
    ]);
    let f = SearchFilter::new("code");
    assert_eq!(store.matching(&times, &f).len(), 3);
    assert_eq!(store.len(), 4);
    let hook = store.lock_hook();
    hook();
    assert!(store.is_empty());
    assert_eq!(store.progress(), (0, 0));
    assert!(
        store.matching(&times, &f).is_empty(),
        "nothing left to find"
    );
    assert_eq!(store.get(times[0]), None);
}
