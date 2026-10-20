//! Retained local stages: read-only discovery/review and explicit checked recovery.
//! A stage is outside project/archive closure until separately attached.
use super::{NativeStore, Result, StoreError};
use omniusong_core::contracts::{OperationId, Validate};
use omniusong_recording::{
    CaptureReader, JournalInspection, PcmEncoding, PcmLayout, RecoveryProblem, SealedManifest,
    StreamKind, TakeState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const MAX_CAPTURE_STAGES: usize = 128;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStageState {
    Uninspected,
    Recording,
    Interrupted,
    Sealed,
    Owned,
    Invalid,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStage {
    pub stage_id: String,
    pub journal_path: PathBuf,
    pub state: CaptureStageState,
    pub problem: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStageScan {
    pub stages: Vec<CaptureStage>,
    pub problems: Vec<String>,
    pub truncated: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureAudio {
    pub stream_id: String,
    pub start_frame: u64,
    pub frames: u64,
    pub recorded_frames: u64,
    pub gap_frames: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: PcmEncoding,
    pub layout: PcmLayout,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureInspection {
    pub stage_id: String,
    pub journal_path: PathBuf,
    pub take_id: String,
    pub descriptor_sha256: String,
    pub inspection_sha256: String,
    pub journal_sha256: String,
    pub original_rows_sha256: String,
    pub byte_length: u64,
    pub state: CaptureStageState,
    pub verified_chunks: u64,
    pub verified_payload_bytes: u64,
    pub chunk_index_sha256: String,
    pub problems: Vec<RecoveryProblem>,
    pub unknown_tail: bool,
    pub audio: Vec<CaptureAudio>,
    pub manifest_sha256: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureRecoveryReceipt {
    pub stage_id: String,
    pub journal_path: PathBuf,
    pub take_id: String,
    pub operation_id: String,
    pub reviewed_inspection_sha256: String,
    pub manifest_sha256: String,
    pub unknown_tail: bool,
    pub audio: Vec<CaptureAudio>,
    pub replayed: bool,
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(message.into()))
    }
}
fn canonical_stage(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|u| u.hyphenated().to_string() == id)
}
fn is_sha(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn ordinary_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Capture directory must be ordinary and non-symlink",
    )?;
    require(
        fs::canonicalize(path)? == path,
        "Capture directory redirects outside its owned namespace",
    )
}
fn ordinary_journal(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    require(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Capture journal must be an ordinary non-symlink file",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.nlink() == 1,
            "Hard-linked capture originals are unsupported",
        )?;
    }
    require(
        fs::canonicalize(path)? == path,
        "Capture journal redirects outside its owned stage",
    )
}
fn capture_root(store: &NativeStore) -> Result<Option<PathBuf>> {
    ordinary_directory(&store.root)?;
    let root = store.root.join("captures");
    match fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(_) => {
            ordinary_directory(&root)?;
            Ok(Some(root))
        }
    }
}
fn stage_path(store: &NativeStore, id: &str) -> Result<PathBuf> {
    require(
        canonical_stage(id),
        "Retained capture identity must be a canonical UUID",
    )?;
    let root = capture_root(store)?
        .ok_or_else(|| StoreError::Invalid("No retained capture directory exists".into()))?;
    let directory = root.join(id);
    ordinary_directory(&directory)?;
    let path = directory.join("take.sqlite3");
    ordinary_journal(&path)?;
    Ok(path)
}
fn audio(inspection: &JournalInspection) -> Result<Vec<CaptureAudio>> {
    let mut audio = Vec::new();
    for stream in &inspection.descriptor.streams {
        let StreamKind::Audio { format, .. } = &stream.kind else {
            continue;
        };
        let prefix = inspection
            .report
            .streams
            .iter()
            .find(|p| p.stream_id == stream.stream_id)
            .ok_or_else(|| StoreError::Invalid("Inspected audio stream prefix missing".into()))?;
        let frames = prefix
            .source_end
            .value()
            .checked_sub(stream.initial_source_position)
            .ok_or_else(|| StoreError::Invalid("Inspected source position regressed".into()))?;
        audio.push(CaptureAudio {
            stream_id: stream.stream_id.clone(),
            start_frame: stream.initial_source_position,
            frames,
            recorded_frames: prefix.recorded_units,
            gap_frames: prefix.gap_units,
            sample_rate: format.sample_rate,
            channels: format.channels,
            encoding: format.encoding,
            layout: format.layout,
        });
    }
    Ok(audio)
}
fn fingerprint(value: &CaptureInspection) -> Result<String> {
    let mut canonical = value.clone();
    canonical.inspection_sha256.clear();
    let bytes = serde_json::to_vec(&canonical).map_err(|e| StoreError::Invalid(e.to_string()))?;
    require(
        bytes.len() <= 64 * 1024,
        "Capture inspection exceeds its bounded metadata limit",
    )?;
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.capture-inspection.v1\0");
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}
fn flatten(stage_id: &str, source: &JournalInspection) -> Result<CaptureInspection> {
    let mut value = CaptureInspection {
        stage_id: stage_id.into(),
        journal_path: source.path.clone(),
        take_id: source.descriptor.take_id.0.clone(),
        descriptor_sha256: source.descriptor_sha256.clone(),
        inspection_sha256: String::new(),
        journal_sha256: source.journal_sha256.clone(),
        original_rows_sha256: source.original_rows_sha256.clone(),
        byte_length: source.byte_length,
        state: match source.report.state {
            TakeState::Recording => CaptureStageState::Recording,
            TakeState::Interrupted => CaptureStageState::Interrupted,
            TakeState::Sealed => CaptureStageState::Sealed,
        },
        verified_chunks: source.report.verified_chunks,
        verified_payload_bytes: source.report.verified_payload_bytes,
        chunk_index_sha256: source.report.chunk_index_sha256.clone(),
        problems: source.report.problems.clone(),
        unknown_tail: source.report.unknown_tail,
        audio: audio(source)?,
        manifest_sha256: source.manifest.as_ref().map(|m| m.sha256.clone()),
    };
    value.inspection_sha256 = fingerprint(&value)?;
    Ok(value)
}
fn validate_review(expected: &CaptureInspection) -> Result<()> {
    require(
        canonical_stage(&expected.stage_id)
            && expected.journal_path.is_absolute()
            && expected.journal_path.to_string_lossy().len() <= 4096,
        "Invalid reviewed capture path/identity",
    )?;
    require(
        expected.take_id.len() <= 96
            && !expected.take_id.is_empty()
            && expected.audio.len() <= 64
            && expected.problems.len() <= 64,
        "Invalid reviewed capture metadata size",
    )?;
    for s in [
        &expected.descriptor_sha256,
        &expected.inspection_sha256,
        &expected.journal_sha256,
        &expected.original_rows_sha256,
        &expected.chunk_index_sha256,
    ] {
        require(is_sha(s), "Invalid reviewed capture digest")?;
    }
    if let Some(s) = &expected.manifest_sha256 {
        require(is_sha(s), "Invalid reviewed manifest digest")?;
    }
    for a in &expected.audio {
        require(
            !a.stream_id.is_empty()
                && a.stream_id.len() <= 96
                && a.start_frame
                    .checked_add(a.frames)
                    .is_some_and(|end| end <= i64::MAX as u64)
                && a.recorded_frames.checked_add(a.gap_frames) == Some(a.frames)
                && (8000..=768000).contains(&a.sample_rate)
                && (1..=16).contains(&a.channels),
            "Invalid reviewed audio range/configuration",
        )?;
    }
    for p in &expected.problems {
        require(
            p.message.len() <= 4096 && p.stream_id.as_ref().is_none_or(|s| s.len() <= 96),
            "Invalid reviewed diagnostic size",
        )?;
    }
    require(
        fingerprint(expected)? == expected.inspection_sha256,
        "Reviewed capture token differs from its exact DTO fields",
    )?;
    require(
        matches!(
            expected.state,
            CaptureStageState::Recording | CaptureStageState::Interrupted
        ) && expected.manifest_sha256.is_none()
            && expected.unknown_tail,
        "Only an exact reviewed unsealed capture can request prefix recovery",
    )
}
fn seal_operation(expected: &CaptureInspection, operation_id: &str) -> Result<String> {
    OperationId(operation_id.into())
        .validate()
        .map_err(|e| StoreError::Invalid(e.to_string()))?;
    let bytes = serde_json::to_vec(&(
        operation_id,
        &expected.stage_id,
        &expected.inspection_sha256,
    ))
    .map_err(|e| StoreError::Invalid(e.to_string()))?;
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.capture-recovery-operation.v1\0");
    hash.update(bytes);
    Ok(format!("capture-recovery-{:x}", hash.finalize()))
}
fn manifest_matches(
    manifest: &SealedManifest,
    expected: &CaptureInspection,
    source: &JournalInspection,
) -> Result<()> {
    require(
        manifest.body.interrupted.is_some()
            && manifest.body.descriptor.take_id.0 == expected.take_id
            && manifest.body.descriptor_sha256 == expected.descriptor_sha256
            && manifest.body.chunk_count == expected.verified_chunks
            && manifest.body.payload_bytes == expected.verified_payload_bytes
            && manifest.body.chunk_index_sha256 == expected.chunk_index_sha256
            && source.original_rows_sha256 == expected.original_rows_sha256
            && audio(source)? == expected.audio,
        "Recovered original/selected prefix differs from the exact reviewed source",
    )
}
impl NativeStore {
    /// Discovery reads at most128 entries; it never opens or changes a journal.
    pub fn scan_captures(&self) -> CaptureStageScan {
        let mut result = CaptureStageScan {
            stages: vec![],
            problems: vec![],
            truncated: false,
        };
        let root = match capture_root(self) {
            Ok(Some(root)) => root,
            Ok(None) => return result,
            Err(e) => {
                result.problems.push(e.to_string());
                return result;
            }
        };
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(e) => {
                result.problems.push(e.to_string());
                return result;
            }
        };
        for (index, entry) in entries.take(MAX_CAPTURE_STAGES).enumerate() {
            if index + 1 == MAX_CAPTURE_STAGES {
                result.truncated = true;
                result.problems.push("Capture scan reached its128-entry bound; additional entries may remain undiscovered".into());
            }
            match entry {
                Err(e) => result.problems.push(e.to_string()),
                Ok(entry) => {
                    let stage_id = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path().join("take.sqlite3");
                    let problem = stage_path(self, &stage_id).err().map(|e| e.to_string());
                    result.stages.push(CaptureStage {
                        stage_id,
                        journal_path: path,
                        state: if problem.is_some() {
                            CaptureStageState::Invalid
                        } else {
                            CaptureStageState::Uninspected
                        },
                        problem,
                    });
                }
            }
        }
        result.stages.sort_by(|a, b| a.stage_id.cmp(&b.stage_id));
        result
    }
    pub fn inspect_capture(&self, stage_id: &str) -> Result<CaptureInspection> {
        let path = stage_path(self, stage_id)?;
        let reader = CaptureReader::open(&path)?;
        ordinary_journal(&path)?;
        flatten(stage_id, reader.inspection())
    }
    pub fn recover_capture(
        &self,
        expected: &CaptureInspection,
        operation_id: &str,
    ) -> Result<CaptureRecoveryReceipt> {
        validate_review(expected)?;
        let path = stage_path(self, &expected.stage_id)?;
        require(
            path == expected.journal_path,
            "Reviewed stage path differs from its owned location",
        )?;
        let derived = seal_operation(expected, operation_id)?;
        let reader = CaptureReader::open(&path)?;
        let current = flatten(&expected.stage_id, reader.inspection())?;
        let replayed = if let Some(manifest) = &reader.inspection().manifest {
            require(
                manifest.body.operation_id == derived,
                "Conflicting recovery operation or changed review",
            )?;
            manifest_matches(manifest, expected, reader.inspection())?;
            true
        } else {
            require(
                current == *expected,
                "Capture changed since inspection; review it again",
            )?;
            false
        };
        ordinary_journal(&path)?;
        let (manifest, recording_replayed) = reader.recover(&derived)?;
        require(
            replayed == recording_replayed,
            "Capture recovery replay state changed",
        )?;
        // A fresh locked read authenticates originals and the exact sealed prefix
        // after intentional interruption/manifest writes, including replay tails.
        let verified = CaptureReader::open(&path)?;
        manifest_matches(&manifest, expected, verified.inspection())?;
        Ok(CaptureRecoveryReceipt {
            stage_id: expected.stage_id.clone(),
            journal_path: path,
            take_id: expected.take_id.clone(),
            operation_id: operation_id.into(),
            reviewed_inspection_sha256: expected.inspection_sha256.clone(),
            manifest_sha256: manifest.sha256,
            unknown_tail: true,
            audio: audio(verified.inspection())?,
            replayed,
        })
    }
}
