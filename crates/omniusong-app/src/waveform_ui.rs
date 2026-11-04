//! Read-only source overviews. Preparation belongs to the bounded waveform
//! worker; paint never opens storage or changes a project, draft or edit receipt.
use crate::clip_ui::{ClipDraft, ClipSelection, ClipSourceIdentity, clip};
use eframe::egui;
use omniusong_core::{
    AudioSourceWindow, ClipPatch, DocumentClipAddress, FieldChange, ProjectDocument, ValueChange,
    contracts::TakeId,
    native::{FadeCurve, TakeReference},
};
use omniusong_engine::{
    waveform::{WaveformClient, WaveformCompletion},
    waveform_types::{
        MAX_WAVEFORM_BINS, WaveformData, WaveformRequest, WaveformSettings, WaveformSourceKey,
    },
};
use serde_json::Number;
use std::path::{Path, PathBuf};

const CHANNEL_PLOT_HEIGHT: f32 = 78.0;

trait WaveformPort {
    fn submit(&mut self, request: WaveformRequest) -> Result<u64, String>;
    fn cancel(&mut self);
    fn poll(&mut self) -> Result<Option<WaveformCompletion>, String>;
    fn pending(&self) -> bool;
}
impl WaveformPort for WaveformClient {
    fn submit(&mut self, request: WaveformRequest) -> Result<u64, String> {
        self.try_submit(request)
    }
    fn cancel(&mut self) {
        WaveformClient::cancel(self);
    }
    fn poll(&mut self) -> Result<Option<WaveformCompletion>, String> {
        self.try_poll()
    }
    fn pending(&self) -> bool {
        WaveformClient::pending(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Placement {
    Ticks(u64),
    Frames(i64),
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Projection {
    window: AudioSourceWindow,
    placement: Placement,
    fade_in: u64,
    fade_out: u64,
    equal_power: bool,
    estimated: bool,
}
impl Projection {
    fn end(&self) -> u64 {
        self.window.start_frame + self.window.frame_count
    }
    fn summary(&self) -> String {
        let placement = match self.placement {
            Placement::Ticks(value) => format!("{value} ticks"),
            Placement::Frames(value) => format!("{value} frames"),
        };
        format!(
            "Source [{}, {}) · {} frames · fades {}/{} frames · arrangement placement {placement}{}",
            self.window.start_frame,
            self.end(),
            self.window.frame_count,
            self.fade_in,
            self.fade_out,
            if self.estimated {
                " · renderer-rounded estimate"
            } else {
                ""
            }
        )
    }
}

fn finite_seconds(value: f64, label: &str) -> Result<f64, String> {
    if value.is_finite() && (0.0..=86_400.0).contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{label} is outside the supported finite seconds range"
        ))
    }
}
fn rounded_frames(seconds: f64, rate: u32) -> Result<u64, String> {
    let frames = (seconds * f64::from(rate)).round();
    if !frames.is_finite() || frames < 0.0 || frames >= 18_446_744_073_709_551_616.0 {
        return Err("Seconds cannot be projected into source frames".into());
    }
    Ok(frames as u64)
}
fn project(settings: &WaveformSettings, data: &WaveformData) -> Result<Projection, String> {
    let (window, placement, fade_in, fade_out, equal_power, estimated) = match settings {
        WaveformSettings::Legacy {
            declared_duration_seconds,
            start_tick,
            trim_start_seconds,
            trim_end_seconds,
            fade_in_seconds,
            fade_out_seconds,
        } => {
            let duration = finite_seconds(*declared_duration_seconds, "Declared duration")?;
            let start = finite_seconds(trim_start_seconds.unwrap_or(0.0), "Source in")?;
            let end = finite_seconds(trim_end_seconds.unwrap_or(duration), "Source out")?;
            let input = finite_seconds(fade_in_seconds.unwrap_or(0.0), "Fade in")?;
            let output = finite_seconds(fade_out_seconds.unwrap_or(0.0), "Fade out")?;
            if data.source_start_frame != 0
                || duration < 0.001
                || *start_tick > 1_000_000_000
                || start >= end
                || end > duration
                || end - start + 1e-9 < 0.001
                || input + output > end - start + 1e-9
            {
                return Err("Effective legacy source window or fades are invalid".into());
            }
            let first = rounded_frames(start, data.sample_rate)?;
            let last = rounded_frames(end, data.sample_rate)?;
            let count = last
                .checked_sub(first)
                .filter(|count| *count > 0)
                .ok_or("The renderer-rounded source window contains no frames")?;
            (
                AudioSourceWindow {
                    start_frame: first,
                    frame_count: count,
                },
                Placement::Ticks(*start_tick),
                rounded_frames(input, data.sample_rate)?,
                rounded_frames(output, data.sample_rate)?,
                false,
                true,
            )
        }
        WaveformSettings::Native {
            window,
            position_frame,
            fades,
        } => {
            let count = i64::try_from(window.frame_count)
                .map_err(|_| "Native source count exceeds signed arrangement bounds")?;
            position_frame
                .checked_add(count)
                .ok_or("Native arrangement extent overflows")?;
            if fades.as_ref().is_some_and(|fade| {
                fade.in_frames
                    .checked_add(fade.out_frames)
                    .is_none_or(|sum| sum > window.frame_count)
            }) {
                return Err("Native fades must fit together inside the source window".into());
            }
            (
                window.clone(),
                Placement::Frames(*position_frame),
                fades.as_ref().map_or(0, |f| f.in_frames),
                fades.as_ref().map_or(0, |f| f.out_frames),
                fades
                    .as_ref()
                    .is_some_and(|f| f.curve == FadeCurve::EqualPower),
                false,
            )
        }
    };
    let end = window
        .start_frame
        .checked_add(window.frame_count)
        .ok_or("Source window overflows")?;
    let source_end = data
        .source_start_frame
        .checked_add(data.frame_count)
        .ok_or("Original source extent overflows")?;
    if window.frame_count == 0 || window.start_frame < data.source_start_frame || end > source_end {
        return Err("Effective source window or fades exceed the authenticated original".into());
    }
    Ok(Projection {
        window,
        placement,
        fade_in,
        fade_out,
        equal_power,
        estimated,
    })
}
fn number_change(change: &FieldChange<Number>, saved: Option<f64>) -> Result<Option<f64>, String> {
    match change {
        FieldChange::Keep => Ok(saved),
        FieldChange::Remove => Ok(None),
        FieldChange::Set { value } => value
            .as_f64()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| {
                "Draft seconds cannot be projected as a finite renderer estimate".into()
            }),
    }
}
fn effective_settings(
    saved: &WaveformSettings,
    patch: ClipPatch,
) -> Result<WaveformSettings, String> {
    Ok(match (saved, patch) {
        (
            WaveformSettings::Legacy {
                declared_duration_seconds,
                start_tick,
                trim_start_seconds,
                trim_end_seconds,
                fade_in_seconds,
                fade_out_seconds,
            },
            ClipPatch::LegacyAudio(patch),
        ) => WaveformSettings::Legacy {
            declared_duration_seconds: *declared_duration_seconds,
            start_tick: match patch.start_tick {
                FieldChange::Keep => *start_tick,
                FieldChange::Remove => 0,
                FieldChange::Set { value } => value,
            },
            trim_start_seconds: number_change(&patch.trim_start_seconds, *trim_start_seconds)?,
            trim_end_seconds: number_change(&patch.trim_end_seconds, *trim_end_seconds)?,
            fade_in_seconds: number_change(&patch.fade_in_seconds, *fade_in_seconds)?,
            fade_out_seconds: number_change(&patch.fade_out_seconds, *fade_out_seconds)?,
        },
        (
            WaveformSettings::Native {
                window,
                position_frame,
                fades,
            },
            ClipPatch::NativeAudio(patch),
        ) => WaveformSettings::Native {
            window: match patch.source_window {
                ValueChange::Keep => window.clone(),
                ValueChange::Set { value } => value,
            },
            position_frame: match patch.position_frame {
                ValueChange::Keep => *position_frame,
                ValueChange::Set { value } => value,
            },
            fades: match patch.fades {
                FieldChange::Keep => fades.clone(),
                FieldChange::Remove => None,
                FieldChange::Set { value } => Some(value),
            },
        },
        _ => return Err("Draft and authenticated waveform representations differ".into()),
    })
}

/// This is the renderer's endpoint/product-then-sine gain law, sampled only for
/// an explanatory outline. No source samples are modified or auditioned here.
fn envelope(projection: &Projection, index: u64) -> f32 {
    let mut gain = 1.0f64;
    if projection.fade_in > 0 && index < projection.fade_in {
        gain *= index as f64 / projection.fade_in as f64;
    }
    let remaining = projection.window.frame_count - 1 - index;
    if projection.fade_out > 0 && remaining < projection.fade_out {
        gain *= remaining as f64 / projection.fade_out as f64;
    }
    if projection.equal_power {
        (gain * std::f64::consts::FRAC_PI_2).sin() as f32
    } else {
        gain as f32
    }
}

struct Prepared {
    data: WaveformData,
    saved: Option<Projection>,
    saved_problem: Option<String>,
    original: Option<Projection>,
    original_problem: Option<String>,
    gap_frames: u64,
    scale: f32,
}
impl Prepared {
    fn new(data: WaveformData) -> Result<Self, String> {
        data.validate()?;
        match (&data.request.source, &data.saved) {
            (WaveformSourceKey::LegacyWav { .. }, WaveformSettings::Legacy { .. }) => {}
            (
                WaveformSourceKey::NativeAudio { sample_rate, .. },
                WaveformSettings::Native { .. },
            ) if *sample_rate == data.sample_rate && data.channels <= 16 => {}
            _ => {
                return Err(
                    "Waveform format or settings differ from the selected source representation"
                        .into(),
                );
            }
        }
        let (saved, saved_problem) = match project(&data.saved, &data) {
            Ok(saved) => (Some(saved), None),
            Err(error) => (None, Some(error)),
        };
        let (original, original_problem) = match data
            .original
            .as_ref()
            .map(|settings| project(settings, &data))
            .transpose()
        {
            Ok(original) => (original, None),
            Err(error) => (None, Some(error)),
        };
        let gap_frames = data.channel_bins[0].iter().map(|bin| bin.gap_frames).sum();
        let scale = data
            .channel_bins
            .iter()
            .flatten()
            .flat_map(|bin| [bin.min, bin.max])
            .flatten()
            .map(f32::abs)
            .fold(1.0f32, f32::max);
        Ok(Self {
            data,
            saved,
            saved_problem,
            original,
            original_problem,
            gap_frames,
            scale,
        })
    }
    fn draft(
        &self,
        draft: Option<&ClipDraft>,
        unresolved: bool,
    ) -> Result<Option<Projection>, String> {
        if unresolved {
            return Err("Retained clip outcome is unresolved; showing saved settings".into());
        }
        let Some(draft) = draft.filter(|draft| draft.dirty()) else {
            return Ok(None);
        };
        if draft.selection.project_id != self.data.request.project_id
            || draft.selection.address != self.data.request.address
            || draft.base_revision != self.data.request.expected_revision
            || !same_source(&draft.source, &self.data.request.source)
        {
            return Err(
                "Draft belongs to a different saved selection; showing saved settings".into(),
            );
        }
        let settings = effective_settings(&self.data.saved, draft.patch()?)?;
        project(&settings, &self.data).map(Some)
    }
}

fn same_source(draft: &ClipSourceIdentity, source: &WaveformSourceKey) -> bool {
    match (draft, source) {
        (
            ClipSourceIdentity::LegacyAudio {
                asset_id,
                source_asset_id,
            },
            WaveformSourceKey::LegacyWav {
                asset_id: actual,
                source_asset_id: ancestry,
            },
        ) => asset_id.as_ref() == Some(actual) && source_asset_id == ancestry,
        (
            ClipSourceIdentity::NativeAudio {
                take_id,
                manifest_sha256,
                stream_id,
                sample_rate,
            },
            WaveformSourceKey::NativeAudio {
                source,
                sample_rate: actual,
            },
        ) => {
            take_id == &source.take_id.0
                && manifest_sha256 == &source.manifest_sha256
                && stream_id == &source.stream_id
                && sample_rate == actual
        }
        _ => false,
    }
}

fn selection_request(
    document: Option<&ProjectDocument>,
    selection: Option<&ClipSelection>,
) -> Result<Option<WaveformRequest>, String> {
    let (Some(document), Some(selection)) = (document, selection) else {
        return Ok(None);
    };
    let Some(raw) = clip(document, selection) else {
        return Ok(None);
    };
    let source = match &selection.address {
        DocumentClipAddress::LegacyAudio { track_id } => {
            let track = document
                .tracks()
                .iter()
                .find(|track| track["id"].as_str() == Some(track_id))
                .ok_or("Selected audio track is absent")?;
            WaveformSourceKey::LegacyWav {
                asset_id: track["assetId"]
                    .as_str()
                    .ok_or("Selected audio has no playable original asset")?
                    .into(),
                source_asset_id: track["sourceAssetId"].as_str().map(str::to_owned),
            }
        }
        DocumentClipAddress::NativeAudio { .. } => WaveformSourceKey::NativeAudio {
            source: TakeReference {
                take_id: TakeId(
                    raw["source"]["takeId"]
                        .as_str()
                        .ok_or("Selected source take is absent")?
                        .into(),
                ),
                manifest_sha256: raw["source"]["manifestSha256"]
                    .as_str()
                    .ok_or("Selected source manifest is absent")?
                    .into(),
                stream_id: raw["source"]["streamId"]
                    .as_str()
                    .ok_or("Selected source stream is absent")?
                    .into(),
            },
            sample_rate: u32::try_from(
                raw["position"]["sampleRate"]
                    .as_u64()
                    .ok_or("Selected source rate is absent")?,
            )
            .map_err(|_| "Selected source rate is out of range")?,
        },
    };
    let request = WaveformRequest {
        project_id: document.id().into(),
        expected_revision: document.revision(),
        address: selection.address.clone(),
        source,
        bins: MAX_WAVEFORM_BINS,
    };
    request.validate()?;
    Ok(Some(request))
}

struct Active {
    ticket: u64,
    request: WaveformRequest,
    accept: bool,
}
pub(super) struct WaveformAccess {
    pub unresolved: bool,
    pub allow_start: bool,
}
#[derive(Default)]
pub(super) struct WaveformUi {
    client: Option<Box<dyn WaveformPort>>,
    root: Option<PathBuf>,
    selected: Option<WaveformRequest>,
    active: Option<Active>,
    prepared: Option<Prepared>,
    shown: bool,
    error: Option<String>,
    notice: Option<&'static str>,
}
impl WaveformUi {
    pub fn pending(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.pending())
    }
    fn sync(&mut self, document: Option<&ProjectDocument>, selection: Option<&ClipSelection>) {
        let (selected, problem) = match selection_request(document, selection) {
            Ok(selected) => (selected, None),
            Err(error) => (None, Some(error)),
        };
        if selected != self.selected {
            self.revoke();
            self.prepared = None;
            self.shown = false;
            self.notice = None;
            self.selected = selected;
            self.error = problem;
        } else if problem.is_some() {
            self.error = problem;
        }
    }
    fn revoke(&mut self) {
        if let Some(active) = &mut self.active {
            active.accept = false;
        }
        if let Some(client) = &mut self.client
            && client.pending()
        {
            client.cancel();
        }
    }
    pub fn poll(&mut self, document: Option<&ProjectDocument>, selection: Option<&ClipSelection>) {
        self.sync(document, selection);
        let Some(client) = &mut self.client else {
            return;
        };
        match client.poll() {
            Ok(Some(completion)) => {
                let Some(active) = self.active.take() else {
                    self.error =
                        Some("Ignored a waveform completion without an active request".into());
                    return;
                };
                if active.ticket != completion.ticket || active.request != completion.request {
                    self.active = Some(active);
                    self.error = Some("Ignored a mismatched waveform completion".into());
                    return;
                }
                if !active.accept
                    || !self.shown
                    || self.selected.as_ref() != Some(&completion.request)
                {
                    return;
                }
                let prepared = completion.result.and_then(|data| {
                    if data.request != completion.request {
                        return Err("Waveform result identifies a different source request".into());
                    }
                    Prepared::new(data)
                });
                match prepared {
                    Ok(prepared) => {
                        self.prepared = Some(prepared);
                        self.error = None;
                        self.notice = None;
                    }
                    Err(error) => {
                        self.prepared = None;
                        self.error = Some(error);
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.active = None;
                self.prepared = None;
                if self.shown {
                    self.error = Some(error);
                }
            }
        }
    }
    fn start(&mut self, root: &Path) {
        if self.pending() {
            return;
        }
        let Some(request) = self.selected.clone() else {
            return;
        };
        self.shown = true;
        self.prepared = None;
        self.notice = None;
        if self.root.as_ref().is_some_and(|existing| existing != root) {
            self.error = Some("Waveform worker belongs to a different studio location; reopen the application to change it".into());
            return;
        }
        if self.client.is_none() {
            match WaveformClient::spawn(root.to_path_buf()) {
                Ok(client) => {
                    self.client = Some(Box::new(client));
                    self.root = Some(root.to_path_buf());
                }
                Err(error) => {
                    self.error = Some(error);
                    return;
                }
            }
        }
        match self
            .client
            .as_mut()
            .expect("waveform client exists")
            .submit(request.clone())
        {
            Ok(ticket) => {
                self.active = Some(Active {
                    ticket,
                    request,
                    accept: true,
                });
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
            }
        }
    }
    fn hide(&mut self) {
        self.revoke();
        self.shown = false;
        self.prepared = None;
        self.error = None;
        self.notice = None;
    }
    fn cancel(&mut self) {
        self.revoke();
        self.prepared = None;
        self.error = None;
        self.notice = Some(
            "Waveform cancelled. The worker may still be finishing; no result will be displayed. Retry is explicit after it finishes.",
        );
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        root: &Path,
        document: &ProjectDocument,
        selection: Option<&ClipSelection>,
        draft: Option<&ClipDraft>,
        access: WaveformAccess,
    ) {
        self.sync(Some(document), selection);
        ui.separator();
        ui.heading("Original source waveform");
        let Some(request) = self.selected.as_ref() else {
            ui.label("Select a supported audio clip to inspect its original source.");
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            return;
        };
        let id = widget_id(request);
        let pending = self.pending();
        let mut show = false;
        let mut hide = false;
        let mut cancel = false;
        ui.scope_builder(egui::UiBuilder::new().scope_id(id), |ui| {
            ui.horizontal_wrapped(|ui| {
                if !self.shown {
                    show = ui
                        .add_enabled(
                            access.allow_start && !pending,
                            egui::Button::new("Show source waveform"),
                        )
                        .clicked();
                } else {
                    hide = ui.button("Hide source waveform").clicked();
                    if pending {
                        cancel = ui.button("Cancel waveform preparation").clicked();
                    }
                    if self.error.is_some() || self.notice.is_some() {
                        show = ui
                            .add_enabled(
                                access.allow_start && !pending,
                                egui::Button::new("Retry source waveform"),
                            )
                            .clicked();
                    }
                }
            });
        });
        if hide {
            self.hide();
        } else if cancel {
            self.cancel();
        } else if show {
            self.start(root);
        }
        if !self.shown {
            if pending {
                ui.small("The previous waveform job is finishing. Navigation and Close do not wait for it.");
            }
            return;
        }
        if pending && self.notice.is_none() {
            ui.label("Preparing an authenticated source overview in the background…");
        }
        if let Some(notice) = self.notice {
            ui.label(notice);
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let Some(prepared) = &self.prepared else {
            return;
        };
        ui.small("Raw original samples · no track gain, fades, normalization, resampling or draft audition");
        ui.label(format!(
            "{} channels · {} Hz · source [{}, {}) · {} frames",
            prepared.data.channels,
            prepared.data.sample_rate,
            prepared.data.source_start_frame,
            prepared.data.source_start_frame + prepared.data.frame_count,
            prepared.data.frame_count
        ));
        ui.small(format!(
            "Authenticated SHA-256: {}",
            prepared.data.authenticated_sha256
        ));
        ui.small(format!(
            "{} declared gap frames · amplitude scale ±{}",
            prepared.gap_frames, prepared.scale
        ));
        if prepared.gap_frames > 0 {
            ui.label("Hatched bins contain declared missing frames; extrema describe recorded content only. Gaps are not silence.");
        }
        if prepared.data.unknown_tail {
            ui.label("Unknown tail beyond the proven source range · no duration or waveform is invented for it.");
        }
        if let Some(saved) = &prepared.saved {
            ui.label(format!("Saved · solid line · {}", saved.summary()));
        } else if let Some(problem) = &prepared.saved_problem {
            ui.colored_label(ui.visuals().error_fg_color, format!("Saved overlay unavailable · {problem}. The authenticated source remains visible."));
        }
        if let Some(original) = &prepared.original {
            ui.label(format!(
                "Permanent original · dotted line · {}",
                original.summary()
            ));
        } else if let Some(problem) = &prepared.original_problem {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("Permanent original overlay unavailable: {problem}"),
            );
        } else {
            ui.small("No separate permanent edit snapshot is declared.");
        }
        let preview = match prepared.draft(draft, access.unresolved) {
            Ok(preview) => preview,
            Err(reason) => {
                ui.label(format!(
                    "Local preview unavailable · {reason}. Valid saved overlays remain visible."
                ));
                None
            }
        };
        if let Some(preview) = &preview {
            ui.label(format!(
                "Local draft preview · dashed line · {}",
                preview.summary()
            ));
        }
        ui.small("Window and gain outlines are visual projections. Save performs authoritative edit and source checks; these views do not play or change audio.");
        // A nested ScrollArea otherwise defaults to a 64-point minimum. That
        // clips a label plus plot even when the enclosing inspector is at its
        // bottom: only the inner viewport contributes to the outer extent.
        let channel_row_height = ui.text_style_height(&egui::TextStyle::Small)
            + ui.spacing().item_spacing.y
            + CHANNEL_PLOT_HEIGHT;
        egui::ScrollArea::vertical().id_salt(id).max_height(340.0)
            .min_scrolled_height(channel_row_height)
            .show_rows(ui, channel_row_height, prepared.data.channels as usize, |ui, rows| {
                for channel in rows {
                    ui.small(format!("Source channel {}", channel + 1));
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(64.0), CHANNEL_PLOT_HEIGHT), egui::Sense::hover());
                    response.accessible_name(format!("Original waveform channel {}. Min/max bins; {} gap frames. Source frames {} through {} exclusive.",
                        channel + 1, prepared.gap_frames, prepared.data.source_start_frame,
                        prepared.data.source_start_frame + prepared.data.frame_count));
                    paint_channel(ui, rect, prepared, channel, preview.as_ref());
                }
            });
    }
}

fn widget_id(request: &WaveformRequest) -> egui::Id {
    let (track, clip) = match &request.address {
        DocumentClipAddress::LegacyAudio { track_id } => (track_id.as_str(), ""),
        DocumentClipAddress::NativeAudio { track_id, clip_id } => {
            (track_id.as_str(), clip_id.as_str())
        }
    };
    let (kind, first, second, third, rate) = match &request.source {
        WaveformSourceKey::LegacyWav {
            asset_id,
            source_asset_id,
        } => (
            "legacy",
            asset_id.as_str(),
            source_asset_id.as_deref().unwrap_or(""),
            "",
            0,
        ),
        WaveformSourceKey::NativeAudio {
            source,
            sample_rate,
        } => (
            "native",
            source.take_id.0.as_str(),
            source.manifest_sha256.as_str(),
            source.stream_id.as_str(),
            *sample_rate,
        ),
    };
    egui::Id::unique((
        "source-waveform",
        &request.project_id,
        request.expected_revision,
        track,
        clip,
        kind,
        first,
        second,
        third,
        rate,
    ))
}
fn frame_x(rect: egui::Rect, origin: u64, count: u64, frame: u64) -> f32 {
    let offset = frame.saturating_sub(origin).min(count);
    rect.left() + (offset as f64 / count as f64 * f64::from(rect.width())) as f32
}
#[derive(Clone, Copy)]
enum LineStyle {
    Solid,
    Dotted,
    Dashed,
}
fn styled_segment(
    painter: &egui::Painter,
    first: egui::Pos2,
    second: egui::Pos2,
    stroke: egui::Stroke,
    style: LineStyle,
) {
    let (dash, gap) = match style {
        LineStyle::Solid => {
            painter.line_segment([first, second], stroke);
            return;
        }
        LineStyle::Dotted => (1.5, 3.5),
        LineStyle::Dashed => (7.0, 4.0),
    };
    let delta = second - first;
    let length = delta.length();
    if length <= 0.0 {
        return;
    }
    let direction = delta / length;
    let mut offset = 0.0;
    while offset < length {
        painter.line_segment(
            [
                first + direction * offset,
                first + direction * (offset + dash).min(length),
            ],
            stroke,
        );
        offset += dash + gap;
    }
}
fn paint_projection(
    painter: &egui::Painter,
    rect: egui::Rect,
    prepared: &Prepared,
    projection: &Projection,
    color: egui::Color32,
    style: LineStyle,
) {
    let x = |frame| {
        frame_x(
            rect,
            prepared.data.source_start_frame,
            prepared.data.frame_count,
            frame,
        )
    };
    let stroke = egui::Stroke::new(1.4, color);
    for frame in [projection.window.start_frame, projection.end()] {
        styled_segment(
            painter,
            egui::pos2(x(frame), rect.top()),
            egui::pos2(x(frame), rect.bottom()),
            stroke,
            style,
        );
    }
    for sign in [-1.0, 1.0] {
        let count = projection.window.frame_count;
        let segments = count.clamp(2, 96);
        let point = |step| {
            let index = ((count - 1) as u128 * step as u128 / (segments - 1) as u128) as u64;
            egui::pos2(
                x(projection.window.start_frame + index),
                rect.center().y
                    - sign * envelope(projection, index) / prepared.scale * rect.height() * 0.44,
            )
        };
        for step in 1..segments {
            styled_segment(painter, point(step - 1), point(step), stroke, style);
        }
    }
}
fn paint_channel(
    ui: &egui::Ui,
    rect: egui::Rect,
    prepared: &Prepared,
    channel: usize,
    preview: Option<&Projection>,
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.center().y),
            egui::pos2(rect.right(), rect.center().y),
        ],
        egui::Stroke::new(0.5, ui.visuals().weak_text_color()),
    );
    let x = |frame| {
        frame_x(
            rect,
            prepared.data.source_start_frame,
            prepared.data.frame_count,
            frame,
        )
    };
    let y = |sample: f32| rect.center().y - sample / prepared.scale * rect.height() * 0.44;
    let saved = egui::Color32::from_rgb(50, 125, 215);
    let original = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(240, 185, 80)
    } else {
        egui::Color32::from_rgb(125, 80, 0)
    };
    let draft = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(70, 225, 160)
    } else {
        egui::Color32::from_rgb(0, 105, 65)
    };
    for bin in &prepared.data.channel_bins[channel] {
        let left = x(bin.start_frame);
        let right = x(bin.start_frame + bin.frame_count);
        if bin.gap_frames > 0 {
            let gap_rect = egui::Rect::from_min_max(
                egui::pos2(left, rect.top()),
                egui::pos2(right.max(left + 1.0), rect.bottom()),
            );
            painter.rect_filled(
                gap_rect,
                0.0,
                ui.visuals().error_fg_color.gamma_multiply(0.12),
            );
            painter.line_segment(
                [gap_rect.left_bottom(), gap_rect.right_top()],
                egui::Stroke::new(0.6, ui.visuals().error_fg_color),
            );
        }
        if let (Some(min), Some(max)) = (bin.min, bin.max) {
            let center = (left + right) * 0.5;
            painter.line_segment(
                [egui::pos2(center, y(min)), egui::pos2(center, y(max))],
                egui::Stroke::new((right - left).clamp(0.5, 2.0), ui.visuals().text_color()),
            );
        }
    }
    if let Some(projection) = &prepared.saved {
        paint_projection(
            &painter,
            rect,
            prepared,
            projection,
            saved,
            LineStyle::Solid,
        );
    }
    if let Some(projection) = &prepared.original {
        paint_projection(
            &painter,
            rect,
            prepared,
            projection,
            original,
            LineStyle::Dotted,
        );
    }
    if let Some(projection) = preview {
        paint_projection(
            &painter,
            rect,
            prepared,
            projection,
            draft,
            LineStyle::Dashed,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip_ui::{ClipDraftFields, ClipEditor, FieldMode, ValueMode};
    use omniusong_core::native::{AudioFades, TakeClip, TakeRange, TimelinePosition};
    use omniusong_engine::waveform_types::WaveformBin;
    use serde_json::json;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    #[derive(Default)]
    struct Fake {
        pending: bool,
        next: u64,
        cancelled: usize,
        submitted: Vec<(u64, WaveformRequest)>,
        replies: VecDeque<Result<WaveformCompletion, String>>,
    }
    struct Port(Rc<RefCell<Fake>>);
    impl WaveformPort for Port {
        fn submit(&mut self, request: WaveformRequest) -> Result<u64, String> {
            let mut fake = self.0.borrow_mut();
            if fake.pending {
                return Err("Waveform request is still pending".into());
            }
            fake.next += 1;
            let ticket = fake.next;
            fake.pending = true;
            fake.submitted.push((ticket, request));
            Ok(ticket)
        }
        fn cancel(&mut self) {
            self.0.borrow_mut().cancelled += 1;
        }
        fn poll(&mut self) -> Result<Option<WaveformCompletion>, String> {
            let mut fake = self.0.borrow_mut();
            match fake.replies.pop_front() {
                Some(result) => {
                    fake.pending = false;
                    result.map(Some)
                }
                None => Ok(None),
            }
        }
        fn pending(&self) -> bool {
            self.0.borrow().pending
        }
    }
    fn injected() -> (WaveformUi, Rc<RefCell<Fake>>) {
        let fake = Rc::new(RefCell::new(Fake::default()));
        (
            WaveformUi {
                client: Some(Box::new(Port(fake.clone()))),
                root: Some(PathBuf::from("/synthetic/waveform/no-files")),
                ..Default::default()
            },
            fake,
        )
    }
    fn document(native: bool) -> (ProjectDocument, ClipSelection) {
        let mut doc = ProjectDocument::new("Synthetic waveform view").unwrap();
        let address = if native {
            doc = doc.upgrade_native().unwrap();
            doc.append_take_clip(
                "audio-track",
                "Synthetic native source",
                TakeClip {
                    id: "clip".into(),
                    source: TakeReference {
                        take_id: TakeId("synthetic-unbound-take".into()),
                        manifest_sha256: "a".repeat(64),
                        stream_id: "audio".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 16,
                    },
                    position: TimelinePosition {
                        frame: -3,
                        sample_rate: 8000,
                    },
                    fades: None,
                    edit_source: None,
                },
            )
            .unwrap();
            DocumentClipAddress::NativeAudio {
                track_id: "audio-track".into(),
                clip_id: "clip".into(),
            }
        } else {
            let mut value = doc.value().clone();
            value["tracks"][0]["id"] = "audio-track".into();
            value["tracks"][0]["kind"] = "audio".into();
            value["tracks"][0]["notes"] = json!([]);
            value["tracks"][0]["assetId"] = format!("{}.wav", "b".repeat(64)).into();
            value["tracks"][0]["audio"] = json!({"origin":"import","mimeType":"audio/wav","durationSeconds":0.002,"byteLength":64});
            doc = ProjectDocument::from_value(value).unwrap();
            DocumentClipAddress::LegacyAudio {
                track_id: "audio-track".into(),
            }
        };
        let selection = ClipSelection {
            project_id: doc.id().into(),
            address,
        };
        (doc, selection)
    }
    fn data(request: WaveformRequest, channels: u16, origin: u64, count: u64) -> WaveformData {
        let bins = (request.bins as u64).min(count) as usize;
        let channel_bins = (0..channels)
            .map(|channel| {
                (0..bins)
                    .map(|index| {
                        let start =
                            origin + (u128::from(count) * index as u128 / bins as u128) as u64;
                        let end = origin
                            + (u128::from(count) * (index + 1) as u128 / bins as u128) as u64;
                        WaveformBin {
                            start_frame: start,
                            frame_count: end - start,
                            content_frames: end - start,
                            gap_frames: 0,
                            min: Some(-0.25 - f32::from(channel) / 64.0),
                            max: Some(0.5 + f32::from(channel) / 32.0),
                        }
                    })
                    .collect()
            })
            .collect();
        let saved = match &request.source {
            WaveformSourceKey::LegacyWav { .. } => WaveformSettings::Legacy {
                declared_duration_seconds: count as f64 / 8000.0,
                start_tick: 0,
                trim_start_seconds: None,
                trim_end_seconds: None,
                fade_in_seconds: None,
                fade_out_seconds: None,
            },
            WaveformSourceKey::NativeAudio { .. } => WaveformSettings::Native {
                window: AudioSourceWindow {
                    start_frame: origin,
                    frame_count: count,
                },
                position_frame: -3,
                fades: None,
            },
        };
        let authenticated_sha256 = match &request.source {
            WaveformSourceKey::LegacyWav { asset_id, .. } => asset_id[..64].into(),
            WaveformSourceKey::NativeAudio { .. } => "c".repeat(64),
        };
        WaveformData {
            request,
            authenticated_sha256,
            sample_rate: 8000,
            channels,
            source_start_frame: origin,
            frame_count: count,
            unknown_tail: false,
            channel_bins,
            saved,
            original: None,
        }
    }
    fn request(doc: &ProjectDocument, selected: &ClipSelection) -> WaveformRequest {
        selection_request(Some(doc), Some(selected))
            .unwrap()
            .unwrap()
    }
    fn draft(doc: &ProjectDocument, selected: &ClipSelection) -> ClipDraft {
        let mut editor = ClipEditor::default();
        editor.select(doc, selected.clone()).unwrap();
        editor.draft.unwrap()
    }
    fn complete(fake: &Rc<RefCell<Fake>>, result: Result<WaveformData, String>) {
        let (ticket, request) = fake.borrow().submitted.last().unwrap().clone();
        fake.borrow_mut().replies.push_back(Ok(WaveformCompletion {
            ticket,
            request,
            result,
        }));
    }
    fn frame(
        ctx: &egui::Context,
        waveform: &mut WaveformUi,
        doc: &ProjectDocument,
        selected: &ClipSelection,
        draft: Option<&ClipDraft>,
        allow: bool,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        waveform.poll(Some(doc), Some(selected));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                waveform.ui(
                    ui,
                    Path::new("/synthetic/waveform/no-files"),
                    doc,
                    Some(selected),
                    draft,
                    WaveformAccess {
                        unresolved: false,
                        allow_start: allow,
                    },
                )
            },
        );
        output.textures_delta.clear();
        output
    }
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.all_styles_mut(|styles| styles.animation_time = 0.0);
        ctx
    }
    fn click(
        ctx: &egui::Context,
        waveform: &mut WaveformUi,
        doc: &ProjectDocument,
        selected: &ClipSelection,
        label: &str,
        allow: bool,
    ) {
        let _ = frame(ctx, waveform, doc, selected, None, allow, vec![]);
        let tree = frame(ctx, waveform, doc, selected, None, allow, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap_or_else(|| panic!("Missing {label}"))
            .0;
        let _ = frame(
            ctx,
            waveform,
            doc,
            selected,
            None,
            allow,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
        );
    }

    #[test]
    fn native_large_absolute_origin_and_negative_placement_do_not_lose_local_frame_geometry() {
        let (doc, selected) = document(true);
        let origin = 9_007_199_254_740_997;
        let mut overview = data(request(&doc, &selected), 2, origin, 64);
        overview.saved = WaveformSettings::Native {
            window: AudioSourceWindow {
                start_frame: origin + 5,
                frame_count: 20,
            },
            position_frame: -9_007_199_254_740_993,
            fades: Some(AudioFades {
                in_frames: 2,
                out_frames: 3,
                curve: FadeCurve::EqualPower,
            }),
        };
        let projection = project(&overview.saved, &overview).unwrap();
        assert_eq!(projection.window.start_frame, origin + 5);
        assert_eq!(projection.end(), origin + 25);
        assert!(projection.summary().contains("-9007199254740993 frames"));
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 0.0), egui::vec2(1024.0, 80.0));
        assert_eq!(
            frame_x(rect, origin, 64, origin + 2) - frame_x(rect, origin, 64, origin + 1),
            16.0
        );
    }

    #[test]
    fn legacy_remove_end_uses_declared_duration_and_renderer_half_frame_rounding() {
        let (doc, selected) = document(false);
        let mut overview = data(request(&doc, &selected), 1, 0, 80);
        overview.saved = WaveformSettings::Legacy {
            declared_duration_seconds: 0.008,
            start_tick: 17,
            trim_start_seconds: None,
            trim_end_seconds: Some(0.005),
            fade_in_seconds: None,
            fade_out_seconds: None,
        };
        let mut editing = draft(&doc, &selected);
        let ClipDraftFields::LegacyAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.trim_start_seconds.mode = FieldMode::Set;
        fields.trim_start_seconds.text = "0.0000625".into();
        fields.trim_end_seconds.mode = FieldMode::Remove;
        fields.fade_in_seconds.mode = FieldMode::Set;
        fields.fade_in_seconds.text = "0.0003125".into();
        let raw = editing.clone();
        let prepared = Prepared::new(overview).unwrap();
        let projection = prepared.draft(Some(&editing), false).unwrap().unwrap();
        assert_eq!(
            projection.window,
            AudioSourceWindow {
                start_frame: 1,
                frame_count: 63
            }
        );
        assert_eq!(
            projection.end(),
            64,
            "Measured 80-frame source must not replace the declared end default"
        );
        assert_eq!(projection.fade_in, 3);
        assert_eq!(projection.placement, Placement::Ticks(17));
        assert!(projection.estimated);
        assert_eq!(
            editing, raw,
            "Projection cannot normalize exact inspector text"
        );
    }

    #[test]
    fn equal_power_outline_uses_renderer_endpoint_and_product_before_sine_law() {
        let projection = Projection {
            window: AudioSourceWindow {
                start_frame: 100,
                frame_count: 5,
            },
            placement: Placement::Frames(0),
            fade_in: 4,
            fade_out: 4,
            equal_power: true,
            estimated: false,
        };
        assert_eq!(envelope(&projection, 0), 0.0);
        assert_eq!(envelope(&projection, 4), 0.0);
        assert!(
            (envelope(&projection, 2) - (std::f64::consts::FRAC_PI_2 * 0.25).sin() as f32).abs()
                < 1e-6
        );
        assert!(
            (envelope(&projection, 2) - 0.5).abs() > 0.1,
            "Do not multiply independently sine-shaped fades"
        );
    }

    #[test]
    fn core_valid_legacy_seconds_can_have_overlapping_independently_rounded_fades() {
        let (doc, selected) = document(false);
        let mut overview = data(request(&doc, &selected), 1, 0, 80);
        overview.saved = WaveformSettings::Legacy {
            declared_duration_seconds: 0.01,
            start_tick: 0,
            trim_start_seconds: Some(0.0000625),
            trim_end_seconds: Some(0.0013125),
            fade_in_seconds: Some(0.000625),
            fade_out_seconds: Some(0.000625),
        };
        let projection = project(&overview.saved, &overview).unwrap();
        assert_eq!(projection.window.frame_count, 10);
        assert_eq!(projection.fade_in + projection.fade_out, 10);
        let mut shifted = overview.saved.clone();
        if let WaveformSettings::Legacy {
            trim_start_seconds,
            trim_end_seconds,
            fade_in_seconds,
            fade_out_seconds,
            ..
        } = &mut shifted
        {
            *trim_start_seconds = Some(0.000125);
            *trim_end_seconds = Some(0.00125);
            *fade_in_seconds = Some(0.0005625);
            *fade_out_seconds = Some(0.0005625);
        }
        let projection = project(&shifted, &overview).unwrap();
        assert_eq!(projection.window.frame_count, 9);
        assert_eq!(
            projection.fade_in + projection.fade_out,
            10,
            "Seconds-valid fades retain renderer rounding rather than rescaling"
        );
    }

    #[test]
    fn invalid_native_combined_fades_bounds_and_unresolved_drafts_fall_back_to_saved() {
        let (doc, selected) = document(true);
        let prepared = Prepared::new(data(request(&doc, &selected), 2, 100, 16)).unwrap();
        let mut editing = draft(&doc, &selected);
        let ClipDraftFields::NativeAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.fades_mode = FieldMode::Set;
        fields.fade_in_frames = "10".into();
        fields.fade_out_frames = "10".into();
        assert!(
            prepared
                .draft(Some(&editing), false)
                .unwrap_err()
                .contains("fit together")
        );
        let ClipDraftFields::NativeAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.fades_mode = FieldMode::Keep;
        fields.window_mode = ValueMode::Set;
        fields.source_start_frame = "99".into();
        assert!(prepared.draft(Some(&editing), false).is_err());
        let ClipDraftFields::NativeAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.window_mode = ValueMode::Keep;
        fields.position_mode = ValueMode::Set;
        fields.position_frame.clear();
        assert!(prepared.draft(Some(&editing), false).is_err());
        assert!(
            prepared
                .draft(Some(&editing), true)
                .unwrap_err()
                .contains("unresolved")
        );
        assert_eq!(
            prepared.saved.as_ref().unwrap().window,
            AudioSourceWindow {
                start_frame: 100,
                frame_count: 16
            }
        );
    }

    #[test]
    fn syntactically_valid_legacy_patch_cannot_preview_invalid_combined_or_actual_source_window() {
        let (doc, selected) = document(false);
        let mut overview = data(request(&doc, &selected), 1, 0, 16);
        overview.saved = WaveformSettings::Legacy {
            declared_duration_seconds: 0.01,
            start_tick: 0,
            trim_start_seconds: None,
            trim_end_seconds: Some(0.002),
            fade_in_seconds: None,
            fade_out_seconds: None,
        };
        let prepared = Prepared::new(overview).unwrap();
        let mut editing = draft(&doc, &selected);
        let ClipDraftFields::LegacyAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.trim_end_seconds.mode = FieldMode::Remove;
        assert!(editing.patch().is_ok());
        assert!(
            prepared.draft(Some(&editing), false).is_err(),
            "Actual original is shorter than the declared default"
        );
        let ClipDraftFields::LegacyAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.trim_end_seconds.mode = FieldMode::Keep;
        fields.fade_in_seconds.mode = FieldMode::Set;
        fields.fade_in_seconds.text = "0.0015".into();
        fields.fade_out_seconds.mode = FieldMode::Set;
        fields.fade_out_seconds.text = "0.0015".into();
        assert!(editing.patch().is_ok());
        assert!(prepared.draft(Some(&editing), false).is_err());
    }

    #[test]
    fn authenticated_source_remains_visible_when_saved_declared_window_is_too_long_and_draft_can_repair_preview()
     {
        let (doc, selected) = document(false);
        let mut value = doc.value().clone();
        value["tracks"][0]["audio"]["durationSeconds"] = 2.into();
        let doc = ProjectDocument::from_value(value).unwrap();
        let mut overview = data(request(&doc, &selected), 1, 0, 8000);
        overview.saved = WaveformSettings::Legacy {
            declared_duration_seconds: 2.0,
            start_tick: 0,
            trim_start_seconds: None,
            trim_end_seconds: None,
            fade_in_seconds: None,
            fade_out_seconds: None,
        };
        let prepared = Prepared::new(overview).unwrap();
        assert!(prepared.saved.is_none());
        assert!(prepared.saved_problem.is_some());
        assert_eq!(prepared.data.frame_count, 8000);
        assert_eq!(prepared.data.channel_bins[0].len(), MAX_WAVEFORM_BINS);
        let mut editing = draft(&doc, &selected);
        let ClipDraftFields::LegacyAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.trim_end_seconds.mode = FieldMode::Set;
        fields.trim_end_seconds.text = "1".into();
        let preview = prepared.draft(Some(&editing), false).unwrap().unwrap();
        assert_eq!(
            preview.window,
            AudioSourceWindow {
                start_frame: 0,
                frame_count: 8000
            }
        );

        let ctx = context();
        let (mut waveform, fake) = injected();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        let mut overview = prepared.data;
        overview.request = fake.borrow().submitted[0].1.clone();
        complete(&fake, Ok(overview));
        let output = frame(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            Some(&editing),
            true,
            vec![],
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(
            text.iter()
                .any(|text| text.contains("Saved overlay unavailable"))
        );
        assert!(text.iter().any(|text| text.contains("Local draft preview")));
        assert!(waveform.prepared.is_some());
        assert_eq!(
            doc.value()["tracks"][0]["audio"]["durationSeconds"],
            json!(2)
        );
    }

    #[test]
    fn saved_original_and_local_overlays_are_distinct_and_invalid_text_keeps_saved_source_views() {
        let (doc, selected) = document(true);
        let fades = AudioFades {
            in_frames: 1,
            out_frames: 1,
            curve: FadeCurve::Linear,
        };
        let doc = doc
            .update_clip(
                &selected.address,
                &ClipPatch::NativeAudio(omniusong_core::NativeAudioClipPatch {
                    position_frame: ValueChange::Set { value: 42 },
                    source_window: ValueChange::Set {
                        value: AudioSourceWindow {
                            start_frame: 102,
                            frame_count: 8,
                        },
                    },
                    fades: FieldChange::Set {
                        value: fades.clone(),
                    },
                }),
                omniusong_core::ClipTimelinePolicy::PreserveLoop,
            )
            .unwrap()
            .with_revision(1)
            .unwrap();
        let mut editing = draft(&doc, &selected);
        let ClipDraftFields::NativeAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.position_mode = ValueMode::Set;
        fields.position_frame = "47".into();
        let ctx = context();
        let (mut waveform, fake) = injected();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        let mut overview = data(fake.borrow().submitted[0].1.clone(), 2, 100, 16);
        overview.saved = WaveformSettings::Native {
            window: AudioSourceWindow {
                start_frame: 102,
                frame_count: 8,
            },
            position_frame: 42,
            fades: Some(fades),
        };
        overview.original = Some(WaveformSettings::Native {
            window: AudioSourceWindow {
                start_frame: 100,
                frame_count: 16,
            },
            position_frame: -3,
            fades: None,
        });
        let raw_bins = overview.channel_bins.clone();
        complete(&fake, Ok(overview));
        let output = frame(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            Some(&editing),
            true,
            vec![],
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(
            text.iter()
                .any(|text| text.contains("Saved · solid line · Source [102, 110)"))
        );
        assert!(
            text.iter()
                .any(|text| text.contains("Permanent original · dotted line · Source [100, 116)"))
        );
        assert!(
            text.iter()
                .any(|text| text.contains("Local draft preview · dashed line")
                    && text.contains("47 frames"))
        );
        let ClipDraftFields::NativeAudio(fields) = &mut editing.fields else {
            panic!()
        };
        fields.position_frame = "invalid".into();
        let output = frame(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            Some(&editing),
            true,
            vec![],
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(
            text.iter()
                .any(|text| text.contains("Local preview unavailable"))
        );
        assert!(text.iter().any(|text| text.contains("Saved · solid line")));
        assert!(
            !text
                .iter()
                .any(|text| text.contains("Local draft preview · dashed line"))
        );
        assert_eq!(
            waveform.prepared.as_ref().unwrap().data.channel_bins,
            raw_bins
        );
    }

    #[test]
    fn show_is_explicit_hide_works_while_session_disabled_and_late_success_is_not_installed() {
        let (doc, selected) = document(false);
        let ctx = context();
        let (mut waveform, fake) = injected();
        let original = doc.to_json().unwrap();
        let _ = frame(&ctx, &mut waveform, &doc, &selected, None, true, vec![]);
        assert!(fake.borrow().submitted.is_empty());
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            false,
        );
        assert!(fake.borrow().submitted.is_empty());
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        assert!(waveform.pending());
        let request = fake.borrow().submitted[0].1.clone();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Hide source waveform",
            false,
        );
        assert!(waveform.pending());
        assert!(!waveform.shown);
        assert_eq!(fake.borrow().cancelled, 1);
        complete(&fake, Ok(data(request, 1, 0, 16)));
        waveform.poll(Some(&doc), Some(&selected));
        assert!(waveform.prepared.is_none());
        assert!(!waveform.pending());
        assert!(!waveform.shown);
        assert_eq!(doc.to_json().unwrap(), original);
    }

    #[test]
    fn cancel_retains_credit_and_retry_never_enqueues_until_matching_completion_is_drained() {
        let (doc, selected) = document(false);
        let ctx = context();
        let (mut waveform, fake) = injected();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Cancel waveform preparation",
            false,
        );
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Retry source waveform",
            true,
        );
        assert_eq!(fake.borrow().submitted.len(), 1);
        assert!(waveform.pending());
        complete(&fake, Err("Waveform cancelled".into()));
        waveform.poll(Some(&doc), Some(&selected));
        assert!(waveform.notice.is_some());
        assert!(waveform.error.is_none());
        assert!(!waveform.pending());
        assert_eq!(
            fake.borrow().submitted.len(),
            1,
            "No automatic retry after cancellation"
        );
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Retry source waveform",
            true,
        );
        assert_eq!(fake.borrow().submitted.len(), 2);
        assert!(waveform.pending());
    }

    #[test]
    fn selection_revision_source_change_and_copied_project_revoke_old_waveform_results() {
        for change in 0..3 {
            let (doc, selected) = document(false);
            let (mut waveform, fake) = injected();
            waveform.poll(Some(&doc), Some(&selected));
            waveform.start(Path::new("/synthetic/waveform/no-files"));
            let old = fake.borrow().submitted[0].1.clone();
            let mut value = doc.value().clone();
            match change {
                0 => value["revision"] = 1.into(),
                1 => value["tracks"][0]["assetId"] = format!("{}.wav", "d".repeat(64)).into(),
                _ => value["id"] = uuid::Uuid::new_v4().to_string().into(),
            }
            let next = ProjectDocument::from_value(value).unwrap();
            let next_selected = ClipSelection {
                project_id: next.id().into(),
                address: selected.address.clone(),
            };
            waveform.poll(Some(&next), Some(&next_selected));
            complete(&fake, Ok(data(old, 1, 0, 16)));
            waveform.poll(Some(&next), Some(&next_selected));
            assert!(waveform.prepared.is_none());
            assert!(!waveform.shown);
            assert!(waveform.error.is_none());
            assert_eq!(fake.borrow().submitted.len(), 1);
        }
    }

    #[test]
    fn copied_selection_does_not_reuse_an_old_show_widget_identity() {
        let (doc, selected) = document(false);
        let ctx = context();
        let (mut waveform, fake) = injected();
        let tree = frame(&ctx, &mut waveform, &doc, &selected, None, true, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Show source waveform"))
            .unwrap()
            .0;
        let mut value = doc.value().clone();
        value["id"] = uuid::Uuid::new_v4().to_string().into();
        let copy = ProjectDocument::from_value(value).unwrap();
        let copy_selected = ClipSelection {
            project_id: copy.id().into(),
            address: selected.address,
        };
        let _ = frame(
            &ctx,
            &mut waveform,
            &copy,
            &copy_selected,
            None,
            true,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: target,
                    data: None,
                },
            )],
        );
        assert!(fake.borrow().submitted.is_empty());
    }

    #[test]
    fn thirty_two_wav_channels_keep_true_extrema_with_bounded_virtualized_lanes() {
        let (doc, selected) = document(false);
        let ctx = context();
        let (mut waveform, fake) = injected();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        let overview = data(fake.borrow().submitted[0].1.clone(), 32, 0, 16);
        let original_extrema = overview.channel_bins[31][0].clone();
        complete(&fake, Ok(overview));
        let output = frame(&ctx, &mut waveform, &doc, &selected, None, true, vec![]);
        let prepared = waveform.prepared.as_ref().unwrap();
        assert_eq!(prepared.data.channel_bins.len(), 32);
        assert_eq!(prepared.gap_frames, 0);
        assert!(!prepared.data.unknown_tail);
        assert_eq!(prepared.data.channel_bins[31][0], original_extrema);
        assert!(
            prepared.scale > 1.0,
            "Raw sample amplitude must not be clipped or normalized"
        );
        let tree = output.platform_output.accesskit_update.unwrap();
        let rendered_channels = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == egui::accesskit::Role::Label)
            .filter_map(|(_, node)| node.value().or(node.label()))
            .filter(|text| text.starts_with("Source channel "))
            .count();
        assert!(
            (1..=6).contains(&rendered_channels),
            "Only visible channel lanes should be painted/accessibly projected"
        );
    }

    #[test]
    fn sixteen_native_channels_keep_mixed_content_gaps_and_unknown_tail_without_fabricated_silence()
    {
        let (doc, selected) = document(true);
        let ctx = context();
        let (mut waveform, fake) = injected();
        click(
            &ctx,
            &mut waveform,
            &doc,
            &selected,
            "Show source waveform",
            true,
        );
        let mut overview = data(fake.borrow().submitted[0].1.clone(), 16, 100, 2048);
        overview.saved = WaveformSettings::Native {
            window: AudioSourceWindow {
                start_frame: 100,
                frame_count: 16,
            },
            position_frame: -3,
            fades: None,
        };
        overview.unknown_tail = true;
        for bins in &mut overview.channel_bins {
            assert_eq!(bins[3].frame_count, 2);
            bins[3].content_frames = 1;
            bins[3].gap_frames = 1;
            bins[4].content_frames = 0;
            bins[4].gap_frames = 2;
            bins[4].min = None;
            bins[4].max = None;
        }
        let recorded_extrema = overview.channel_bins[15][3].clone();
        complete(&fake, Ok(overview));
        let output = frame(&ctx, &mut waveform, &doc, &selected, None, true, vec![]);
        let prepared = waveform.prepared.as_ref().unwrap();
        assert_eq!(prepared.data.channels, 16);
        assert_eq!(prepared.gap_frames, 3);
        assert_eq!(prepared.data.source_start_frame, 100);
        assert_eq!(
            prepared.data.channel_bins[15][3], recorded_extrema,
            "Mixed bins retain only their recorded content extrema"
        );
        assert_eq!(prepared.data.channel_bins[15][4].min, None);
        assert_eq!(prepared.data.channel_bins[15][4].max, None);
        let tree = output.platform_output.accesskit_update.unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(
            text.iter()
                .any(|text| text.contains("Unknown tail beyond the proven source range"))
        );
        assert!(
            text.iter()
                .any(|text| text.contains("Gaps are not silence"))
        );
        assert!(
            text.iter()
                .any(|text| text.contains("3 declared gap frames"))
        );
    }

    #[test]
    fn invalid_extrema_or_mismatched_result_never_become_ready_and_errors_require_explicit_retry() {
        let (doc, selected) = document(false);
        let (mut waveform, fake) = injected();
        waveform.poll(Some(&doc), Some(&selected));
        waveform.start(Path::new("/synthetic/waveform/no-files"));
        let mut overview = data(fake.borrow().submitted[0].1.clone(), 1, 0, 16);
        overview.channel_bins[0][0].max = Some(f32::NAN);
        complete(&fake, Ok(overview));
        waveform.poll(Some(&doc), Some(&selected));
        assert!(waveform.prepared.is_none());
        assert!(waveform.error.as_ref().unwrap().contains("extrema"));
        for _ in 0..3 {
            waveform.poll(Some(&doc), Some(&selected));
        }
        assert_eq!(fake.borrow().submitted.len(), 1);
        waveform.start(Path::new("/synthetic/waveform/no-files"));
        let mut overview = data(fake.borrow().submitted[1].1.clone(), 1, 0, 16);
        overview.request.expected_revision += 1;
        complete(&fake, Ok(overview));
        waveform.poll(Some(&doc), Some(&selected));
        assert!(waveform.prepared.is_none());
        assert!(
            waveform
                .error
                .as_ref()
                .unwrap()
                .contains("different source request")
        );
    }

    fn nested_bottom_frame(
        ctx: &egui::Context,
        waveform: &mut WaveformUi,
        doc: &ProjectDocument,
        selected: &ClipSelection,
    ) -> (egui::FullOutput, egui::Vec2) {
        let mut scroll_metrics = egui::Vec2::ZERO;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 320.0),
                )),
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| {
                    let outer = egui::ScrollArea::vertical()
                        .id_salt("nested-waveform-inspector")
                        .stick_to_bottom(true)
                        .animated(false)
                        .show(ui, |ui| {
                            ui.add_space(900.0);
                            waveform.ui(
                                ui,
                                Path::new("/synthetic/waveform/no-files"),
                                doc,
                                Some(selected),
                                None,
                                WaveformAccess {
                                    unresolved: false,
                                    allow_start: false,
                                },
                            );
                        });
                    scroll_metrics = egui::vec2(
                        outer.state.clamped_offset().y,
                        (outer.content_size.y - outer.inner_rect.height()).max(0.0),
                    );
                });
            },
        );
        output.textures_delta.clear();
        (output, scroll_metrics)
    }

    #[test]
    fn nested_inspector_at_bottom_exposes_a_complete_lane_and_retains_channel_virtualization() {
        for large_font in [false, true] {
            for channels in [1, 32] {
                let (doc, selected) = document(false);
                let ctx = context();
                if large_font {
                    ctx.all_styles_mut(|style| {
                        style
                            .text_styles
                            .insert(egui::TextStyle::Small, egui::FontId::proportional(22.0));
                    });
                }
                let requested = request(&doc, &selected);
                let mut waveform = WaveformUi {
                    prepared: Some(
                        Prepared::new(data(requested.clone(), channels, 0, 16)).unwrap(),
                    ),
                    selected: Some(requested),
                    shown: true,
                    ..Default::default()
                };
                // Let the two independently sized scroll areas settle, just as
                // repeated outer scrolling did in the actual failing window.
                for _ in 0..6 {
                    let _ = nested_bottom_frame(&ctx, &mut waveform, &doc, &selected);
                }
                let (output, scroll) = nested_bottom_frame(&ctx, &mut waveform, &doc, &selected);
                assert!(
                    scroll.x > 500.0,
                    "The fixture must contain substantial preceding inspector content"
                );
                assert!(
                    (scroll.x - scroll.y).abs() <= 0.5,
                    "The outer inspector must actually be at its bottom: {scroll:?}"
                );
                let plots: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|clipped| {
                        let egui::Shape::Rect(shape) = &clipped.shape else {
                            return None;
                        };
                        ((shape.rect.height() - CHANNEL_PLOT_HEIGHT).abs() <= 0.01)
                            .then_some((shape.rect, clipped.clip_rect))
                    })
                    .collect();
                assert!(
                    !plots.is_empty(),
                    "The actual waveform plot must be painted"
                );
                assert!(
                    plots
                        .iter()
                        .any(|(rect, clip)| clip.top() <= rect.top() + 0.5
                            && clip.bottom() >= rect.bottom() - 0.5),
                    "A complete positive and negative waveform lane must fit its nested clip rectangle, channels={channels}, large_font={large_font}, plots={plots:?}"
                );
                if channels == 1 {
                    assert_eq!(plots.len(), 1);
                } else {
                    assert!(
                        plots.len() <= 6,
                        "A small viewport must not paint all 32 channel plots"
                    );
                }
            }
        }
    }
}
