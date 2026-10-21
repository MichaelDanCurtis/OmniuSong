//! Bounded metadata overview. Placement previews are window-local; only the
//! existing clip editor and session worker can publish a musical edit.
use crate::{
    arrangement_math::{self, AxisInterval, ClipTiming, PlacementBasis, TempoRatio},
    arrangement_types::{PlacementValue, Snap, TICK_SCALE, TickFp, Viewport},
    clip_ui::{ClipContext, ClipDraft, ClipSelection},
};
use eframe::egui;
use omniusong_core::{ClipPatch, DocumentClipAddress, FieldChange, ProjectDocument, ValueChange};
use serde_json::{Number, Value};

const BAR: i128 = 3840 * TICK_SCALE;
const MAX_NOTE_SCAN: usize = 64 * 20_000;
const MAX_ITEMS: usize = 16_448;
const MAX_PAINTED_ITEMS: usize = 256;
const TRACK_LABEL_WIDTH: f32 = 154.0;
const MIN_LANE_WIDTH: f32 = 96.0;

pub(super) struct ArrangementAccess {
    pub allow_select: bool,
    pub allow_stage: bool,
}
pub(super) enum ArrangementAction {
    SelectTrack(String),
    SelectClip(ClipSelection),
    StagePlacement {
        expected: Box<ClipContext>,
        value: PlacementValue,
    },
    SaveClip,
    DiscardClip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheKey {
    project_id: String,
    revision: u64,
    tempo: u64,
}
#[derive(Clone)]
enum Settings {
    Legacy {
        duration: f64,
        start: u64,
        input: Option<f64>,
        output: Option<f64>,
        fade_in: Option<f64>,
        fade_out: Option<f64>,
    },
    Native {
        start: u64,
        count: u64,
        position: i64,
        rate: u32,
        fade_in: u64,
        fade_out: u64,
    },
}
impl Settings {
    fn basis(&self) -> Result<PlacementBasis, String> {
        match *self {
            Self::Legacy {
                duration,
                start,
                input,
                output,
                fade_in,
                fade_out,
            } => {
                let first = input.unwrap_or(0.0);
                let last = output.unwrap_or(duration);
                let fi = fade_in.unwrap_or(0.0);
                let fo = fade_out.unwrap_or(0.0);
                if ![duration, first, last, fi, fo]
                    .iter()
                    .all(|n| n.is_finite() && *n >= 0.0)
                    || start > 1_000_000_000
                    || first >= last
                    || last > duration
                    || last - first + 1e-9 < 0.001
                    || fi + fo > last - first + 1e-9
                {
                    return Err("Clip fields do not form a valid source window and fades; the existing draft is kept".into());
                }
                Ok(PlacementBasis::Legacy {
                    start_tick: start,
                    duration_seconds: last - first,
                })
            }
            Self::Native {
                start,
                count,
                position,
                rate,
                fade_in,
                fade_out,
            } => {
                if count == 0
                    || start
                        .checked_add(count)
                        .is_none_or(|end| end > i64::MAX as u64)
                    || fade_in.checked_add(fade_out).is_none_or(|sum| sum > count)
                {
                    return Err(
                        "Clip window and fades are invalid; the existing draft is kept".into(),
                    );
                }
                Ok(PlacementBasis::Native {
                    position_frame: position,
                    frame_count: count,
                    sample_rate: rate,
                })
            }
        }
    }
    fn effective(&self, draft: &ClipDraft) -> Result<Self, String> {
        let seconds = |patch: &FieldChange<Number>, old| match patch {
            FieldChange::Keep => Ok(old),
            FieldChange::Remove => Ok(None),
            FieldChange::Set { value } => value
                .as_f64()
                .filter(|n| n.is_finite())
                .map(Some)
                .ok_or("Draft seconds are not finite".to_owned()),
        };
        Ok(match (self, draft.patch()?) {
            (
                Self::Legacy {
                    duration,
                    start,
                    input,
                    output,
                    fade_in,
                    fade_out,
                },
                ClipPatch::LegacyAudio(patch),
            ) => Self::Legacy {
                duration: *duration,
                start: match patch.start_tick {
                    FieldChange::Keep => *start,
                    FieldChange::Remove => 0,
                    FieldChange::Set { value } => value,
                },
                input: seconds(&patch.trim_start_seconds, *input)?,
                output: seconds(&patch.trim_end_seconds, *output)?,
                fade_in: seconds(&patch.fade_in_seconds, *fade_in)?,
                fade_out: seconds(&patch.fade_out_seconds, *fade_out)?,
            },
            (
                Self::Native {
                    start,
                    count,
                    position,
                    rate,
                    fade_in,
                    fade_out,
                },
                ClipPatch::NativeAudio(patch),
            ) => {
                let (start, count) = match patch.source_window {
                    ValueChange::Keep => (*start, *count),
                    ValueChange::Set { value } => (value.start_frame, value.frame_count),
                };
                let position = match patch.position_frame {
                    ValueChange::Keep => *position,
                    ValueChange::Set { value } => value,
                };
                let (fade_in, fade_out) = match patch.fades {
                    FieldChange::Keep => (*fade_in, *fade_out),
                    FieldChange::Remove => (0, 0),
                    FieldChange::Set { value } => (value.in_frames, value.out_frames),
                };
                Self::Native {
                    start,
                    count,
                    position,
                    rate: *rate,
                    fade_in,
                    fade_out,
                }
            }
            _ => return Err("Clip draft and arrangement source have different units".into()),
        })
    }
}
fn timing(basis: PlacementBasis) -> ClipTiming {
    match basis {
        PlacementBasis::Legacy {
            start_tick,
            duration_seconds,
        } => ClipTiming::Legacy {
            start_tick,
            duration_seconds,
        },
        PlacementBasis::Native {
            position_frame,
            frame_count,
            sample_rate,
        } => ClipTiming::Native {
            position_frame,
            frame_count,
            sample_rate,
        },
    }
}
fn placed(basis: PlacementBasis, value: PlacementValue) -> Result<PlacementBasis, String> {
    match (basis, value) {
        (
            PlacementBasis::Legacy {
                duration_seconds, ..
            },
            PlacementValue::LegacyTick(start_tick),
        ) => Ok(PlacementBasis::Legacy {
            start_tick,
            duration_seconds,
        }),
        (
            PlacementBasis::Native {
                frame_count,
                sample_rate,
                ..
            },
            PlacementValue::NativeFrame(position_frame),
        ) => Ok(PlacementBasis::Native {
            position_frame,
            frame_count,
            sample_rate,
        }),
        _ => Err("Arrangement placement units changed".into()),
    }
}
fn same_position(basis: PlacementBasis, value: PlacementValue) -> bool {
    matches!((basis, value), (PlacementBasis::Legacy { start_tick, .. }, PlacementValue::LegacyTick(value)) if start_tick == value)
        || matches!((basis, value), (PlacementBasis::Native { position_frame, .. }, PlacementValue::NativeFrame(value)) if position_frame == value)
}

struct AudioItem {
    clip_id: Option<String>,
    settings: Settings,
    saved: AxisInterval,
    original: Option<AxisInterval>,
}
struct EventItem {
    clip_id: String,
    at: TickFp,
    start: u64,
    count: u64,
}
enum Item {
    Audio(AudioItem),
    Events(EventItem),
}
impl Item {
    fn interval(&self) -> AxisInterval {
        match self {
            Self::Audio(item) => item.saved,
            Self::Events(item) => AxisInterval {
                start: item.at,
                end: item.at,
            },
        }
    }
}
struct NoteSpan {
    count: usize,
    interval: Option<AxisInterval>,
    limited: bool,
}
struct TrackRow {
    id: String,
    name: String,
    locked: bool,
    muted: bool,
    items: Vec<Item>,
    notes: Option<NoteSpan>,
}
struct Cache {
    key: CacheKey,
    tempo: TempoRatio,
    rows: Vec<TrackRow>,
    extent: AxisInterval,
    loop_end: TickFp,
    notice: Option<String>,
}
fn number(raw: &Value, key: &str) -> Option<f64> {
    raw.get(key).and_then(Value::as_f64)
}
fn legacy_settings(raw: &Value, duration: f64) -> Settings {
    Settings::Legacy {
        duration,
        start: raw.get("startTick").and_then(Value::as_u64).unwrap_or(0),
        input: number(raw, "trimStartSeconds"),
        output: number(raw, "trimEndSeconds"),
        fade_in: number(raw, "fadeInSeconds"),
        fade_out: number(raw, "fadeOutSeconds"),
    }
}
fn native_settings(raw: &Value) -> Option<Settings> {
    Some(Settings::Native {
        start: raw["range"]["start"].as_u64()?,
        count: raw["range"]["count"].as_u64()?,
        position: raw["position"]["frame"].as_i64()?,
        rate: u32::try_from(raw["position"]["sampleRate"].as_u64()?).ok()?,
        fade_in: raw["fades"]["inFrames"].as_u64().unwrap_or(0),
        fade_out: raw["fades"]["outFrames"].as_u64().unwrap_or(0),
    })
}
fn include(extent: &mut AxisInterval, interval: AxisInterval) {
    extent.start.0 = extent.start.0.min(interval.start.0);
    extent.end.0 = extent.end.0.max(interval.end.0);
}
impl Cache {
    fn build(doc: &ProjectDocument) -> Result<Self, String> {
        let tempo = TempoRatio::from_bpm(doc.tempo())?;
        let loop_end = TickFp(i128::from(doc.bars()) * BAR);
        let mut extent = AxisInterval {
            start: TickFp(0),
            end: loop_end,
        };
        let mut rows = Vec::with_capacity(doc.tracks().len());
        let mut scanned = 0;
        let mut items = 0;
        let mut limited = false;
        for track in doc.tracks() {
            let mut row = TrackRow {
                id: track["id"].as_str().unwrap_or_default().into(),
                name: track["name"].as_str().unwrap_or_default().into(),
                locked: track["locked"] == true,
                muted: track["muted"] == true,
                items: Vec::new(),
                notes: None,
            };
            if doc.schema_version() == 2 && track["kind"] == "recording" {
                if let Some(clips) = track["recording"]["clips"].as_array() {
                    for raw in clips {
                        if items >= MAX_ITEMS {
                            limited = true;
                            break;
                        }
                        let id = raw["id"].as_str().unwrap_or_default().to_owned();
                        let item = if raw["range"]["unit"] == "audio_frames" {
                            let settings = native_settings(raw)
                                .ok_or("Invalid declared audio clip metadata")?;
                            let saved = arrangement_math::project_interval(
                                timing(settings.basis()?),
                                &tempo,
                            )?;
                            let original = raw
                                .get("editSource")
                                .and_then(|source| native_settings(&source["original"]))
                                .and_then(|settings| settings.basis().ok())
                                .and_then(|basis| {
                                    arrangement_math::project_interval(timing(basis), &tempo).ok()
                                });
                            Item::Audio(AudioItem {
                                clip_id: Some(id),
                                settings,
                                saved,
                                original,
                            })
                        } else {
                            let frame = raw["position"]["frame"]
                                .as_i64()
                                .ok_or("Invalid event marker placement")?;
                            let rate = u32::try_from(
                                raw["position"]["sampleRate"]
                                    .as_u64()
                                    .ok_or("Invalid event marker rate")?,
                            )
                            .map_err(|_| "Invalid event marker rate")?;
                            Item::Events(EventItem {
                                clip_id: id,
                                at: arrangement_math::project_point(frame, rate, &tempo)?,
                                start: raw["range"]["start"].as_u64().unwrap_or(0),
                                count: raw["range"]["count"].as_u64().unwrap_or(0),
                            })
                        };
                        include(&mut extent, item.interval());
                        row.items.push(item);
                        items += 1;
                    }
                }
            } else {
                if track["kind"] == "audio"
                    && let Some(raw) = track.get("audio").filter(|raw| raw.is_object())
                {
                    let duration =
                        number(raw, "durationSeconds").ok_or("Invalid legacy audio duration")?;
                    let settings = legacy_settings(raw, duration);
                    let saved =
                        arrangement_math::project_interval(timing(settings.basis()?), &tempo)?;
                    let original = raw
                        .get("editSource")
                        .map(|source| legacy_settings(&source["original"], duration))
                        .and_then(|settings| settings.basis().ok())
                        .and_then(|basis| {
                            arrangement_math::project_interval(timing(basis), &tempo).ok()
                        });
                    include(&mut extent, saved);
                    row.items.push(Item::Audio(AudioItem {
                        clip_id: None,
                        settings,
                        saved,
                        original,
                    }));
                    items += 1;
                }
                if let Some(notes) = track["notes"].as_array().filter(|notes| !notes.is_empty()) {
                    let mut span: Option<AxisInterval> = None;
                    let count = notes.len();
                    let available = MAX_NOTE_SCAN.saturating_sub(scanned);
                    let taken = count.min(available);
                    for note in notes.iter().take(taken) {
                        let start = note["startTick"].as_u64().unwrap_or(0);
                        let end = start.saturating_add(note["durationTicks"].as_u64().unwrap_or(0));
                        let interval = AxisInterval {
                            start: TickFp(i128::from(start) * TICK_SCALE),
                            end: TickFp(i128::from(end) * TICK_SCALE),
                        };
                        if let Some(span) = &mut span {
                            include(span, interval);
                        } else {
                            span = Some(interval);
                        }
                    }
                    scanned += taken;
                    limited |= taken != count;
                    if let Some(span) = span {
                        include(&mut extent, span);
                    }
                    row.notes = Some(NoteSpan {
                        count,
                        interval: span,
                        limited: taken != count,
                    });
                }
            }
            rows.push(row);
        }
        Ok(Self { key: CacheKey { project_id: doc.id().into(), revision: doc.revision(), tempo: doc.tempo().to_bits() }, tempo, rows, extent, loop_end,
            notice: limited.then(|| "Arrangement projection limit reached for dense content. Counts and exact clip selectors remain available; a partial note span is not the complete performance.".into()) })
    }
    fn selection(&self, row: &TrackRow, item: &AudioItem) -> ClipSelection {
        ClipSelection {
            project_id: self.key.project_id.clone(),
            address: match &item.clip_id {
                None => DocumentClipAddress::LegacyAudio {
                    track_id: row.id.clone(),
                },
                Some(clip_id) => DocumentClipAddress::NativeAudio {
                    track_id: row.id.clone(),
                    clip_id: clip_id.clone(),
                },
            },
        }
    }
    fn selected(&self, context: &ClipContext) -> Option<(&TrackRow, &AudioItem, PlacementBasis)> {
        let draft = context.draft.as_ref().filter(|draft| {
            draft.selection.project_id == self.key.project_id
                && draft.base_revision == self.key.revision
        })?;
        for row in &self.rows {
            for item in &row.items {
                if let Item::Audio(item) = item
                    && selected_item(&self.key.project_id, row, item, context)
                {
                    let basis = item
                        .settings
                        .effective(draft)
                        .and_then(|settings| settings.basis())
                        .ok()?;
                    return Some((row, item, basis));
                }
            }
        }
        None
    }
}

fn selected_item(project: &str, row: &TrackRow, item: &AudioItem, context: &ClipContext) -> bool {
    context.draft.as_ref().is_some_and(|draft| {
        draft.selection.project_id == project
            && match &draft.selection.address {
                DocumentClipAddress::LegacyAudio { track_id } => {
                    track_id == &row.id && item.clip_id.is_none()
                }
                DocumentClipAddress::NativeAudio { track_id, clip_id } => {
                    track_id == &row.id && item.clip_id.as_ref() == Some(clip_id)
                }
            }
    })
}

struct Drag {
    expected: ClipContext,
    key: CacheKey,
    basis: PlacementBasis,
    viewport: Viewport,
    start_pointer: egui::Pos2,
    lane: egui::Rect,
    clip_rect: egui::Rect,
    target: Result<PlacementValue, String>,
    moved: bool,
}
pub(super) struct ArrangementUi {
    cache: Option<Cache>,
    viewport: Viewport,
    snap: Snap,
    drag: Option<Drag>,
    error: Option<String>,
}
impl Default for ArrangementUi {
    fn default() -> Self {
        Self {
            cache: None,
            viewport: Viewport::new(TickFp(0), TickFp(4 * BAR)).expect("constant viewport"),
            snap: Snap::Off,
            drag: None,
            error: None,
        }
    }
}
fn viewport(origin: i128, span: i128) -> Result<Viewport, String> {
    let limit = arrangement_math::AXIS_LIMIT.0;
    let span = span.clamp(TICK_SCALE, 2 * limit);
    Viewport::new(TickFp(origin.clamp(-limit, limit - span)), TickFp(span))
}
fn fit(interval: AxisInterval) -> Result<Viewport, String> {
    let length = interval
        .end
        .0
        .checked_sub(interval.start.0)
        .ok_or("Arrangement extent overflow")?
        .max(TICK_SCALE);
    let margin = (length / 8).max(TICK_SCALE / 2);
    viewport(
        interval.start.0.saturating_sub(margin),
        length.saturating_add(margin.saturating_mul(2)),
    )
}
fn pointer_delta(
    start: egui::Pos2,
    current: egui::Pos2,
    lane: egui::Rect,
    view: Viewport,
) -> Result<TickFp, String> {
    let width = f64::from(lane.width());
    let pixels = f64::from(current.x) - f64::from(start.x);
    let delta = (pixels / width * view.span.0 as f64).round();
    let limit = 2 * arrangement_math::AXIS_LIMIT.0;
    if !width.is_finite() || width <= 0.0 || !delta.is_finite() || delta.abs() > limit as f64 {
        return Err("Pointer displacement exceeds the arrangement bounds".into());
    }
    Ok(TickFp(delta as i128))
}
fn key(raw: &str, max: usize) -> String {
    let mut shown = String::new();
    for character in raw.chars().take(max) {
        shown.extend(character.escape_debug());
    }
    if raw.chars().nth(max).is_some() {
        shown.push('…');
    }
    if shown.is_empty() {
        shown.push_str("\"\"");
    }
    shown
}
fn item_id(project: &str, track: &str, item: &AudioItem) -> egui::Id {
    egui::Id::unique(("arrangement-audio", project, track, item.clip_id.as_deref()))
}
fn placement_label(value: PlacementValue) -> String {
    match value {
        PlacementValue::LegacyTick(value) => format!("{value} ticks"),
        PlacementValue::NativeFrame(value) => format!("{value} frames"),
    }
}
fn basis_position(basis: PlacementBasis) -> PlacementValue {
    match basis {
        PlacementBasis::Legacy { start_tick, .. } => PlacementValue::LegacyTick(start_tick),
        PlacementBasis::Native { position_frame, .. } => {
            PlacementValue::NativeFrame(position_frame)
        }
    }
}
impl ArrangementUi {
    /// Called for every installed document, including same-revision refreshes.
    /// The UI revision is not a content hash; a fresh source must rebuild metadata.
    pub fn document_installed(&mut self) {
        self.cache = None;
        self.drag = None;
        self.error = None;
    }
    fn sync(
        &mut self,
        document: &ProjectDocument,
        context: &ClipContext,
        access: &ArrangementAccess,
        ui: &egui::Ui,
    ) {
        let changed = self.cache.as_ref().is_none_or(|cache| {
            cache.key.project_id != document.id()
                || cache.key.revision != document.revision()
                || cache.key.tempo != document.tempo().to_bits()
        });
        if changed {
            let new_project = self
                .cache
                .as_ref()
                .is_none_or(|cache| cache.key.project_id != document.id());
            self.drag = None;
            match Cache::build(document) {
                Ok(cache) => {
                    self.cache = Some(cache);
                    self.error = None;
                }
                Err(error) => {
                    self.cache = None;
                    self.error = Some(error);
                }
            }
            if new_project {
                self.viewport = Self::default().viewport;
            }
        }
        if self.drag.as_ref().is_some_and(|drag| {
            !access.allow_stage
                || drag.expected != *context
                || drag.viewport != self.viewport
                || self
                    .cache
                    .as_ref()
                    .is_none_or(|cache| cache.key != drag.key)
        }) || !ui.input(|input| input.focused)
            || ui.ctx().text_edit_focused()
            || ui.ctx().any_popup_open()
            || ui.input(|input| {
                input.events.iter().any(|event| {
                    matches!(event, egui::Event::PointerGone)
                        || matches!(
                            event,
                            egui::Event::Key {
                                key: egui::Key::Escape,
                                pressed: true,
                                ..
                            }
                        )
                })
            })
        {
            self.drag = None;
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        document: &ProjectDocument,
        selected_track: Option<&str>,
        context: &ClipContext,
        access: ArrangementAccess,
    ) -> Option<ArrangementAction> {
        self.sync(document, context, &access, ui);
        ui.heading("Arrangement");
        ui.small("Saved metadata · select an audio clip, then drag to stage placement · Save is explicit");
        let mut action = None;
        let before_view = self.viewport;
        let before_snap = self.snap;
        ui.horizontal_wrapped(|ui| {
            if ui.button("Fit all arrangement").clicked()
                && let Some(cache) = &self.cache
            {
                match fit(cache.extent) {
                    Ok(view) => self.viewport = view,
                    Err(error) => self.error = Some(error),
                }
            }
            let selected = self
                .cache
                .as_ref()
                .and_then(|cache| cache.selected(context));
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Fit selected clip"))
                .clicked()
                && let (Some(cache), Some((_, _, basis))) = (&self.cache, selected)
            {
                match arrangement_math::project_interval(timing(basis), &cache.tempo).and_then(fit)
                {
                    Ok(view) => self.viewport = view,
                    Err(error) => self.error = Some(error),
                }
            }
            for (label, numerator, denominator) in [
                ("Zoom arrangement in", 1, 2),
                ("Zoom arrangement out", 2, 1),
            ] {
                if ui.button(label).clicked() {
                    let span = self.viewport.span.0.saturating_mul(numerator) / denominator;
                    let origin = self
                        .viewport
                        .origin
                        .0
                        .saturating_add((self.viewport.span.0 - span) / 2);
                    match viewport(origin, span) {
                        Ok(view) => self.viewport = view,
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            for (label, sign) in [("View earlier", -1), ("View later", 1)] {
                if ui.button(label).clicked() {
                    match viewport(
                        self.viewport
                            .origin
                            .0
                            .saturating_add(sign * self.viewport.span.0 / 2),
                        self.viewport.span.0,
                    ) {
                        Ok(view) => self.viewport = view,
                        Err(error) => self.error = Some(error),
                    }
                }
            }
            if ui.button("View bar 1").clicked()
                && let Ok(view) = viewport(0, self.viewport.span.0)
            {
                self.viewport = view;
            }
            egui::ComboBox::new("arrangement-snap", "Placement snap")
                .selected_text(match self.snap {
                    Snap::Off => "Off",
                    Snap::Beat => "Beat",
                    Snap::SixteenthNote => "Sixteenth note",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.snap, Snap::Off, "Off");
                    ui.selectable_value(&mut self.snap, Snap::Beat, "Beat");
                    ui.selectable_value(&mut self.snap, Snap::SixteenthNote, "Sixteenth note");
                });
        });
        if before_view != self.viewport || before_snap != self.snap {
            self.drag = None;
        }
        let Some(cache) = &self.cache else {
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            return None;
        };
        if let Some(notice) = &cache.notice {
            ui.label(notice);
        }
        if ui.available_width() < TRACK_LABEL_WIDTH + MIN_LANE_WIDTH {
            ui.small("Arrangement view is narrow. Use the exact source selector and clip placement fields, or widen the window.");
        }
        ui.small(format!("View: {:.3} to {:.3} beats · saved loop ends at beat {:.3}; loop is a guide, not a crop",
            self.viewport.origin.0 as f64 / TICK_SCALE as f64 / 960.0,
            (self.viewport.origin.0 + self.viewport.span.0) as f64 / TICK_SCALE as f64 / 960.0,
            cache.loop_end.0 as f64 / TICK_SCALE as f64 / 960.0));
        let row_height = (ui.text_style_height(&egui::TextStyle::Body)
            + ui.text_style_height(&egui::TextStyle::Small)
            + 32.0)
            .max(64.0);
        let mut painted = 0;
        let mut latest_lane = None;
        let reserve_selected = context.draft.as_ref().is_some_and(|draft| {
            draft.selection.project_id == cache.key.project_id
                && draft.base_revision == cache.key.revision
        });
        egui::ScrollArea::vertical().id_salt(("arrangement-tracks", &cache.key.project_id)).min_scrolled_height(row_height).max_height(160.0)
            .show_rows(ui, row_height, cache.rows.len(), |ui, rows| {
                for row_index in rows {
                    let row = &cache.rows[row_index];
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), egui::Sense::hover());
                    let header = egui::Rect::from_min_max(rect.min, egui::pos2((rect.left() + TRACK_LABEL_WIDTH).min(rect.right()), rect.bottom()));
                    let lane = egui::Rect::from_min_max(egui::pos2(header.right() + 4.0, rect.top()), rect.max);
                    let response = ui.interact(header, egui::Id::unique(("arrangement-track", &cache.key.project_id, &row.id)), if access.allow_select { egui::Sense::click() } else { egui::Sense::hover() });
                    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, access.allow_select, selected_track == Some(row.id.as_str()), format!("Select arrangement track {} · key {}", key(&row.name, 48), key(&row.id, 64))));
                    let header_painter = ui.painter_at(header);
                    header_painter.rect_filled(header, 2.0, if selected_track == Some(row.id.as_str()) { ui.visuals().selection.bg_fill } else { ui.visuals().faint_bg_color });
                    header_painter.text(header.left_top() + egui::vec2(4.0, 6.0), egui::Align2::LEFT_TOP, key(&row.name, 20), egui::FontId::proportional(12.0), ui.visuals().text_color());
                    header_painter.text(header.left_bottom() + egui::vec2(4.0, -6.0), egui::Align2::LEFT_BOTTOM,
                        format!("{}{}{}", if row.locked { "Locked · " } else { "" }, if row.muted { "Muted · " } else { "" }, if let Some(notes) = &row.notes { format!("{} notes", notes.count) } else { format!("{} sources", row.items.len()) }), egui::FontId::proportional(10.0), ui.visuals().text_color());
                    if response.clicked() { action = Some(ArrangementAction::SelectTrack(row.id.clone())); }
                    if lane.width() < MIN_LANE_WIDTH { continue; }
                    paint_grid(ui, lane, self.viewport, cache.loop_end);
                    let mut visible = 0; let mut hit_count = 0; let mut hit_items = Vec::new(); let painted_before = painted;
                    for (index, item) in row.items.iter().enumerate() {
                        let selected = match item { Item::Audio(item) => selected_item(&cache.key.project_id, row, item, context), _ => false };
                        let effective = if selected { context.draft.as_ref().and_then(|draft| match item { Item::Audio(item) => item.settings.effective(draft).and_then(|settings| settings.basis()).ok(), _ => None })
                            .and_then(|basis| arrangement_math::project_interval(timing(basis), &cache.tempo).ok()) } else { None };
                        let current = self.viewport.project_interval(effective.unwrap_or_else(|| item.interval()), lane.width()).ok().flatten();
                        let saved = self.viewport.project_interval(item.interval(), lane.width()).ok().flatten();
                        let original = match item { Item::Audio(item) => item.original.and_then(|original| self.viewport.project_interval(original, lane.width()).ok().flatten()), _ => None };
                        if current.is_none() && saved.is_none() && original.is_none() { continue; }
                        visible += 1;
                        let hit_rect = current.map(|interval| item_rect(lane, interval.start, interval.end));
                        if hit_rect.is_some_and(|rect| rect.contains(ui.input(|input| input.pointer.hover_pos()).unwrap_or(egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY)))) { hit_count += 1; }
                        if !selected && painted >= MAX_PAINTED_ITEMS - usize::from(reserve_selected) { continue; }
                        painted += 1;
                        match item {
                            Item::Audio(_) => {
                                let color = if selected { ui.visuals().selection.stroke.color } else if ui.visuals().dark_mode { egui::Color32::from_rgb(80, 190, 165) } else { egui::Color32::from_rgb(0, 105, 85) };
                                if let Some(saved) = saved {
                                    ui.painter().rect_stroke(item_rect(lane, saved.start, saved.end), 2.0, egui::Stroke::new(if selected { 2.0 } else { 1.0 }, color), egui::StrokeKind::Inside);
                                }
                                if let Some(interval) = original {
                                    dotted(ui.painter(), egui::pos2(lane.left() + interval.start, lane.bottom() - 9.0), egui::pos2(lane.left() + interval.end, lane.bottom() - 9.0), ui.visuals().weak_text_color());
                                }
                                if selected { latest_lane = Some((lane, ui.clip_rect().intersect(lane))); }
                                if let Some(rect) = hit_rect { hit_items.push((index, rect)); }
                            }
                            Item::Events(item) => {
                                let Some(hit_rect) = hit_rect else { continue; };
                                let center = egui::pos2(hit_rect.center().x, hit_rect.center().y);
                                ui.painter().circle_stroke(center, 4.0, egui::Stroke::new(1.0, ui.visuals().text_color()));
                                ui.painter().text(egui::pos2(center.x + 5.0, center.y), egui::Align2::LEFT_CENTER, "Events", egui::FontId::proportional(10.0), ui.visuals().text_color());
                                let response = ui.interact(hit_rect, egui::Id::unique(("arrangement-events", &cache.key.project_id, &row.id, &item.clip_id)), egui::Sense::hover());
                                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, false, format!("Raw events {} · ordinals {} plus {} · duration unknown, read-only", key(&item.clip_id, 96), item.start, item.count)));
                            }
                        }
                    }
                    if let Some(notes) = &row.notes && let Some(span) = notes.interval && let Ok(Some(interval)) = self.viewport.project_interval(span, lane.width()) {
                        let y = lane.bottom() - 3.0;
                        ui.painter().line_segment([egui::pos2(lane.left() + interval.start, y), egui::pos2(lane.left() + interval.end, y)], egui::Stroke::new(2.0, ui.visuals().weak_text_color()));
                        let label = format!("{} retained notes · read-only activity span{}", notes.count, if notes.limited { " (projection limited)" } else { "" });
                        let response = ui.interact(egui::Rect::from_min_max(egui::pos2(lane.left(), y - 5.0), lane.max), egui::Id::unique(("arrangement-notes", &cache.key.project_id, &row.id)), egui::Sense::hover());
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, false, &label));
                    }
                    if visible > 1 || visible > painted - painted_before {
                        let drawn = painted - painted_before;
                        let label = if visible > drawn { format!("{visible} sources · {drawn} drawn (global limit {MAX_PAINTED_ITEMS}) · exact selector below") } else { format!("{visible} sources · exact selector below") };
                        ui.painter().text(lane.right_top() + egui::vec2(-3.0, 1.0), egui::Align2::RIGHT_TOP, &label, egui::FontId::proportional(9.0), ui.visuals().weak_text_color());
                        let response = ui.interact(egui::Rect::from_min_size(lane.min, egui::vec2(lane.width(), 14.0)), egui::Id::unique(("arrangement-density", &cache.key.project_id, &row.id)), egui::Sense::hover());
                        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, false, &label));
                    }
                    for (index, rect) in hit_items {
                        let Item::Audio(item) = &row.items[index] else { continue; };
                        let selected = selected_item(&cache.key.project_id, row, item, context);
                        let enabled = access.allow_select && (hit_count <= 1 || selected);
                        let response = ui.interact(rect, item_id(&cache.key.project_id, &row.id, item), if enabled { egui::Sense::click_and_drag() } else { egui::Sense::hover() });
                        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, enabled, selected,
                            format!("Select arrangement audio clip {} · track {}", item.clip_id.as_deref().map(|id| key(id, 96)).unwrap_or_else(|| "legacy".into()), key(&row.id, 200))));
                        if response.clicked() && !selected && enabled { action = Some(ArrangementAction::SelectClip(cache.selection(row, item))); }
                        if selected && access.allow_stage && !row.locked
                            && let Some(draft) = &context.draft && let Ok(basis) = item.settings.effective(draft).and_then(|settings| settings.basis()) {
                                if response.is_pointer_button_down_on() && ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary)) {
                                    response.request_focus();
                                    if let Some(start_pointer) = ui.input(|input| input.pointer.interact_pos()) {
                                        self.drag = Some(Drag { expected: context.clone(), key: cache.key.clone(), basis, viewport: self.viewport,
                                            start_pointer, lane, clip_rect: ui.clip_rect().intersect(lane), target: Ok(basis_position(basis)), moved: false });
                                        self.error = None;
                                    }
                                }
                                if response.dragged() && let Some(drag) = &mut self.drag { drag.moved = true; }
                                if response.has_focus() && !ui.ctx().text_edit_focused() && !ui.ctx().any_popup_open() {
                                    for (key, direction) in [(egui::Key::ArrowLeft, -1), (egui::Key::ArrowRight, 1)] {
                                        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                                            match arrangement_math::nudge_candidate(basis, direction, &cache.tempo, self.snap) { Ok(value) => action = Some(ArrangementAction::StagePlacement { expected: Box::new(context.clone()), value }), Err(error) => self.error = Some(error) }
                                        }
                                    }
                                }
                        }
                    }
                }
            });
        if let Some(drag) = &mut self.drag {
            let pointer = ui.input(|input| input.pointer.interact_pos());
            if latest_lane != Some((drag.lane, drag.clip_rect))
                || pointer.is_none_or(|pointer| !drag.clip_rect.contains(pointer))
            {
                self.drag = None;
            } else if let Some(pointer) = pointer {
                drag.target = pointer_delta(drag.start_pointer, pointer, drag.lane, drag.viewport)
                    .and_then(|delta| {
                        arrangement_math::drag_candidate(drag.basis, delta, &cache.tempo, self.snap)
                    });
                if let Ok(value) = drag.target
                    && let Ok(interval) = placed(drag.basis, value).and_then(|basis| {
                        arrangement_math::project_interval(timing(basis), &cache.tempo)
                    })
                    && let Ok(Some(interval)) =
                        self.viewport.project_interval(interval, drag.lane.width())
                {
                    let ghost = item_rect(drag.lane, interval.start, interval.end);
                    let painter = ui.painter_at(drag.clip_rect);
                    dashed(
                        &painter,
                        ghost.left_top(),
                        ghost.right_top(),
                        ui.visuals().selection.stroke.color,
                    );
                    dashed(
                        &painter,
                        ghost.left_bottom(),
                        ghost.right_bottom(),
                        ui.visuals().selection.stroke.color,
                    );
                }
                if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
                    let drag = self.drag.take().expect("drag exists");
                    match drag.target {
                        Ok(value) if drag.moved && !same_position(drag.basis, value) => {
                            action = Some(ArrangementAction::StagePlacement {
                                expected: Box::new(drag.expected),
                                value,
                            })
                        }
                        Ok(_) => {}
                        Err(error) => self.error = Some(error),
                    }
                }
            }
        }
        if let Some((row, _, basis)) = cache.selected(context) {
            ui.horizontal_wrapped(|ui| {
                let enabled = access.allow_stage && !row.locked && self.drag.is_none();
                for (label, direction) in [("Nudge clip earlier", -1), ("Nudge clip later", 1)] {
                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                        match arrangement_math::nudge_candidate(
                            basis,
                            direction,
                            &cache.tempo,
                            self.snap,
                        ) {
                            Ok(value) => {
                                action = Some(ArrangementAction::StagePlacement {
                                    expected: Box::new(context.clone()),
                                    value,
                                })
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                let dirty = context.draft.as_ref().is_some_and(ClipDraft::dirty);
                if ui
                    .add_enabled(
                        enabled && dirty,
                        egui::Button::new("Save arrangement placement"),
                    )
                    .clicked()
                {
                    action = Some(ArrangementAction::SaveClip);
                }
                if ui
                    .add_enabled(
                        access.allow_stage && dirty,
                        egui::Button::new("Discard arrangement placement"),
                    )
                    .clicked()
                {
                    action = Some(ArrangementAction::DiscardClip);
                }
            });
            ui.small(format!(
                "Current {} · {}",
                placement_label(basis_position(basis)),
                match self.snap {
                    Snap::Off => match basis {
                        PlacementBasis::Legacy { .. } => "Earlier/Later step: one tick",
                        PlacementBasis::Native { .. } => "Earlier/Later step: one frame",
                    },
                    Snap::Beat => "Earlier/Later: previous/next beat grid",
                    Snap::SixteenthNote => "Earlier/Later: previous/next sixteenth-note grid",
                }
            ));
            if let Ok(interval) = arrangement_math::project_interval(timing(basis), &cache.tempo)
                && let Ok(Some(interval)) = self
                    .viewport
                    .project_interval(interval, latest_lane.map_or(0.0, |(lane, _)| lane.width()))
                && let Some((lane, clip_rect)) = latest_lane
            {
                let rect = item_rect(lane, interval.start, interval.end);
                dashed(
                    &ui.painter_at(clip_rect),
                    rect.left_top() + egui::vec2(0.0, 3.0),
                    rect.right_top() + egui::vec2(0.0, 3.0),
                    ui.visuals().selection.stroke.color,
                );
            }
        } else if context.draft.is_some() {
            ui.small("Placement unavailable for invalid clip fields. The existing draft is kept; use its inspector to correct or discard it.");
        }
        ui.small("Solid = saved · dotted = permanent original · dashed = local draft/drag. Save/Discard applies to the whole clip draft, including inspector fields.");
        if let Some(row) = cache
            .rows
            .iter()
            .find(|row| selected_track == Some(row.id.as_str()))
        {
            ui.collapsing("Exact arrangement sources on selected track", |ui| {
                let exact_row_height = ui.spacing().interact_size.y.max(ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().button_padding.y);
                egui::ScrollArea::vertical().id_salt(("arrangement-exact", &cache.key.project_id, &row.id)).min_scrolled_height(exact_row_height).max_height(120.0_f32.max(exact_row_height))
                    .show_rows(ui, exact_row_height, row.items.len(), |ui, range| {
                        for index in range { match &row.items[index] {
                            Item::Audio(item) => { ui.scope_builder(egui::UiBuilder::new().scope_id(item_id(&cache.key.project_id, &row.id, item)), |ui| {
                                let label = format!("Select exact arrangement clip {}", item.clip_id.as_deref().map(|id| key(id, 96)).unwrap_or_else(|| "legacy audio".into()));
                                if ui.add_enabled(access.allow_select, egui::Button::new(&label).truncate()).on_hover_text(&label).clicked() { action = Some(ArrangementAction::SelectClip(cache.selection(row, item))); }
                            }); },
                            Item::Events(item) => { ui.add(egui::Label::new(format!("Raw events {} · {} ordinals from {} · duration unknown, read-only", key(&item.clip_id, 96), item.count, item.start)).truncate()); },
                        } }
                    });
            });
        }
        if let Some(drag) = &self.drag {
            match &drag.target {
                Ok(value) => {
                    ui.small(format!(
                        "Drag preview {} · release stages locally; Escape cancels",
                        placement_label(*value)
                    ));
                }
                Err(error) => {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if matches!(action, Some(ArrangementAction::StagePlacement { .. })) {
            self.error = None;
        }
        action
    }
}
fn item_rect(lane: egui::Rect, start: f32, end: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(lane.left() + start, lane.top() + 18.0),
        egui::pos2(
            (lane.left() + end)
                .max(lane.left() + start + 6.0)
                .min(lane.right()),
            lane.bottom() - 15.0,
        ),
    )
}
fn dashed(painter: &egui::Painter, start: egui::Pos2, end: egui::Pos2, color: egui::Color32) {
    let delta = end - start;
    let length = delta.length();
    if !length.is_finite() || length <= 0.0 {
        return;
    }
    let segments = (length / 7.0).ceil().clamp(1.0, 32.0) as usize;
    let stride = length / segments as f32;
    for segment in 0..segments {
        let offset = segment as f32 * stride;
        painter.line_segment(
            [
                start + delta / length * offset,
                start + delta / length * (offset + 4.0_f32.min(stride * 0.6)).min(length),
            ],
            egui::Stroke::new(1.0, color),
        );
    }
}
fn dotted(painter: &egui::Painter, start: egui::Pos2, end: egui::Pos2, color: egui::Color32) {
    let delta = end - start;
    let length = delta.length();
    if !length.is_finite() || length <= 0.0 {
        return;
    }
    let dots = (length / 6.0).ceil().clamp(1.0, 32.0) as usize;
    for dot in 0..=dots {
        painter.circle_filled(start + delta * (dot as f32 / dots as f32), 1.0, color);
    }
}
fn paint_grid(ui: &egui::Ui, rect: egui::Rect, view: Viewport, loop_end: TickFp) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let mut step = if view.span.0 < 240 * TICK_SCALE {
        TICK_SCALE
    } else if view.span.0 < 960 * TICK_SCALE {
        60 * TICK_SCALE
    } else {
        960 * TICK_SCALE
    };
    while view.span.0 / step > 64 {
        step = step.saturating_mul(2);
    }
    let first = view.origin.0.div_euclid(step);
    let last = (view.origin.0 + view.span.0).div_euclid(step);
    let label_stride = ((last - first + 1) / 8).max(1);
    for line in first..=last.min(first + 65) {
        let relative = line * step - view.origin.0;
        let x =
            rect.left() + (relative as f64 / view.span.0 as f64 * f64::from(rect.width())) as f32;
        if x < rect.left() || x > rect.right() {
            continue;
        }
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(0.5, ui.visuals().weak_text_color()),
        );
        if (line - first) % label_stride == 0 {
            let at = line * step;
            let label = if at >= 0 && at % BAR == 0 {
                format!("bar {}", at / BAR + 1)
            } else {
                format!("{} ticks", at / TICK_SCALE)
            };
            painter.text(
                egui::pos2(x + 2.0, rect.top() + 1.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(9.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
    for (at, label) in [(TickFp(0), "Bar 1"), (loop_end, "Loop end")] {
        if let Ok(Some(interval)) =
            view.project_interval(AxisInterval { start: at, end: at }, rect.width())
        {
            let x = rect.left() + interval.start;
            painter.text(
                egui::pos2(x + 2.0, rect.top()),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(9.0),
                ui.visuals().text_color(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip_ui::{ClipDraftFields, ClipEditor, FieldMode, ValueMode};
    use omniusong_core::{
        ClipTimelinePolicy, LegacyAudioClipPatch,
        contracts::TakeId,
        native::{TakeClip, TakeRange, TakeReference, TimelinePosition},
    };
    use serde_json::json;

    const LEGACY: &str = "\0arrangement legacy";
    const NATIVE: &str = "native-track";

    fn document() -> ProjectDocument {
        let mut value = ProjectDocument::new("Synthetic arrangement, no source files")
            .unwrap()
            .value()
            .clone();
        value["tempo"] = 120.into();
        value["tracks"].as_array_mut().unwrap().truncate(1);
        let track = &mut value["tracks"][0];
        track["id"] = LEGACY.into();
        track["name"] = "Legacy audio and retained notes".into();
        track["kind"] = "audio".into();
        track["assetId"] = format!("{}.wav", "a".repeat(64)).into();
        track["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":2.0,"byteLength":64,"startTick":960,"trimStartSeconds":0.125,"trimEndSeconds":1.75,"fadeInSeconds":0.1,"fadeOutSeconds":0.2});
        track["notes"] = json!([{"id":"retained-note","pitch":60,"startTick":18000,"durationTicks":100,"velocity16":32769}]);
        let mut doc = ProjectDocument::from_value(value)
            .unwrap()
            .upgrade_native()
            .unwrap();
        for (id, range, position) in [
            (
                "first",
                TakeRange::AudioFrames {
                    start: 100,
                    count: 12_000,
                },
                -480,
            ),
            (
                "second",
                TakeRange::AudioFrames {
                    start: 1100,
                    count: 11_000,
                },
                36_000,
            ),
            (
                "raw-max",
                TakeRange::Events { start: 4, count: 2 },
                i64::MAX,
            ),
        ] {
            doc.append_take_clip(
                NATIVE,
                "Declared native audio and raw events",
                TakeClip {
                    id: id.into(),
                    source: TakeReference {
                        take_id: TakeId("synthetic-unbound-take".into()),
                        manifest_sha256: "b".repeat(64),
                        stream_id: if id == "raw-max" { "raw" } else { "audio" }.into(),
                    },
                    range,
                    position: TimelinePosition {
                        frame: position,
                        sample_rate: 48_000,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
        }
        let mut value = doc.value().clone();
        value["tracks"][1]["notes"] =
            json!([null, {"startTick":"OPAQUE_RECORDING_EXTENSION","durationTicks":false}]);
        value["tracks"][1]["audio"] = json!({"durationSeconds":"OPAQUE_NOT_AUDIO"});
        ProjectDocument::from_value(value).unwrap()
    }
    fn legacy(doc: &ProjectDocument) -> ClipSelection {
        ClipSelection {
            project_id: doc.id().into(),
            address: DocumentClipAddress::LegacyAudio {
                track_id: LEGACY.into(),
            },
        }
    }
    fn native(doc: &ProjectDocument, id: &str) -> ClipSelection {
        ClipSelection {
            project_id: doc.id().into(),
            address: DocumentClipAddress::NativeAudio {
                track_id: NATIVE.into(),
                clip_id: id.into(),
            },
        }
    }
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|style| style.animation_time = 0.0);
        ctx
    }
    struct Harness {
        ctx: egui::Context,
        view: ArrangementUi,
        editor: ClipEditor,
        doc: ProjectDocument,
        clip_bottom: Option<f32>,
    }
    impl Harness {
        fn new() -> Self {
            let doc = document();
            let mut editor = ClipEditor::default();
            editor.select(&doc, legacy(&doc)).unwrap();
            Self {
                ctx: context(),
                view: ArrangementUi::default(),
                editor,
                doc,
                clip_bottom: None,
            }
        }
        fn frame(
            &mut self,
            events: Vec<egui::Event>,
        ) -> (egui::FullOutput, Option<ArrangementAction>) {
            self.frame_with(events, true, true, egui::vec2(1100.0, 850.0))
        }
        fn frame_with(
            &mut self,
            events: Vec<egui::Event>,
            allow: bool,
            focused: bool,
            size: egui::Vec2,
        ) -> (egui::FullOutput, Option<ArrangementAction>) {
            let mut action = None;
            let clip_context = self.editor.context();
            let selected = clip_context
                .draft
                .as_ref()
                .map(|draft| crate::clip_ui::track_id(&draft.selection.address));
            let mut output = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    focused,
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(bottom) = self.clip_bottom {
                        let mut rect = ui.clip_rect();
                        rect.max.y = bottom;
                        ui.set_clip_rect(rect);
                    }
                    action = self.view.show(
                        ui,
                        &self.doc,
                        selected,
                        &clip_context,
                        ArrangementAccess {
                            allow_select: allow,
                            allow_stage: allow,
                        },
                    );
                },
            );
            output.textures_delta.clear();
            (output, action)
        }
        fn settle(&mut self) {
            let _ = self.frame(vec![]);
            let _ = self.frame(vec![]);
        }
        fn audio_id(&self, selection: &ClipSelection) -> egui::Id {
            let clip = match &selection.address {
                DocumentClipAddress::LegacyAudio { .. } => None,
                DocumentClipAddress::NativeAudio { clip_id, .. } => Some(clip_id.as_str()),
            };
            egui::Id::unique((
                "arrangement-audio",
                self.doc.id(),
                crate::clip_ui::track_id(&selection.address),
                clip,
            ))
        }
        fn rect(&self, selection: &ClipSelection) -> egui::Rect {
            self.ctx
                .read_response(self.audio_id(selection))
                .expect("actual audio hit response")
                .rect
        }
        fn activate(&mut self, label: &str) -> Option<ArrangementAction> {
            self.settle();
            let (output, _) = self.frame(vec![]);
            let tree = output.platform_output.accesskit_update.unwrap();
            let target = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .unwrap_or_else(|| panic!("Missing {label}"))
                .0;
            self.frame(vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )])
            .1
        }
    }
    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    fn down(h: &mut Harness, at: egui::Pos2) {
        assert!(
            h.frame(vec![egui::Event::PointerMoved(at), button(at, true)])
                .1
                .is_none()
        );
    }
    fn labels(output: &egui::FullOutput) -> Vec<String> {
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().or(node.value()).map(str::to_owned))
            .collect()
    }
    fn stage(action: Option<ArrangementAction>) -> (ClipContext, PlacementValue) {
        match action {
            Some(ArrangementAction::StagePlacement { expected, value }) => (*expected, value),
            _ => panic!("Expected one local stage action"),
        }
    }

    #[test]
    fn metadata_keeps_legacy_note_activity_separate_and_events_at_max_are_points() {
        let doc = document();
        let original = doc.to_json().unwrap();
        let cache = Cache::build(&doc).unwrap();
        assert_eq!(cache.rows[0].items.len(), 1);
        let notes = cache.rows[0].notes.as_ref().unwrap();
        assert_eq!(notes.count, 1);
        assert!(!notes.limited);
        assert_eq!(notes.interval.unwrap().end, TickFp(18100 * TICK_SCALE));
        assert!(
            cache.rows[1].notes.is_none(),
            "Schema-2 recording extension notes stay opaque"
        );
        let Item::Events(events) = &cache.rows[1].items[2] else {
            panic!("Raw marker expected")
        };
        assert_eq!(events.start, 4);
        assert_eq!(events.count, 2);
        assert_eq!(
            events.at,
            arrangement_math::project_point(i64::MAX, 48_000, &cache.tempo).unwrap()
        );
        assert_eq!(
            cache.rows[1].items[2].interval().start,
            cache.rows[1].items[2].interval().end
        );
        assert_eq!(
            doc.to_json().unwrap(),
            original,
            "Metadata inspection cannot mutate original JSON"
        );
    }

    #[test]
    fn thirty_two_thousand_valid_notes_retain_the_last_extent_without_per_note_cache_entries() {
        let mut value = document().value().clone();
        value["tracks"].as_array_mut().unwrap().truncate(1);
        let mut base = value["tracks"][0].clone();
        base["kind"] = "notes".into();
        base.as_object_mut().unwrap().remove("audio");
        base.as_object_mut().unwrap().remove("assetId");
        let mut tracks = Vec::new();
        for track in 0..4 {
            let mut row = base.clone();
            row["id"] = format!("notes-{track}").into();
            row["notes"] = Value::Array((0..8_000).map(|note| json!({"id":format!("n-{note}"),"pitch":60,"startTick":if track == 3 && note == 7999 { 900_000_000 } else { 0 },"durationTicks":1,"velocity16":1})).collect());
            tracks.push(row);
        }
        value["tracks"] = tracks.into();
        let doc = ProjectDocument::from_value(value).unwrap();
        assert!(doc.to_json().unwrap().len() <= omniusong_core::native::MAX_NATIVE_PROJECT_BYTES);
        let cache = Cache::build(&doc).unwrap();
        assert_eq!(cache.extent.end, TickFp(900_000_001 * TICK_SCALE));
        assert!(cache.notice.is_none());
        assert!(cache.rows.iter().all(|row| {
            row.items.is_empty()
                && row
                    .notes
                    .as_ref()
                    .is_some_and(|notes| !notes.limited && notes.count == 8_000)
        }));
    }

    #[test]
    fn selected_drag_only_stages_on_release_and_keeps_trim_fades_and_document() {
        let mut h = Harness::new();
        h.settle();
        let selection = legacy(&h.doc);
        let at = h.rect(&selection).center();
        let before = h.editor.context();
        let raw = h.doc.to_json().unwrap();
        down(&mut h, at);
        assert!(h.view.drag.is_some());
        let end = at + egui::vec2(50.0, 0.0);
        assert!(h.frame(vec![egui::Event::PointerMoved(end)]).1.is_none());
        assert_eq!(h.editor.context(), before);
        assert_eq!(h.doc.to_json().unwrap(), raw);
        let (expected, value) = stage(h.frame(vec![button(end, false)]).1);
        assert_eq!(expected, before);
        assert!(matches!(value, PlacementValue::LegacyTick(value) if value > 960));
        h.editor.stage_placement(&h.doc, &expected, value).unwrap();
        let ClipDraftFields::LegacyAudio(before) = &before.draft.unwrap().fields else {
            panic!()
        };
        let ClipDraftFields::LegacyAudio(after) = &h.editor.draft.as_ref().unwrap().fields else {
            panic!()
        };
        assert_eq!(after.trim_start_seconds, before.trim_start_seconds);
        assert_eq!(after.trim_end_seconds, before.trim_end_seconds);
        assert_eq!(after.fade_in_seconds, before.fade_in_seconds);
        assert_eq!(after.fade_out_seconds, before.fade_out_seconds);
        assert_eq!(h.doc.to_json().unwrap(), raw);
    }

    #[test]
    fn unselected_pointer_gesture_never_selects_and_moves_together() {
        let mut h = Harness::new();
        h.editor.clear_selection();
        h.settle();
        let selection = legacy(&h.doc);
        let at = h.rect(&selection).center();
        down(&mut h, at);
        let end = at + egui::vec2(50.0, 0.0);
        assert!(h.frame(vec![egui::Event::PointerMoved(end)]).1.is_none());
        assert!(h.frame(vec![button(end, false)]).1.is_none());
        assert!(h.view.drag.is_none());
        h.settle();
        down(&mut h, at);
        match h.frame(vec![button(at, false)]).1 {
            Some(ArrangementAction::SelectClip(actual)) => assert_eq!(actual, selection),
            _ => panic!("A subsequent plain click must select exactly this clip"),
        }
    }

    #[test]
    fn unchanged_drag_with_snap_preserves_exact_modes_spelling_and_context() {
        let mut h = Harness::new();
        let ClipDraftFields::LegacyAudio(fields) = &mut h.editor.draft.as_mut().unwrap().fields
        else {
            panic!()
        };
        fields.start_tick.mode = FieldMode::Set;
        fields.start_tick.text = "0000961".into();
        h.view.snap = Snap::Beat;
        h.settle();
        let before = h.editor.context();
        let at = h.rect(&legacy(&h.doc)).center();
        down(&mut h, at);
        assert!(
            h.frame(vec![egui::Event::PointerMoved(at + egui::vec2(50.0, 0.0))])
                .1
                .is_none()
        );
        assert!(
            h.frame(vec![egui::Event::PointerMoved(at), button(at, false)])
                .1
                .is_none()
        );
        assert_eq!(h.editor.context(), before);
        assert!(h.view.drag.is_none());
    }

    #[test]
    fn click_with_one_pixel_jitter_does_not_stage_a_placement() {
        let mut h = Harness::new();
        h.settle();
        let before = h.editor.context();
        let at = h.rect(&legacy(&h.doc)).center();
        down(&mut h, at);
        let end = at + egui::vec2(1.0, 0.0);
        assert!(h.frame(vec![egui::Event::PointerMoved(end)]).1.is_none());
        assert!(h.frame(vec![button(end, false)]).1.is_none());
        assert_eq!(h.editor.context(), before);
        assert!(!h.editor.dirty());
    }

    #[test]
    fn gesture_cancels_on_escape_pointer_loss_focus_availability_transform_and_context() {
        for reason in 0..7 {
            let mut h = Harness::new();
            h.settle();
            let at = h.rect(&legacy(&h.doc)).center();
            down(&mut h, at);
            assert!(h.view.drag.is_some());
            let end = at + egui::vec2(30.0, 0.0);
            match reason {
                0 => {
                    let _ = h.frame(vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: Some(egui::Key::Escape),
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }]);
                }
                1 => {
                    let _ = h.frame(vec![egui::Event::PointerGone]);
                }
                2 => {
                    let _ = h.frame_with(vec![], true, false, egui::vec2(1100.0, 850.0));
                }
                3 => {
                    let _ = h.frame_with(vec![], false, true, egui::vec2(1100.0, 850.0));
                }
                4 => {
                    h.view.viewport = viewport(0, BAR).unwrap();
                    let _ = h.frame(vec![]);
                }
                5 => {
                    h.editor.select(&h.doc, native(&h.doc, "first")).unwrap();
                    let _ = h.frame(vec![]);
                }
                _ => {
                    let _ = h.frame_with(vec![], true, true, egui::vec2(900.0, 850.0));
                }
            }
            assert!(h.view.drag.is_none(), "Cancellation case {reason}");
            assert!(
                h.frame(vec![egui::Event::PointerMoved(end), button(end, false)])
                    .1
                    .is_none()
            );
        }
    }

    #[test]
    fn accessible_nudges_keep_exact_native_frames_above_double_precision_and_clear_errors() {
        let mut h = Harness::new();
        let position = 9_007_199_254_740_993i64;
        let mut value = h.doc.value().clone();
        value["tracks"][1]["recording"]["clips"][0]["position"]["frame"] = position.into();
        h.doc = ProjectDocument::from_value(value).unwrap();
        h.editor.select(&h.doc, native(&h.doc, "first")).unwrap();
        h.settle();
        h.view.error = Some("old rejected nudge".into());
        let (expected, value) = stage(h.activate("Nudge clip later"));
        assert_eq!(value, PlacementValue::NativeFrame(position + 1));
        assert_eq!(expected, h.editor.context());
        assert!(h.view.error.is_none());
        h.editor.stage_placement(&h.doc, &expected, value).unwrap();
        let (_, value) = stage(h.activate("Nudge clip earlier"));
        assert_eq!(value, PlacementValue::NativeFrame(position));
    }

    #[test]
    fn installed_document_invalidates_same_revision_source_and_live_gesture() {
        let mut h = Harness::new();
        h.settle();
        let at = h.rect(&legacy(&h.doc)).center();
        down(&mut h, at);
        let old_key = h.view.cache.as_ref().unwrap().key.clone();
        let mut value = h.doc.value().clone();
        value["tracks"][0]["audio"]["durationSeconds"] = 3.0.into();
        value["tracks"][0]["audio"]["trimEndSeconds"] = 2.75.into();
        h.doc = ProjectDocument::from_value(value).unwrap();
        h.editor.document_installed(&h.doc);
        h.view.document_installed();
        assert!(h.view.cache.is_none() && h.view.drag.is_none());
        h.settle();
        assert_eq!(h.view.cache.as_ref().unwrap().key, old_key);
        let Item::Audio(item) = &h.view.cache.as_ref().unwrap().rows[0].items[0] else {
            panic!()
        };
        assert!(matches!(
            item.settings,
            Settings::Legacy {
                duration: 3.0,
                output: Some(2.75),
                ..
            }
        ));
        assert!(h.frame(vec![button(at, false)]).1.is_none());
    }

    #[test]
    fn offscreen_draft_keeps_saved_outline_without_a_saved_geometry_hit_target() {
        let mut h = Harness::new();
        h.settle();
        h.view.viewport = viewport(0, 5000 * TICK_SCALE).unwrap();
        let ClipDraftFields::LegacyAudio(fields) = &mut h.editor.draft.as_mut().unwrap().fields
        else {
            panic!()
        };
        fields.start_tick.mode = FieldMode::Set;
        fields.start_tick.text = "30000".into();
        // New context prevents any cached response from an earlier pass being
        // mistaken for a current offscreen hit.
        h.ctx = context();
        let (output, _) = h.frame(vec![]);
        let selected_color = h.ctx.style_of(h.ctx.theme()).visuals.selection.stroke.color;
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.stroke.color == selected_color && rect.stroke.width == 2.0 && rect.rect.height() > 20.0)), "Saved solid outline must remain visible");
        assert!(
            h.ctx.read_response(h.audio_id(&legacy(&h.doc))).is_none(),
            "The old saved outline is never the offscreen draft's drag handle"
        );
    }

    #[test]
    fn original_only_visible_is_dotted_and_not_interactive() {
        let mut h = Harness::new();
        h.doc = h
            .doc
            .update_clip(
                &legacy(&h.doc).address,
                &ClipPatch::LegacyAudio(LegacyAudioClipPatch {
                    start_tick: FieldChange::Set { value: 30_000 },
                    trim_start_seconds: FieldChange::Keep,
                    trim_end_seconds: FieldChange::Keep,
                    fade_in_seconds: FieldChange::Keep,
                    fade_out_seconds: FieldChange::Keep,
                }),
                ClipTimelinePolicy::PreserveLoop,
            )
            .unwrap();
        h.editor.document_installed(&h.doc);
        h.settle();
        h.view.viewport = viewport(0, 5000 * TICK_SCALE).unwrap();
        h.ctx = context();
        let (output, _) = h.frame(vec![]);
        let dots = output.shapes.iter().filter(|shape| matches!(&shape.shape, egui::Shape::Circle(circle) if (circle.radius - 1.0).abs() < 0.01)).count();
        assert!(
            dots >= 2,
            "The permanent original remains visible even with current saved placement offscreen"
        );
        assert!(h.ctx.read_response(h.audio_id(&legacy(&h.doc))).is_none());
    }

    #[test]
    fn dense_sources_have_explicit_drawing_limit_and_bounded_canvas_accessibility() {
        let mut h = Harness::new();
        let mut value = h.doc.value().clone();
        let base = value["tracks"][1]["recording"]["clips"][0].clone();
        value["tracks"][1]["recording"]["clips"] = Value::Array(
            (0..800)
                .map(|index| {
                    let mut clip = base.clone();
                    clip["id"] = format!("dense-{index}").into();
                    clip["position"]["frame"] = 0.into();
                    clip
                })
                .collect(),
        );
        h.doc = ProjectDocument::from_value(value).unwrap();
        h.editor.clear_selection();
        h.settle();
        let (output, _) = h.frame(vec![]);
        let text = labels(&output);
        assert!(
            text.iter()
                .any(|text| text.contains("800 sources") && text.contains("global limit 256")),
            "Every visual omission must be disclosed: {text:?}"
        );
        let canvas_buttons = output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.label()
                    .is_some_and(|label| label.starts_with("Select arrangement audio clip"))
            })
            .count();
        assert!(
            canvas_buttons <= MAX_PAINTED_ITEMS,
            "Do not materialize all dense source widgets: {canvas_buttons}"
        );
    }

    #[test]
    fn invalid_same_target_fields_remain_intact_and_cannot_be_moved() {
        let mut h = Harness::new();
        h.editor.select(&h.doc, native(&h.doc, "first")).unwrap();
        let ClipDraftFields::NativeAudio(fields) = &mut h.editor.draft.as_mut().unwrap().fields
        else {
            panic!()
        };
        fields.position_mode = ValueMode::Set;
        fields.position_frame = "".into();
        let before = h.editor.context();
        let (output, action) = h.frame(vec![]);
        assert!(action.is_none());
        assert!(
            labels(&output)
                .iter()
                .any(|label| label.contains("Placement unavailable for invalid clip fields"))
        );
        assert_eq!(h.editor.context(), before);
        assert!(h.view.cache.as_ref().unwrap().selected(&before).is_none());
    }

    #[test]
    fn exact_selector_retains_full_ninety_six_byte_clip_identity_and_distinct_accessible_names() {
        let mut h = Harness::new();
        let prefix = "x".repeat(95);
        let first = format!("{prefix}a");
        let second = format!("{prefix}b");
        let mut value = h.doc.value().clone();
        value["tracks"][1]["recording"]["clips"]
            .as_array_mut()
            .unwrap()
            .truncate(2);
        value["tracks"][1]["recording"]["clips"][0]["id"] = first.clone().into();
        value["tracks"][1]["recording"]["clips"][1]["id"] = second.clone().into();
        h.doc = ProjectDocument::from_value(value).unwrap();
        h.editor.select(&h.doc, native(&h.doc, &first)).unwrap();
        assert!(
            h.activate("Exact arrangement sources on selected track")
                .is_none()
        );
        let (output, _) = h.frame(vec![]);
        let text = labels(&output);
        for id in [&first, &second] {
            assert!(
                text.iter()
                    .any(|text| text == &format!("Select exact arrangement clip {id}")),
                "Full exact key must be available to accessibility"
            );
        }
        match h.activate(&format!("Select exact arrangement clip {second}")) {
            Some(ArrangementAction::SelectClip(selection)) => {
                assert_eq!(selection, native(&h.doc, &second))
            }
            _ => panic!("Exact picker must select the full second key"),
        }
    }

    #[test]
    fn release_inside_geometric_row_but_outside_visible_clip_cancels() {
        let mut h = Harness::new();
        h.settle();
        let rect = h.rect(&legacy(&h.doc));
        // This is the same clip intersection used by a partially scrolled row,
        // established before press and unchanged throughout the real gesture.
        h.clip_bottom = Some(rect.center().y + 2.0);
        h.settle();
        let at = egui::pos2(rect.center().x, rect.top() + 3.0);
        down(&mut h, at);
        let drag = h
            .view
            .drag
            .as_ref()
            .expect("Press in the visible source area starts a gesture");
        let end = egui::pos2(at.x + 25.0, drag.clip_rect.bottom() + 2.0);
        assert!(drag.lane.contains(end));
        assert!(!drag.clip_rect.contains(end));
        assert!(
            h.frame(vec![egui::Event::PointerMoved(end), button(end, false)])
                .1
                .is_none()
        );
        assert!(h.view.drag.is_none());
        assert!(!h.editor.dirty());
    }

    #[test]
    fn nested_small_viewport_exposes_full_rows_and_virtualizes_all_sixty_four_tracks() {
        let mut h = Harness::new();
        let mut value = h.doc.value().clone();
        let base = value["tracks"][0].clone();
        value["tracks"] = Value::Array(
            (0..64)
                .map(|index| {
                    let mut track = base.clone();
                    track["id"] = format!("track-{index}").into();
                    track
                })
                .collect(),
        );
        h.doc = ProjectDocument::from_value(value).unwrap();
        h.editor.clear_selection();
        let mut final_output = None;
        let mut offset = 0.0;
        for _ in 0..7 {
            let mut output = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(650.0, 440.0),
                    )),
                    focused: true,
                    ..Default::default()
                },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        let outer = egui::ScrollArea::vertical()
                            .id_salt("arrangement-outer-test")
                            .stick_to_bottom(true)
                            .animated(false)
                            .show(ui, |ui| {
                                ui.add_space(900.0);
                                let _ = h.view.show(
                                    ui,
                                    &h.doc,
                                    None,
                                    &h.editor.context(),
                                    ArrangementAccess {
                                        allow_select: false,
                                        allow_stage: false,
                                    },
                                );
                            });
                        offset = outer.state.clamped_offset().y;
                    });
                },
            );
            output.textures_delta.clear();
            final_output = Some(output);
        }
        assert!(
            offset > 500.0,
            "The outer inspector must really be scrolled"
        );
        let output = final_output.unwrap();
        let lanes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.width() > MIN_LANE_WIDTH
                        && (rect.rect.height() - 64.0).abs() < 0.01
                        && rect.fill == h.ctx.style_of(h.ctx.theme()).visuals.extreme_bg_color =>
                {
                    Some((rect.rect, shape.clip_rect))
                }
                _ => None,
            })
            .collect();
        assert!(!lanes.is_empty());
        assert!(
            lanes.len() <= 5,
            "Track virtualization must bound paint: {}",
            lanes.len()
        );
        assert!(
            lanes.iter().any(|(rect, clip)| clip.contains_rect(*rect)),
            "One full measured track lane must be reachable: {lanes:?}"
        );
    }
}
