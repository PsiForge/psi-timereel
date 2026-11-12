#![allow(clippy::unwrap_used)]

use std::fs;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use chrono_tz::America::Sao_Paulo;
use chrono_tz::Europe::Amsterdam;
use timereel_store::Store;

use crate::*;

/// A UTC time as a capture time.
fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> CaptureTime {
    CaptureTime(
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .unwrap()
            .timestamp_millis(),
    )
}

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn idx(ms: &[i64]) -> Index {
    Index::from_times(ms.iter().map(|m| CaptureTime(*m)).collect())
}

/// Creates an (empty) capture file; listing looks at names only.
fn touch(store: &Store, t: CaptureTime) {
    let path = store.path_of(t);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"").unwrap();
}

#[test]
fn next_and_prev_at_start_middle_and_end() {
    let i = idx(&[10, 20, 30]);
    let t = CaptureTime;
    // start
    assert_eq!(i.prev(t(10)), None);
    assert_eq!(i.next(t(10)), Some(t(20)));
    // middle
    assert_eq!(i.prev(t(20)), Some(t(10)));
    assert_eq!(i.next(t(20)), Some(t(30)));
    // end
    assert_eq!(i.prev(t(30)), Some(t(20)));
    assert_eq!(i.next(t(30)), None);
    // a time that is not a capture
    assert_eq!(i.prev(t(25)), Some(t(20)));
    assert_eq!(i.next(t(25)), Some(t(30)));
    assert_eq!(i.prev(t(5)), None);
    assert_eq!(i.next(t(5)), Some(t(10)));
    // empty index
    assert_eq!(Index::new().next(t(1)), None);
    assert_eq!(Index::new().prev(t(1)), None);
}

#[test]
fn from_times_sorts_and_dedups() {
    let i = idx(&[30, 10, 20, 10]);
    assert_eq!(
        i.times(),
        &[CaptureTime(10), CaptureTime(20), CaptureTime(30)]
    );
    assert_eq!((i.len(), i.is_empty()), (3, false));
    assert_eq!(
        (i.first(), i.last()),
        (Some(CaptureTime(10)), Some(CaptureTime(30)))
    );
}

#[test]
fn range_of_a_local_day_spans_utc_day_folders() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    // Amsterdam is UTC+2 in May: 2025-05-14 local = 05-13T22:00Z .. 05-14T22:00Z.
    let outside_before = utc(2025, 5, 13, 21, 59, 59);
    let first = utc(2025, 5, 13, 22, 0, 0); // UTC folder 2025-05-13
    let noon = utc(2025, 5, 14, 10, 0, 0);
    let last = CaptureTime(utc(2025, 5, 14, 22, 0, 0).0 - 1); // UTC folder 2025-05-14
    let outside_after = utc(2025, 5, 14, 22, 0, 0);
    for t in [outside_before, first, noon, last, outside_after] {
        touch(&store, t);
    }
    assert_ne!(first.day_name(), last.day_name());

    let i = Index::load(&store).unwrap();
    assert_eq!(i.len(), 5);
    assert_eq!(i.range(date(2025, 5, 14), &Amsterdam), &[first, noon, last]);
    assert_eq!(i.range(date(2025, 5, 13), &Amsterdam), &[outside_before]);
    assert_eq!(i.range(date(2025, 5, 15), &Amsterdam), &[outside_after]);
    assert!(i.range(date(2025, 5, 16), &Amsterdam).is_empty());
    // The same instants form different days in UTC.
    assert_eq!(
        i.range(date(2025, 5, 14), &Utc),
        &[noon, last, outside_after]
    );
}

#[test]
fn at_or_after_cases() {
    let t = CaptureTime;
    let i = idx(&[10, 20, 30]);
    // exact match
    assert_eq!(
        i.at_or_after(t(20)),
        Some(Landing {
            time: t(20),
            exact: true
        })
    );
    // between captures
    assert_eq!(
        i.at_or_after(t(21)),
        Some(Landing {
            time: t(30),
            exact: true
        })
    );
    // before the first
    assert_eq!(
        i.at_or_after(t(1)),
        Some(Landing {
            time: t(10),
            exact: true
        })
    );
    // after the last: falls back to the nearest capture
    assert_eq!(
        i.at_or_after(t(31)),
        Some(Landing {
            time: t(30),
            exact: false
        })
    );
    // empty index
    assert_eq!(Index::new().at_or_after(t(1)), None);
}

#[test]
fn at_or_after_on_an_empty_day() {
    // Captures on 05-13 and 05-16 local; 05-14 and 05-15 are empty.
    let a = utc(2025, 5, 13, 10, 0, 0);
    let b = utc(2025, 5, 16, 10, 0, 0);
    let i = Index::from_times(vec![a, b]);
    let (start, _) = local_day_bounds(date(2025, 5, 14), &Amsterdam);
    // A later capture exists: take it.
    assert_eq!(
        i.at_or_after(start),
        Some(Landing {
            time: b,
            exact: true
        })
    );
    // Nothing follows: nearest (the last).
    let (start, _) = local_day_bounds(date(2025, 5, 20), &Amsterdam);
    assert_eq!(
        i.at_or_after(start),
        Some(Landing {
            time: b,
            exact: false
        })
    );
}

#[test]
fn nearest_picks_the_closest_and_the_earlier_on_a_tie() {
    let t = CaptureTime;
    let i = idx(&[10, 20, 40]);
    assert_eq!(i.nearest(t(10)), Some(t(10)));
    assert_eq!(i.nearest(t(14)), Some(t(10)));
    assert_eq!(i.nearest(t(15)), Some(t(10))); // tie -> earlier
    assert_eq!(i.nearest(t(16)), Some(t(20)));
    assert_eq!(i.nearest(t(31)), Some(t(40)));
    assert_eq!(i.nearest(t(-100)), Some(t(10)));
    assert_eq!(i.nearest(t(1000)), Some(t(40)));
    assert_eq!(Index::new().nearest(t(1)), None);
}

#[test]
fn quick_jumps_between_days() {
    let tz = Amsterdam;
    let may12 = utc(2025, 5, 12, 8, 0, 0);
    let may13 = utc(2025, 5, 13, 8, 0, 0);
    let may14 = utc(2025, 5, 14, 8, 0, 0);
    let may14b = utc(2025, 5, 14, 9, 0, 0);
    let may16 = utc(2025, 5, 16, 8, 0, 0);
    let i = Index::from_times(vec![may12, may13, may14, may14b, may16]);
    let now = utc(2025, 5, 16, 12, 0, 0);
    let q = |kind, cur| i.quick_jump(kind, cur, now, &tz);
    let hit = |time| Some(Landing { time, exact: true });

    assert_eq!(q(QuickJump::Today, may14), hit(may16));
    // Yesterday (05-15) has no captures: the next capture follows, not exact.
    assert_eq!(
        q(QuickJump::Yesterday, may14),
        Some(Landing {
            time: may16,
            exact: false
        })
    );
    assert_eq!(q(QuickJump::PreviousDay, may14b), hit(may13));
    assert_eq!(q(QuickJump::NextDay, may13), hit(may14));
    // The day after 05-14 is empty: lands on 05-16, flagged as not exact.
    assert_eq!(
        q(QuickJump::NextDay, may14b),
        Some(Landing {
            time: may16,
            exact: false
        })
    );
    // Next day from the last day: nothing follows, nearest = last, not exact.
    assert_eq!(
        q(QuickJump::NextDay, may16),
        Some(Landing {
            time: may16,
            exact: false
        })
    );
    // Previous day with no captures before: lands on the first one after.
    assert_eq!(
        q(QuickJump::PreviousDay, may12),
        Some(Landing {
            time: may12,
            exact: false
        })
    );
    assert_eq!(
        Index::new().quick_jump(QuickJump::Today, may14, now, &tz),
        None
    );
}

#[test]
fn today_with_no_captures_yet_shows_the_nearest() {
    let yesterday = utc(2025, 5, 15, 8, 0, 0);
    let i = Index::from_times(vec![yesterday]);
    let now = utc(2025, 5, 16, 5, 0, 0);
    assert_eq!(
        i.quick_jump(QuickJump::Today, yesterday, now, &Amsterdam),
        Some(Landing {
            time: yesterday,
            exact: false
        })
    );
}

#[test]
fn quick_jumps_use_the_local_day_not_the_utc_day() {
    // 23:30 local on 05-14 is 21:30Z: still 05-14 locally, and 00:30 local on
    // 05-15 is 22:30Z on 05-14 in UTC.
    let late = utc(2025, 5, 14, 21, 30, 0);
    let after_midnight = utc(2025, 5, 14, 22, 30, 0);
    let i = Index::from_times(vec![late, after_midnight]);
    let now = utc(2025, 5, 15, 12, 0, 0);
    let hit = |time| Some(Landing { time, exact: true });
    assert_eq!(
        i.quick_jump(QuickJump::NextDay, late, now, &Amsterdam),
        hit(after_midnight)
    );
    assert_eq!(
        i.quick_jump(QuickJump::PreviousDay, after_midnight, now, &Amsterdam),
        hit(late)
    );
    assert_eq!(
        i.quick_jump(QuickJump::Yesterday, late, now, &Amsterdam),
        hit(late)
    );
}

#[test]
fn quick_jumps_by_hour() {
    let h = |hour, min| utc(2025, 5, 14, hour, min, 0);
    let i = Index::from_times(vec![h(8, 0), h(8, 30), h(9, 0), h(9, 20), h(12, 0)]);
    let now = h(13, 0);
    let q = |kind, cur| i.quick_jump(kind, cur, now, &Amsterdam);
    let hit = |time| Some(Landing { time, exact: true });
    // +1 h: first capture at or after cur + 1 h
    assert_eq!(q(QuickJump::PlusHour, h(8, 0)), hit(h(9, 0)));
    assert_eq!(
        q(QuickJump::PlusHour, h(8, 30)),
        hit(h(12, 0)) // 09:30 target: the next capture is 12:00
    );
    // -1 h: last capture at or before cur - 1 h
    assert_eq!(q(QuickJump::MinusHour, h(9, 20)), hit(h(8, 0))); // 08:20 target
    assert_eq!(q(QuickJump::MinusHour, h(9, 0)), hit(h(8, 0)));
    // nothing an hour earlier: first capture, not exact
    assert_eq!(
        q(QuickJump::MinusHour, h(8, 30)),
        Some(Landing {
            time: h(8, 0),
            exact: false
        })
    );
    // nothing an hour later: last capture, not exact
    assert_eq!(
        q(QuickJump::PlusHour, h(12, 0)),
        Some(Landing {
            time: h(12, 0),
            exact: false
        })
    );
}

#[test]
fn quick_jumps_on_the_dst_change_days_of_amsterdam() {
    let tz = Amsterdam;
    // Spring forward: 2025-03-30, 02:00 CET -> 03:00 CEST. The day has 23 hours.
    let (s, e) = local_day_bounds(date(2025, 3, 30), &tz);
    assert_eq!(s, utc(2025, 3, 29, 23, 0, 0));
    assert_eq!(e, utc(2025, 3, 30, 22, 0, 0));
    assert_eq!(e.0 - s.0, 23 * 3_600_000);
    // Fall back: 2025-10-26, 03:00 CEST -> 02:00 CET. The day has 25 hours.
    let (s, e) = local_day_bounds(date(2025, 10, 26), &tz);
    assert_eq!(s, utc(2025, 10, 25, 22, 0, 0));
    assert_eq!(e, utc(2025, 10, 26, 23, 0, 0));
    assert_eq!(e.0 - s.0, 25 * 3_600_000);

    // Spring: captures every hour of real time around the change.
    let sat = utc(2025, 3, 29, 12, 0, 0); // 13:00 CET on the 29th
    let first_of_30 = utc(2025, 3, 29, 23, 30, 0); // 00:30 CET
    let before = utc(2025, 3, 30, 0, 30, 0); // 01:30 CET
    let after = utc(2025, 3, 30, 1, 30, 0); // 03:30 CEST
    let mon = utc(2025, 3, 30, 22, 30, 0); // 00:30 CEST on the 31st
    let i = Index::from_times(vec![sat, first_of_30, before, after, mon]);
    let now = utc(2025, 3, 31, 12, 0, 0);
    let hit = |time| Some(Landing { time, exact: true });
    assert_eq!(
        i.range(date(2025, 3, 30), &tz),
        &[first_of_30, before, after]
    );
    assert_eq!(
        i.quick_jump(QuickJump::NextDay, sat, now, &tz),
        hit(first_of_30)
    );
    assert_eq!(i.quick_jump(QuickJump::NextDay, after, now, &tz), hit(mon));
    assert_eq!(
        i.quick_jump(QuickJump::PreviousDay, mon, now, &tz),
        hit(first_of_30)
    );
    // +1 h is one real hour: 01:30 CET -> 03:30 CEST (local clock jumps two hours).
    assert_eq!(
        i.quick_jump(QuickJump::PlusHour, before, now, &tz),
        hit(after)
    );
    assert_eq!(
        i.quick_jump(QuickJump::MinusHour, after, now, &tz),
        hit(before)
    );

    // Autumn: the local hour 02:00-03:00 happens twice.
    let a = utc(2025, 10, 25, 22, 30, 0); // 00:30 CEST on the 26th
    let b = utc(2025, 10, 26, 0, 30, 0); // 02:30 CEST (first time)
    let c = utc(2025, 10, 26, 1, 30, 0); // 02:30 CET (second time)
    let d = utc(2025, 10, 26, 22, 30, 0); // 23:30 CET
    let next = utc(2025, 10, 26, 23, 30, 0); // 00:30 CET on the 27th
    let i = Index::from_times(vec![a, b, c, d, next]);
    let now = utc(2025, 10, 27, 12, 0, 0);
    assert_eq!(i.range(date(2025, 10, 26), &tz), &[a, b, c, d]);
    assert_eq!(i.quick_jump(QuickJump::Today, next, now, &tz), hit(next));
    assert_eq!(i.quick_jump(QuickJump::Yesterday, next, now, &tz), hit(a));
    assert_eq!(i.quick_jump(QuickJump::NextDay, d, now, &tz), hit(next));
    assert_eq!(i.quick_jump(QuickJump::PlusHour, b, now, &tz), hit(c));
    assert_eq!(i.quick_jump(QuickJump::MinusHour, c, now, &tz), hit(b));
}

#[test]
fn day_start_when_midnight_does_not_exist() {
    // Sao Paulo skipped 2018-11-04 00:00-01:00 (DST start at midnight).
    let start = local_day_start(date(2018, 11, 4), &Sao_Paulo);
    assert_eq!(start, utc(2018, 11, 4, 3, 0, 0)); // 01:00 local (UTC-2)
    let (s, e) = local_day_bounds(date(2018, 11, 4), &Sao_Paulo);
    assert_eq!(s, start);
    assert_eq!(e, utc(2018, 11, 5, 2, 0, 0));
    assert_eq!(local_date(start, &Sao_Paulo), date(2018, 11, 4));
}

#[test]
fn gaps_are_found_above_one_and_a_half_intervals_only() {
    let interval = Duration::from_secs(300); // 5 min -> limit 7 min 30 s
    let base = utc(2025, 5, 14, 9, 0, 0);
    let at = |s: i64| CaptureTime(base.0 + s * 1000);
    let times = [
        at(0),
        at(300),
        at(300 + 449),
        at(300 + 449 + 450),
        at(300 + 449 + 450 + 451),
    ];
    let gaps = find_gaps(&times, interval);
    // 300 s and 449 s are fine, exactly 450 s is not a gap, 451 s is.
    assert_eq!(
        gaps,
        vec![Gap {
            before: times[3],
            after: times[4]
        }]
    );
    assert_eq!(gaps[0].length_ms(), 451_000);
    // The same through the index.
    assert_eq!(Index::from_times(times.to_vec()).gaps(interval), gaps);
    // Several gaps, and none for empty or single-capture lists.
    let wide = [at(0), at(1000), at(2000)];
    assert_eq!(find_gaps(&wide, interval).len(), 2);
    assert!(find_gaps(&[], interval).is_empty());
    assert!(find_gaps(&[at(0)], interval).is_empty());
    // A different interval moves the limit: 1000 s is below 1.5 x 700 s.
    assert!(find_gaps(&wide, Duration::from_secs(700)).is_empty());
}

#[test]
fn incremental_refresh_adds_new_captures_without_a_full_reload() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let old1 = utc(2025, 5, 12, 9, 0, 0);
    let old2 = utc(2025, 5, 13, 9, 0, 0);
    let today1 = utc(2025, 5, 14, 9, 0, 0);
    for t in [old1, old2, today1] {
        touch(&store, t);
    }
    let mut i = Index::load(&store).unwrap();
    assert_eq!(i.times(), &[old1, old2, today1]);

    // Nothing changed.
    assert_eq!(i.refresh(&store).unwrap(), 0);
    assert_eq!(i.times(), &[old1, old2, today1]);

    // A new capture today, and an old day folder loses a file. A full reload
    // would drop `old1`; the incremental refresh keeps it.
    let today2 = utc(2025, 5, 14, 9, 5, 0);
    touch(&store, today2);
    fs::remove_file(store.path_of(old1)).unwrap();
    assert_eq!(i.refresh(&store).unwrap(), 1);
    assert_eq!(i.times(), &[old1, old2, today1, today2]);

    // The agent starts a new UTC day folder: it is picked up too.
    let tomorrow = utc(2025, 5, 15, 0, 1, 0);
    touch(&store, tomorrow);
    assert_eq!(i.refresh(&store).unwrap(), 1);
    assert_eq!(i.last(), Some(tomorrow));

    // A capture that disappeared from the re-listed folder is dropped.
    let tomorrow2 = utc(2025, 5, 15, 0, 6, 0);
    touch(&store, tomorrow2);
    assert_eq!(i.refresh(&store).unwrap(), 1);
    fs::remove_file(store.path_of(tomorrow2)).unwrap();
    assert_eq!(i.refresh(&store).unwrap(), 0);
    assert_eq!(i.times(), &[old1, old2, today1, today2, tomorrow]);

    // An explicit full load does apply the old deletion.
    assert_eq!(
        Index::load(&store).unwrap().times(),
        &[old2, today1, today2, tomorrow]
    );
}

#[test]
fn refresh_of_an_empty_index_loads_everything_and_missing_store_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let mut i = Index::load(&store).unwrap();
    assert!(i.is_empty());
    assert_eq!(i.refresh(&store).unwrap(), 0);
    let t1 = utc(2025, 5, 10, 9, 0, 0);
    let t2 = utc(2025, 5, 14, 9, 0, 0);
    touch(&store, t1);
    touch(&store, t2);
    assert_eq!(i.refresh(&store).unwrap(), 2);
    assert_eq!(i.times(), &[t1, t2]);
}
