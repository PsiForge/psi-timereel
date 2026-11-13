//! The timeline screen (architecture sections 2.4 and 8): a horizontal strip
//! of thumbnails with markers for breaks in the recording, and a large view
//! of the selected capture with its local time, app name and window title.
//!
//! * The strip is virtualised: [`timereel_timeline::StripLayout`] finds the
//!   visible thumbnails by binary search, and only those are drawn. The
//!   strip follows the selection, which stays in the middle.
//! * [`timereel_timeline::plan_requests`] decides what to ask the loader for:
//!   the visible thumbnails plus the thumbnail prefetch, and the full image
//!   of the selected capture plus the full-image prefetch. Nothing is
//!   decoded on the UI thread; the loader calls back to repaint when a
//!   picture arrives.
//! * While the full image loads, the thumbnail is shown scaled up.
//! * Left/right arrow keys and the mouse wheel move the selection; so does
//!   a click on a thumbnail.
//! * The labels come from [`timereel_timeline::MetaStore`], decrypted in the
//!   background. Its lock hook and the loader's are registered with the
//!   [`Session`] ([`TimelineView::register_lock_hooks`]), and dropping the
//!   view frees the GPU textures, so locking clears everything.
//!
//! * Search: a box above the timeline filters the strip with a
//!   [`timereel_timeline::SearchFilter`] (case-insensitive substring of the
//!   app name or window title) on the metadata loaded so far, and the arrow
//!   keys, the wheel and the step buttons move from match to match. While the
//!   details are still being read the UI shows the progress. Clearing the box
//!   shows the full timeline again; the selection is never changed by the
//!   clearing, so the same capture stays selected. The query is not logged
//!   and not stored anywhere but in the view, so locking drops it.
//!
//! Nothing here writes to disk.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::load::SizedTexture;
use egui::{Color32, Key, Modifiers, Rect, Sense, TextureHandle, TextureOptions};
use timereel_crypto::UnlockedKey;
use timereel_image::RgbaImage;
use timereel_store::{CaptureTime, Store};
use timereel_timeline::{
    format_clock, format_date_time, format_gap, plan_requests, Decoder, ImageLoader, Index, Item,
    Kind, MetaEntry, MetaSource, MetaStore, SearchFilter, Status, StoreDecoder, StoreMetaSource,
    StripLayout, PREFETCH_THUMBS, REFRESH_INTERVAL,
};

use crate::session::Session;

/// Shown instead of the timeline when there are no captures.
pub const EMPTY_TIMELINE_MESSAGE: &str = "No captures yet.";
/// Shown while the metadata of the selected capture is being decrypted.
pub const LOADING_DETAILS_MESSAGE: &str = "Loading details…";
/// Shown when the metadata of the selected capture cannot be read.
pub const DETAILS_FAILED_MESSAGE: &str = "The details of this capture cannot be read.";
/// Shown when the full image of the selected capture cannot be loaded.
pub const IMAGE_FAILED_MESSAGE: &str = "This capture's image cannot be loaded";
/// Accessible text of the large image once the full image is shown.
pub const FULL_IMAGE_TEXT: &str = "Screenshot";
/// Accessible text of the large image while the thumbnail stands in.
pub const THUMB_IMAGE_TEXT: &str = "Screenshot (loading, showing the thumbnail)";
/// The prefix of a thumbnail's accessible label (followed by the time).
pub const CAPTURE_LABEL_PREFIX: &str = "Capture ";
/// The hint of the empty search box (also its accessible name).
pub const SEARCH_HINT: &str = "Search app or window title";
/// The prefix of the progress text while the details are being read.
pub const LOADING_PROGRESS_PREFIX: &str = "Reading details: ";
/// Shown when the search finds nothing in what is loaded so far.
pub const NO_MATCHES_MESSAGE: &str = "No matches";
/// The suffix of the match count (preceded by the number).
pub const MATCHES_SUFFIX: &str = " matches";
/// The label of the button that clears the search.
pub const CLEAR_SEARCH_LABEL: &str = "Clear search";
/// The prefix of a gap marker's text (followed by the length).
pub const GAP_LABEL_PREFIX: &str = "Gap: ";

const THUMB_SIZE: egui::Vec2 = egui::vec2(120.0, 68.0);
const TIME_LABEL_HEIGHT: f32 = 18.0;
const STRIP_HEIGHT: f32 = THUMB_SIZE.y + TIME_LABEL_HEIGHT + 6.0;
const SPACING: f32 = 6.0;
const GAP_WIDTH: f32 = 72.0;
/// Wheel distance (points) for one step; one notch of a normal wheel.
const WHEEL_STEP: f32 = 40.0;
/// Thumbnail textures kept (the visible ones and a margin).
const THUMB_TEXTURES: usize = 160;
/// Full-image textures kept (the selected capture and its neighbours).
const FULL_TEXTURES: usize = 8;

/// The text of a gap marker, for example `Gap: 25 min`.
pub fn gap_label(length_ms: i64) -> String {
    format!("{GAP_LABEL_PREFIX}{}", format_gap(length_ms))
}

/// What the timeline needs to find the captures.
#[derive(Debug, Clone)]
pub struct TimelineOptions {
    /// The capture store.
    pub store: Store,
    /// The capture interval, for the break markers (a break is more than
    /// 1.5 x this).
    pub interval: Duration,
}

impl TimelineOptions {
    /// The real store and the interval from the config (the default if the
    /// config cannot be read).
    ///
    /// # Errors
    /// Fails if the data directory cannot be determined.
    pub fn from_env() -> Result<Self, timereel_config::PathError> {
        let minutes = timereel_config::load()
            .map(|c| c.interval_minutes)
            .unwrap_or_else(|_| timereel_config::Config::default().interval_minutes);
        Ok(Self {
            store: Store::at_default_location()?,
            interval: Duration::from_secs(u64::from(minutes) * 60),
        })
    }
}

/// Textures of decoded pictures, with a limit. The ones farthest in time from
/// the selection go first.
struct TextureCache {
    limit: usize,
    map: HashMap<CaptureTime, TextureHandle>,
}

impl TextureCache {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            map: HashMap::new(),
        }
    }

    fn get_or_load(
        &mut self,
        ctx: &egui::Context,
        time: CaptureTime,
        image: &RgbaImage,
        selected: CaptureTime,
    ) -> egui::TextureId {
        if let Some(handle) = self.map.get(&time) {
            return handle.id();
        }
        let size = [image.width() as usize, image.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.data());
        let handle = ctx.load_texture("capture", color, TextureOptions::LINEAR);
        let id = handle.id();
        self.map.insert(time, handle);
        while self.map.len() > self.limit {
            let farthest = self
                .map
                .keys()
                .copied()
                .max_by_key(|t| t.0.abs_diff(selected.0));
            match farthest {
                Some(t) if t != time => self.map.remove(&t),
                _ => break,
            };
        }
        id
    }
}

/// The timeline of one unlock.
pub struct TimelineView {
    index: Index,
    store: Option<Store>,
    interval: Duration,
    layout: StripLayout,
    loader: ImageLoader,
    meta: MetaStore,
    selected: Option<CaptureTime>,
    thumbs: TextureCache,
    full: TextureCache,
    wheel: f32,
    last_refresh: Instant,
    /// The UTC offset of the newest metadata seen, for labels whose own
    /// metadata is not loaded yet.
    offset_hint: i32,
    visible: std::ops::Range<usize>,
    error: Option<String>,
    /// The text in the search box.
    query: String,
    filter: SearchFilter,
    /// The captures that match `filter`, oldest first (only while filtering).
    matches: Vec<CaptureTime>,
    /// `(metadata loaded, captures)` when `matches` was computed; `None`
    /// forces a new computation.
    matches_stamp: Option<(usize, usize)>,
    /// The query changed since `matches` was computed.
    query_changed: bool,
}

impl TimelineView {
    /// Opens the timeline of the captures in `options.store`, decrypting
    /// with the key on background threads. The newest capture is
    /// selected.
    pub fn open(options: &TimelineOptions, key: &UnlockedKey, ctx: &egui::Context) -> Self {
        let identity = key.identity();
        let store = options.store.clone();
        let (index, error) = match Index::load(&store) {
            Ok(index) => (index, None),
            Err(e) => (
                Index::new(),
                Some(format!("The captures cannot be listed: {e}")),
            ),
        };
        let decoder = Arc::new(StoreDecoder::new(store.clone(), identity.clone()));
        let meta = Arc::new(StoreMetaSource::new(store.clone(), identity.clone()));
        let mut view = Self::new(index, decoder, meta, options.interval, ctx);
        view.store = Some(store);
        view.error = error;
        view
    }

    /// A timeline over `index` with the given sources (tests use fakes).
    pub fn new(
        index: Index,
        decoder: Arc<dyn Decoder>,
        meta_source: Arc<dyn MetaSource>,
        interval: Duration,
        ctx: &egui::Context,
    ) -> Self {
        let (repaint_loader, repaint_meta) = (ctx.clone(), ctx.clone());
        let loader = ImageLoader::new(decoder, move || repaint_loader.request_repaint());
        let meta = MetaStore::new(meta_source, move || repaint_meta.request_repaint());
        meta.queue(index.times().iter().rev().copied().collect());
        let selected = index.last();
        if let Some(t) = selected {
            loader.jump(t);
        }
        let layout = Self::layout_of(&index, interval);
        Self {
            index,
            store: None,
            interval,
            layout,
            loader,
            meta,
            selected,
            thumbs: TextureCache::new(THUMB_TEXTURES),
            full: TextureCache::new(FULL_TEXTURES),
            wheel: 0.0,
            last_refresh: Instant::now(),
            offset_hint: 0,
            visible: 0..0,
            error: None,
            query: String::new(),
            filter: SearchFilter::new(""),
            matches: Vec::new(),
            matches_stamp: None,
            query_changed: false,
        }
    }

    fn layout_of(index: &Index, interval: Duration) -> StripLayout {
        StripLayout::new(index.times(), interval, THUMB_SIZE.x, GAP_WIDTH, SPACING)
    }

    /// Makes locking the session clear the image caches and the metadata.
    /// (Dropping the view also frees the textures.)
    pub fn register_lock_hooks<K>(&self, session: &mut Session<K>) {
        session.on_lock(self.loader.lock_hook());
        session.on_lock(self.meta.lock_hook());
    }

    /// The text in the search box.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sets the search text, as typing in the box does. An empty text shows
    /// the full timeline again; the selection stays as it is, except that a
    /// capture that does not match the new query moves to the nearest match.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
        self.apply_query();
    }

    fn apply_query(&mut self) {
        self.filter = SearchFilter::new(&self.query);
        self.matches_stamp = None;
        self.query_changed = true;
    }

    /// Whether a non-empty search is active.
    pub fn is_filtering(&self) -> bool {
        !self.filter.is_empty()
    }

    /// The captures the strip shows: the matches while searching, otherwise
    /// all of them.
    pub fn shown_times(&self) -> &[CaptureTime] {
        if self.is_filtering() {
            &self.matches
        } else {
            self.index.times()
        }
    }

    /// Computes the matches again if the query, the loaded metadata or the
    /// index changed, and lays out the strip for what is shown.
    fn update_matches(&mut self) {
        let stamp = (self.meta.len(), self.index.len());
        let needed = self.query_changed
            || self.matches_stamp.is_none()
            || (self.is_filtering() && self.matches_stamp != Some(stamp));
        if !needed {
            return;
        }
        let query_changed = std::mem::take(&mut self.query_changed);
        if self.is_filtering() {
            self.matches = self.meta.matching(self.index.times(), &self.filter);
            if query_changed {
                if let Some(sel) = self.selected {
                    if self.matches.binary_search(&sel).is_err() {
                        if let Some(near) = nearest_in(&self.matches, sel) {
                            self.select(near);
                        }
                    }
                }
            }
        } else {
            self.matches = Vec::new();
        }
        self.matches_stamp = Some(stamp);
        self.layout = self.layout_for_shown();
    }

    /// The strip layout of the shown captures. Break markers only make sense
    /// in the full timeline, so a search lays out without them.
    fn layout_for_shown(&self) -> StripLayout {
        let interval = if self.is_filtering() {
            Duration::from_secs(u64::from(u32::MAX))
        } else {
            self.interval
        };
        StripLayout::new(
            self.shown_times(),
            interval,
            THUMB_SIZE.x,
            GAP_WIDTH,
            SPACING,
        )
    }

    /// The selected capture.
    pub fn selected(&self) -> Option<CaptureTime> {
        self.selected
    }

    /// How many captures there are.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether there are no captures.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The captures (as indexes) the strip showed in the last frame.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        self.visible.clone()
    }

    /// The metadata held so far (for tests and the lock check).
    pub fn meta(&self) -> &MetaStore {
        &self.meta
    }

    /// Selects `time` (the nearest capture if it is not in the index).
    pub fn select(&mut self, time: CaptureTime) {
        let Some(time) = self.index.nearest(time) else {
            return;
        };
        let far = self.selected.map_or(true, |old| {
            let (a, b) = (self.position(old), self.position(time));
            a.abs_diff(b) > PREFETCH_THUMBS
        });
        if far {
            self.loader.jump(time);
        } else {
            self.loader.set_position(time);
        }
        self.selected = Some(time);
    }

    /// Moves the selection by `steps` captures, stopping at the ends. While
    /// searching it moves from match to match, skipping the others.
    pub fn step(&mut self, steps: isize) {
        let Some(sel) = self.selected else { return };
        if self.is_filtering() {
            let mut at = sel;
            for _ in 0..steps.unsigned_abs() {
                let keep = |t| self.meta.matches(t, &self.filter);
                let found = if steps > 0 {
                    self.index.next_matching(at, keep)
                } else {
                    self.index.prev_matching(at, keep)
                };
                match found {
                    Some(t) => at = t,
                    None => break,
                }
            }
            if at != sel {
                self.select(at);
            }
            return;
        }
        let to = self
            .position(sel)
            .saturating_add_signed(steps)
            .min(self.index.len().saturating_sub(1));
        if let Some(&t) = self.index.times().get(to) {
            self.select(t);
        }
    }

    /// The position of `time` among the shown captures (where it would be).
    fn position(&self, time: CaptureTime) -> usize {
        self.shown_times().partition_point(|t| *t < time)
    }

    /// Draws the timeline and handles its input.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Results come back through the caches; the channel only needs to be
        // drained.
        let _ = self.loader.poll();
        self.refresh_index();
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.index.is_empty() {
            ui.label(EMPTY_TIMELINE_MESSAGE);
            return;
        }
        self.search_bar(ui);
        self.update_matches();
        self.handle_input(ui);
        let Some(selected) = self.selected else {
            return;
        };

        let large_height =
            (ui.available_height() - STRIP_HEIGHT - ui.spacing().item_spacing.y).max(0.0);
        let width = ui.available_width();
        ui.allocate_ui(egui::vec2(width, large_height), |ui| {
            ui.set_min_size(egui::vec2(width, large_height));
            self.large_view(ui, selected);
        });
        self.strip(ui, selected);
    }

    /// The search box, the match count and the loading progress.
    fn search_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(SEARCH_HINT)
                    .desired_width(280.0),
            );
            if edit.changed() {
                self.apply_query();
            }
            let escape = edit.has_focus() && ui.input(|i| i.key_pressed(Key::Escape));
            if self.is_filtering() {
                if ui.button(CLEAR_SEARCH_LABEL).clicked() || escape {
                    self.set_query("");
                }
                let found = self.matches.len();
                if found == 0 {
                    ui.label(NO_MATCHES_MESSAGE);
                } else {
                    ui.label(format!("{found}{MATCHES_SUFFIX}"));
                }
            }
            let (done, total) = self.meta.progress();
            if done < total {
                ui.spinner();
                ui.label(format!("{LOADING_PROGRESS_PREFIX}{done} / {total}"));
            }
        });
    }

    /// Picks up new captures once a minute.
    fn refresh_index(&mut self) {
        let Some(store) = &self.store else { return };
        if self.last_refresh.elapsed() < REFRESH_INTERVAL {
            return;
        }
        self.last_refresh = Instant::now();
        let before = self.index.len();
        match self.index.refresh(store) {
            Ok(0) => self.error = None,
            Ok(_) => {
                self.error = None;
                let times = self.index.times();
                let new = times.get(before.min(times.len())..).unwrap_or_default();
                self.meta.queue(new.iter().rev().copied().collect());
                self.matches_stamp = None;
                self.layout = self.layout_for_shown();
                self.selected = self.selected.or_else(|| self.index.last());
            }
            Err(e) => self.error = Some(format!("The captures cannot be listed: {e}")),
        }
    }

    fn handle_input(&mut self, ui: &egui::Ui) {
        // The arrow keys belong to the search box while it has the focus.
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (left, right, scroll) = ui.input_mut(|i| {
            (
                !typing && i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                !typing && i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.smooth_scroll_delta,
            )
        });
        if left {
            self.step(-1);
        }
        if right {
            self.step(1);
        }
        // Wheel down or a swipe to the left goes forward in time.
        self.wheel -= scroll.x + scroll.y;
        let steps = (self.wheel / WHEEL_STEP).trunc();
        if steps != 0.0 {
            self.wheel -= steps * WHEEL_STEP;
            self.step(steps as isize);
        }
    }

    fn offset_of(&mut self, time: CaptureTime) -> i32 {
        if let Some(MetaEntry::Ready(m)) = self.meta.get(time) {
            self.offset_hint = m.tz_offset_min;
            return m.tz_offset_min;
        }
        self.offset_hint
    }

    fn large_view(&mut self, ui: &mut egui::Ui, selected: CaptureTime) {
        match self.meta.get(selected) {
            Some(MetaEntry::Ready(meta)) => {
                self.offset_hint = meta.tz_offset_min;
                ui.heading(format_date_time(selected, meta.tz_offset_min));
                ui.add(egui::Label::new(egui::RichText::new(&meta.app_name).strong()).truncate());
                ui.add(egui::Label::new(&meta.window_title).truncate());
            }
            Some(MetaEntry::Failed(_)) => {
                ui.heading(format_date_time(selected, self.offset_hint));
                ui.label(DETAILS_FAILED_MESSAGE);
            }
            None => {
                ui.heading(format_date_time(selected, self.offset_hint));
                ui.label(LOADING_DETAILS_MESSAGE);
            }
        }
        // Full image first, then its thumbnail, then the neighbours.
        let full = self.loader.request(Kind::Full, selected);
        let thumb = self.loader.request(Kind::Thumb, selected);
        self.request_prefetch(selected);

        let ctx = ui.ctx().clone();
        let shown = match (&full, &thumb) {
            (Status::Done(Item::Ready(img)), _) => Some((
                self.full.get_or_load(&ctx, selected, img, selected),
                img,
                true,
            )),
            (_, Status::Done(Item::Ready(img))) => Some((
                self.thumbs.get_or_load(&ctx, selected, img, selected),
                img,
                false,
            )),
            _ => None,
        };
        if let Status::Done(Item::Failed(msg)) = &full {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("{IMAGE_FAILED_MESSAGE}: {msg}"),
            );
        }
        match shown {
            Some((id, img, is_full)) => {
                let avail = ui.available_size();
                let (w, h) = (img.width() as f32, img.height() as f32);
                let scale = (avail.x / w).min(avail.y / h).max(0.0);
                let size = egui::vec2(w * scale, h * scale);
                let alt = if is_full {
                    FULL_IMAGE_TEXT
                } else {
                    THUMB_IMAGE_TEXT
                };
                ui.add(egui::Image::new(SizedTexture::new(id, size)).alt_text(alt));
            }
            None if matches!(full, Status::Pending) => {
                ui.spinner();
            }
            None => {}
        }
    }

    /// Asks the loader for the neighbours of the selected capture: full
    /// images for the selection and its prefetch neighbours.
    fn request_prefetch(&self, selected: CaptureTime) {
        let at = self.position(selected);
        let plan = plan_requests(at..at + 1, at, self.shown_times().len());
        for t in &self.shown_times()[plan.full] {
            self.loader.request(Kind::Full, *t);
        }
    }

    fn strip(&mut self, ui: &mut egui::Ui, selected: CaptureTime) {
        let width = ui.available_width();
        let (strip, _) = ui.allocate_exact_size(egui::vec2(width, STRIP_HEIGHT), Sense::hover());
        let at = self.position(selected);
        let scroll_x = self.layout.center(at) - width / 2.0;
        let visible = self.layout.visible(scroll_x, width);
        self.visible = visible.clone();
        let plan = plan_requests(visible.clone(), at, self.shown_times().len());
        for t in &self.shown_times()[plan.thumbs] {
            self.loader.request(Kind::Thumb, *t);
        }

        let ctx = ui.ctx().clone();
        let mut clicked = None;
        let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
        strip_ui.set_clip_rect(strip.intersect(ui.clip_rect()));
        let ui = &mut strip_ui;

        let times: Vec<CaptureTime> = self.shown_times()[visible.clone()].to_vec();
        for (n, time) in times.into_iter().enumerate() {
            let i = visible.start + n;
            let x = strip.left() + self.layout.left(i) - scroll_x;
            let thumb_rect = Rect::from_min_size(egui::pos2(x, strip.top()), THUMB_SIZE);
            let label_rect = Rect::from_min_size(
                egui::pos2(x, thumb_rect.bottom()),
                egui::vec2(THUMB_SIZE.x, TIME_LABEL_HEIGHT),
            );
            let clock = format_clock(time, self.offset_of(time));
            let hit = thumb_rect.union(label_rect).intersect(strip);
            let response =
                ui.interact(hit, ui.scope_id().with(("capture", time.0)), Sense::click());
            let is_selected = time == selected;
            let text = format!("{CAPTURE_LABEL_PREFIX}{clock}");
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::accesskit::Role::Button,
                    true,
                    is_selected,
                    text.clone(),
                )
            });
            if response.clicked() {
                clicked = Some(time);
            }
            let painter = ui.painter();
            match self.loader.request(Kind::Thumb, time) {
                Status::Done(Item::Ready(img)) => {
                    let id = self.thumbs.get_or_load(&ctx, time, &img, selected);
                    painter.image(
                        id,
                        thumb_rect,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                Status::Done(Item::Failed(_)) => {
                    painter.rect_filled(thumb_rect, 2.0, ui.visuals().faint_bg_color);
                    painter.text(
                        thumb_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "!",
                        egui::FontId::proportional(18.0),
                        ui.visuals().error_fg_color,
                    );
                }
                Status::Pending | Status::Closed => {
                    painter.rect_filled(thumb_rect, 2.0, ui.visuals().extreme_bg_color);
                }
            }
            if is_selected {
                painter.rect_stroke(
                    thumb_rect.expand(1.0),
                    2.0,
                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    egui::StrokeKind::Outside,
                );
            }
            painter.text(
                label_rect.center_top(),
                egui::Align2::CENTER_TOP,
                &clock,
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
        }

        for &(i, gap) in self.layout.gaps_near(visible) {
            let x = strip.left() + self.layout.gap_left(i) - scroll_x;
            let rect = Rect::from_min_size(
                egui::pos2(x, strip.top()),
                egui::vec2(self.layout.gap_width(), THUMB_SIZE.y),
            );
            ui.put(
                rect,
                egui::Label::new(
                    egui::RichText::new(gap_label(gap.length_ms()))
                        .small()
                        .weak(),
                )
                .selectable(false),
            );
        }

        if let Some(time) = clicked {
            self.select(time);
        }
    }
}

/// The element of `times` (sorted) closest in time to `t`; the earlier one on
/// a tie.
fn nearest_in(times: &[CaptureTime], t: CaptureTime) -> Option<CaptureTime> {
    let i = times.partition_point(|x| *x < t);
    let after = times.get(i).copied();
    let before = i.checked_sub(1).and_then(|i| times.get(i)).copied();
    match (before, after) {
        (Some(b), Some(a)) => Some(if t.0 - b.0 <= a.0 - t.0 { b } else { a }),
        (b, a) => b.or(a),
    }
}
