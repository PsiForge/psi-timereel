//! The jump controls of the timeline (ticket T21): a date picker with hour
//! and minute fields and a Go button, and the quick-jump buttons.
//!
//! This module holds the state of the controls and draws them; it does not
//! move the selection. [`JumpControls::show`] returns the [`JumpAction`] the
//! user chose, and the [`crate::timeline_ui::TimelineView`] resolves it with
//! the T18 queries ([`timereel_timeline::Index::at_or_after`] and
//! [`timereel_timeline::Index::quick_jump`]), selects the landing and gives
//! it the highest loader priority.
//!
//! Days are local days: the viewer's time zone ([`Zone::Local`]) unless a
//! fixed offset is set (tests).

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike,
};
pub use timereel_timeline::QuickJump;
use timereel_timeline::{local_date, CaptureTime, Index, Landing};

/// The label of the Go button.
pub const GO_LABEL: &str = "Go";
/// The label of the quick jump to today.
pub const TODAY_LABEL: &str = "Today";
/// The label of the quick jump to yesterday.
pub const YESTERDAY_LABEL: &str = "Yesterday";
/// The label of the quick jump to the previous day.
pub const PREVIOUS_DAY_LABEL: &str = "Previous day";
/// The label of the quick jump to the next day.
pub const NEXT_DAY_LABEL: &str = "Next day";
/// The label of the quick jump one hour back.
pub const MINUS_HOUR_LABEL: &str = "-1 h";
/// The label of the quick jump one hour forward.
pub const PLUS_HOUR_LABEL: &str = "+1 h";
/// The end of the message shown when a jump had to fall back to the nearest
/// capture. It starts with what was missing, for example
/// `No captures on 2025-05-13`.
pub const SHOWING_NEAREST_SUFFIX: &str = "; showing nearest";

/// The quick-jump buttons in the order they are drawn.
pub const QUICK_JUMPS: [(QuickJump, &str); 6] = [
    (QuickJump::Today, TODAY_LABEL),
    (QuickJump::Yesterday, YESTERDAY_LABEL),
    (QuickJump::PreviousDay, PREVIOUS_DAY_LABEL),
    (QuickJump::NextDay, NEXT_DAY_LABEL),
    (QuickJump::MinusHour, MINUS_HOUR_LABEL),
    (QuickJump::PlusHour, PLUS_HOUR_LABEL),
];

/// The time zone of the "local" days and clock times.
#[derive(Debug, Clone, Copy)]
pub enum Zone {
    /// The time zone of the computer.
    Local,
    /// A fixed offset (tests).
    Fixed(FixedOffset),
}

fn local_naive<Tz: TimeZone>(time: CaptureTime, tz: &Tz) -> NaiveDateTime {
    DateTime::from_timestamp_millis(time.0)
        .map(|utc| utc.with_timezone(tz).naive_local())
        .unwrap_or_default()
}

/// The instant of a local wall-clock time. In a DST gap (the time does not
/// exist) it is the same clock time one hour later; if it happens twice, the
/// earlier one.
fn resolve<Tz: TimeZone>(local: NaiveDateTime, tz: &Tz) -> Option<CaptureTime> {
    let at = |t: NaiveDateTime| {
        tz.from_local_datetime(&t)
            .earliest()
            .map(|t| CaptureTime(t.timestamp_millis()))
    };
    at(local).or_else(|| at(local + Duration::hours(1)))
}

impl Zone {
    /// The local calendar date of `time`.
    pub fn date(&self, time: CaptureTime) -> NaiveDate {
        match self {
            Zone::Local => local_date(time, &Local),
            Zone::Fixed(tz) => local_date(time, tz),
        }
    }

    /// The local date and clock time of `time`.
    pub fn date_time(&self, time: CaptureTime) -> NaiveDateTime {
        match self {
            Zone::Local => local_naive(time, &Local),
            Zone::Fixed(tz) => local_naive(time, tz),
        }
    }

    /// The instant of `date` at `hour`:`minute` local time.
    pub fn resolve(&self, date: NaiveDate, hour: u32, minute: u32) -> Option<CaptureTime> {
        let local = date.and_hms_opt(hour, minute, 0)?;
        match self {
            Zone::Local => resolve(local, &Local),
            Zone::Fixed(tz) => resolve(local, tz),
        }
    }

    /// Whether the index has a capture on the local day `date`.
    pub fn has_captures(&self, index: &Index, date: NaiveDate) -> bool {
        match self {
            Zone::Local => !index.range(date, &Local).is_empty(),
            Zone::Fixed(tz) => !index.range(date, tz).is_empty(),
        }
    }

    /// Where a quick jump lands (the T18 query).
    pub fn quick_jump(
        &self,
        index: &Index,
        kind: QuickJump,
        current: CaptureTime,
        now: CaptureTime,
    ) -> Option<Landing> {
        match self {
            Zone::Local => index.quick_jump(kind, current, now, &Local),
            Zone::Fixed(tz) => index.quick_jump(kind, current, now, tz),
        }
    }
}

/// What the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpAction {
    /// Jump to the date and time in the fields.
    Go,
    /// One of the quick-jump buttons.
    Quick(QuickJump),
}

/// The state of the jump controls.
#[derive(Debug, Clone)]
pub struct JumpControls {
    date: NaiveDate,
    hour: u32,
    minute: u32,
    message: Option<String>,
    zone: Zone,
    now: Option<CaptureTime>,
    /// The selection the fields were last set from.
    synced: Option<CaptureTime>,
}

impl JumpControls {
    /// Controls in the local time zone, with the fields at the current time.
    pub fn new() -> Self {
        let mut controls = Self {
            date: NaiveDate::default(),
            hour: 0,
            minute: 0,
            message: None,
            zone: Zone::Local,
            now: None,
            synced: None,
        };
        controls.set_fields_from(controls.now());
        controls
    }

    /// The time zone of the days and clock times.
    pub fn zone(&self) -> Zone {
        self.zone
    }

    /// Sets the time zone (tests use a fixed offset).
    pub fn set_zone(&mut self, zone: Zone) {
        self.zone = zone;
    }

    /// Overrides the clock behind Today and Yesterday (tests).
    pub fn set_now(&mut self, now: CaptureTime) {
        self.now = Some(now);
    }

    /// The current time: the override, or the system clock.
    pub fn now(&self) -> CaptureTime {
        self.now.unwrap_or_else(|| {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            CaptureTime(ms)
        })
    }

    /// The date in the date field.
    pub fn date(&self) -> NaiveDate {
        self.date
    }

    /// The hour field (0 to 23).
    pub fn hour(&self) -> u32 {
        self.hour
    }

    /// The minute field (0 to 59).
    pub fn minute(&self) -> u32 {
        self.minute
    }

    /// Fills the date, hour and minute fields, as picking them does.
    pub fn set_target(&mut self, date: NaiveDate, hour: u32, minute: u32) {
        self.date = date;
        self.hour = hour.min(23);
        self.minute = minute.min(59);
    }

    /// The instant the fields name.
    pub fn target_time(&self) -> Option<CaptureTime> {
        self.zone.resolve(self.date, self.hour, self.minute)
    }

    /// Points the fields at `time`.
    pub fn set_fields_from(&mut self, time: CaptureTime) {
        let local = self.zone.date_time(time);
        self.set_target(local.date(), local.hour(), local.minute());
    }

    /// The message after the last jump, such as 'No captures on 2025-05-13;
    /// showing nearest'. `None` if it landed as asked.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Sets or clears the message.
    pub fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }

    /// Makes the fields follow the selection when it changed since the last
    /// time (an arrow key, a click or a jump).
    pub fn follow(&mut self, selected: Option<CaptureTime>) {
        if selected != self.synced {
            self.synced = selected;
            if let Some(time) = selected {
                self.set_fields_from(time);
            }
        }
    }

    /// The local date a day jump aims at, or `None` for the hour jumps.
    pub fn target_day(&self, kind: QuickJump, current: CaptureTime) -> Option<NaiveDate> {
        let day = |time, back: bool| {
            let date = self.zone.date(time);
            if back {
                date.pred_opt()
            } else {
                date.succ_opt()
            }
        };
        match kind {
            QuickJump::Today => Some(self.zone.date(self.now())),
            QuickJump::Yesterday => day(self.now(), true),
            QuickJump::PreviousDay => day(current, true),
            QuickJump::NextDay => day(current, false),
            QuickJump::MinusHour | QuickJump::PlusHour => None,
        }
    }

    /// The date picker button. The widget edits a `jiff` date, so the date is
    /// converted in and back; an out-of-range date is left as it was.
    fn date_button(&mut self, ui: &mut egui::Ui) {
        let mut picked = jiff_date(self.date);
        // No calendar icon: it is an emoji that the default fonts may not have.
        ui.add(
            egui_extras::DatePickerButton::new(&mut picked)
                .id_salt("jump_date")
                .show_icon(false),
        );
        if let Some(date) = chrono_date(picked) {
            self.date = date;
        }
    }

    /// Draws the controls. Returns what the user chose, if anything.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<JumpAction> {
        let mut action = None;
        ui.horizontal_wrapped(|ui| {
            self.date_button(ui);
            ui.add(
                egui::DragValue::new(&mut self.hour)
                    .range(0..=23)
                    .custom_formatter(|n, _| format!("{n:02}")),
            )
            .on_hover_text("Hour (0 to 23)");
            ui.label(":");
            ui.add(
                egui::DragValue::new(&mut self.minute)
                    .range(0..=59)
                    .custom_formatter(|n, _| format!("{n:02}")),
            )
            .on_hover_text("Minute (0 to 59)");
            if ui.button(GO_LABEL).clicked() {
                action = Some(JumpAction::Go);
            }
            ui.separator();
            for (kind, label) in QUICK_JUMPS {
                if ui.button(label).clicked() {
                    action = Some(JumpAction::Quick(kind));
                }
            }
        });
        if let Some(message) = &self.message {
            ui.label(message);
        }
        action
    }
}

fn jiff_date(date: NaiveDate) -> jiff::civil::Date {
    let (y, m, d) = (date.year(), date.month(), date.day());
    jiff::civil::Date::new(
        i16::try_from(y).unwrap_or(1970),
        i8::try_from(m).unwrap_or(1),
        i8::try_from(d).unwrap_or(1),
    )
    .unwrap_or_default()
}

fn chrono_date(date: jiff::civil::Date) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        i32::from(date.year()),
        u32::try_from(date.month()).ok()?,
        u32::try_from(date.day()).ok()?,
    )
}

impl Default for JumpControls {
    fn default() -> Self {
        Self::new()
    }
}
