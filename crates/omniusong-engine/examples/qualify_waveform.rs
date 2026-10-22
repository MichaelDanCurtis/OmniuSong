//! Read-only offline waveform receipt; no devices, rendering, models or UI.
//!
//! Usage: qualify_waveform STORE PROJECT_ID EXPECTED_REVISION OUTPUT_JSON
//! OUTPUT_JSON must be a fresh name outside STORE in an existing directory.
//! Every declared legacy audio/native audio-frame clip is prepared at 1024 bins;
//! any unsupported/damaged/stale audio fails the whole report. Raw-event clips
//! are explicitly listed as skipped, never interpreted as notes/audio. The CLI
//! bounds itself to 64 audio views, 16MiB of typed bins and 64MiB of JSON. It
//! rejects excess work instead of writing a partial-success report. A failed
//! final write may leave its own new incomplete output; no catalog/source writes
//! or cleanup are attempted. Coordinator-owned stable paths are assumed.
use omniusong_core::{DocumentClipAddress, ProjectDocument, native::TakeClip};
use omniusong_engine::{
    waveform::prepare_waveform,
    waveform_types::{WaveformBin, WaveformData, WaveformRequest, WaveformSourceKey},
};
use omniusong_store::NativeReader;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

type Outcome<T> = Result<T, Box<dyn Error>>;
const MAX_VIEWS: usize = 64;
const MAX_TYPED_BYTES: usize = 16 * 1024 * 1024;
const MAX_REPORT_BYTES: usize = 64 * 1024 * 1024;
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn directory(path: &Path) -> Outcome<PathBuf> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Expected an existing ordinary non-symlink directory".into());
    }
    Ok(fs::canonicalize(path)?)
}
fn output_path(path: &Path, root: &Path) -> Outcome<PathBuf> {
    let name = path.file_name().ok_or("Output requires a filename")?;
    let parent = directory(
        path.parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let output = parent.join(name);
    if output.starts_with(root) {
        return Err("Offline report must be outside the native store".into());
    }
    match fs::symlink_metadata(&output) {
        Ok(_) => return Err("Output already exists; no waveform preparation was started".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(output)
}
fn requests(document: &ProjectDocument) -> Outcome<(Vec<WaveformRequest>, Vec<Value>)> {
    let mut requests = Vec::new();
    let mut skipped = Vec::new();
    for track in document.tracks() {
        let track_id = track["id"]
            .as_str()
            .ok_or("Validated track has no identity")?;
        if track["kind"] == "audio" {
            if !track.get("audio").is_some_and(Value::is_object) {
                return Err(format!("Audio track {track_id:?} has no declared clip").into());
            }
            let asset_id = track["assetId"]
                .as_str()
                .ok_or("Audio clip has no playable original asset ID")?;
            requests.push(WaveformRequest {
                project_id: document.id().into(),
                expected_revision: document.revision(),
                address: DocumentClipAddress::LegacyAudio {
                    track_id: track_id.into(),
                },
                source: WaveformSourceKey::LegacyWav {
                    asset_id: asset_id.into(),
                    source_asset_id: track
                        .get("sourceAssetId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                },
                bins: 1024,
            });
        } else if document.supports_native_takes() && track["kind"] == "recording" {
            for raw in track["recording"]["clips"]
                .as_array()
                .ok_or("Validated recording clips missing")?
            {
                if raw["range"]["unit"] == "events" {
                    skipped.push(json!({"reason":"raw_events_skipped_read_only_no_audio_duration","trackId":track_id,"clipId":raw["id"],"source":raw["source"],"range":raw["range"],"muted":track["muted"],"interpretedAsAudio":false}));
                } else {
                    let clip: TakeClip = serde_json::from_value(raw.clone())?;
                    clip.validate()?;
                    if !matches!(
                        clip.range,
                        omniusong_core::native::TakeRange::AudioFrames { .. }
                    ) {
                        return Err("Unexpected native source units".into());
                    }
                    requests.push(WaveformRequest {
                        project_id: document.id().into(),
                        expected_revision: document.revision(),
                        address: DocumentClipAddress::NativeAudio {
                            track_id: track_id.into(),
                            clip_id: clip.id,
                        },
                        source: WaveformSourceKey::NativeAudio {
                            source: clip.source,
                            sample_rate: clip.position.sample_rate,
                        },
                        bins: 1024,
                    });
                }
            }
        }
        if requests.len() > MAX_VIEWS {
            return Err(
                "Offline qualifier exceeds its 64-audio-view bound; no partial report is accepted"
                    .into(),
            );
        }
    }
    for request in &requests {
        request.validate()?;
    }
    Ok((requests, skipped))
}
struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .0
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > MAX_REPORT_BYTES)
        {
            return Err(io::Error::other("Offline waveform report exceeds 64MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    format: &'static str,
    version: u32,
    offline_only: bool,
    device_opened: bool,
    ui_operated: bool,
    catalog_mutation_api_called: bool,
    source_mutation_api_called: bool,
    inference: bool,
    store: PathBuf,
    project_id: String,
    expected_revision: u64,
    project_name: String,
    canonical_project_json_sha256: String,
    canonical_project_json_bytes: usize,
    project_metadata_rechecked_exact: bool,
    resolution_bins: usize,
    waveforms: Vec<WaveformData>,
    skipped_raw_events: Vec<Value>,
    limitations: Vec<&'static str>,
}
fn main() -> Outcome<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [root_arg, project_arg, revision_arg, output_arg] = args.as_slice() else {
        return Err(
            "usage: qualify_waveform STORE PROJECT_ID EXPECTED_REVISION OUTPUT_JSON".into(),
        );
    };
    let root = directory(Path::new(root_arg))?;
    let output = output_path(Path::new(output_arg), &root)?;
    let project_id = project_arg.to_str().ok_or("Project ID must be UTF-8")?;
    let revision_text = revision_arg.to_str().ok_or("Revision must be UTF-8")?;
    if revision_text.is_empty() || !revision_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Expected revision must be a whole nonnegative integer".into());
    }
    let expected_revision: u64 = revision_text.parse()?;
    let mut reader = NativeReader::open(&root)?;
    let document = reader.read_snapshot(|reader| reader.load_document(project_id))?;
    if document.id() != project_id || document.revision() != expected_revision {
        return Err("Project identity/revision differs from the requested offline snapshot".into());
    }
    let canonical = document.to_json()?;
    let (requests, skipped) = requests(&document)?;
    let mut data = Vec::new();
    let mut typed_bytes = 0usize;
    for request in requests {
        let waveform = prepare_waveform(&root, &request, || false).map_err(|error| {
            format!(
                "Whole offline waveform report rejected for {:?}: {error}",
                request.address
            )
        })?;
        waveform.validate()?;
        let bytes = waveform
            .channel_bins
            .iter()
            .try_fold(std::mem::size_of::<WaveformData>(), |size, bins| {
                size.checked_add(
                    bins.capacity()
                        .checked_mul(std::mem::size_of::<WaveformBin>())?,
                )
            })
            .ok_or("Typed waveform size overflow")?;
        typed_bytes = typed_bytes
            .checked_add(bytes)
            .ok_or("Typed report size overflow")?;
        if typed_bytes > MAX_TYPED_BYTES {
            return Err(
                "Offline waveform summaries exceed 16MiB; no partial report is accepted".into(),
            );
        }
        data.push(waveform);
    }
    let latest = reader.read_snapshot(|reader| reader.load_document(project_id))?;
    if latest.revision() != expected_revision || latest.to_json()? != canonical {
        return Err("Project changed during offline preparation; no report is accepted".into());
    }
    let report = Report {
        format: "omniusong.waveform.offline-qualification",
        version: 1,
        offline_only: true,
        device_opened: false,
        ui_operated: false,
        catalog_mutation_api_called: false,
        source_mutation_api_called: false,
        inference: false,
        store: root,
        project_id: project_id.into(),
        expected_revision,
        project_name: document.name().into(),
        canonical_project_json_sha256: sha(&canonical),
        canonical_project_json_bytes: canonical.len(),
        project_metadata_rechecked_exact: true,
        resolution_bins: 1024,
        waveforms: data,
        skipped_raw_events: skipped,
        limitations: vec![
            "Production prepare_waveform data only: original-source per-channel extrema and coverage, not audio rendering, listening, latency, UI or Windows acceptance.",
            "Saved trims, placement, gains and fades do not alter original-source extrema. Cached settings are overlay inputs; legacy measured/declaration mismatch can remain inspectable.",
            "Raw-event entries are listed explicitly and are not interpreted as audio or MIDI performances. Native recovery exposes only the verified prefix and labels an unknown tail.",
            "No catalog/source mutation API is called. The coordinator independently binds this report to immutable catalog/source snapshots and compares the bins against retained bytes.",
            "Every selected audio view must succeed; unsupported codecs, damaged sources, stale revision and resource bounds fail the entire report. A final filesystem-write error may retain this invocation's own new incomplete output.",
        ],
    };
    let mut encoded = BoundedJson(Vec::new());
    serde_json::to_writer_pretty(&mut encoded, &report)?;
    encoded.write_all(b"\n")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    file.write_all(&encoded.0)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(output.parent().ok_or("Output parent missing")?)?.sync_all()?;
    println!(
        "Offline waveform report {} · {} audio views · {} raw-event entries skipped · project {} r{} · SHA256 {}",
        output.display(),
        report.waveforms.len(),
        report.skipped_raw_events.len(),
        report.project_id,
        report.expected_revision,
        report.canonical_project_json_sha256
    );
    Ok(())
}
