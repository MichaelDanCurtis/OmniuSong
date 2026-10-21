//! Manual software cost measurement, deliberately excluded from normal gates.
//!
//! Run only with both OMNIUSONG_SEAL_PROFILE_SOURCE (an existing quiescent
//! Finish-v2 journal) and OMNIUSONG_SEAL_PROFILE_RECEIPT (a fresh JSON path).
//! Example test filter: performance_writer::seal_cost_tests::profile_trusted_storage_seal_cost
//! with --ignored --exact --nocapture. The coordinator owns Cargo execution.
//!
//! This copies trusted originals into a retained isolated storage fixture. It
//! never constructs NativeSealPermit, opens a device, changes a catalog, or
//! grants actual native normal-seal authority. The four measured phases model
//! authentication/storage work only, without any native lifecycle assertions.
use super::*;
use omniusong_recording::{AppendChunk, ChunkRecord, SealedJournal, TakeState};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    time::{Duration, Instant},
};

const SOURCE_CEILING: u64 = 512 * 1024 * 1024;
const COPY_MEMBERS: usize = 64;
const COPY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
struct FileIdentity {
    byte_length: u64,
    sha256: String,
}
fn file_identity(path: &Path) -> FileIdentity {
    let metadata = fs::symlink_metadata(path).expect("read source metadata");
    assert!(metadata.is_file() && !metadata.file_type().is_symlink());
    assert!(
        metadata.len() <= SOURCE_CEILING,
        "profile source exceeds512MiB"
    );
    let mut input = File::open(path).expect("read explicitly selected file");
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let count = input.read(&mut buffer).expect("stream selected file hash");
        if count == 0 {
            break;
        }
        bytes = bytes
            .checked_add(count as u64)
            .expect("file length overflow");
        assert!(bytes <= SOURCE_CEILING, "file grew beyond profile ceiling");
        hash.update(&buffer[..count]);
    }
    assert_eq!(bytes, metadata.len(), "file changed while hashing");
    assert_eq!(input.metadata().unwrap().len(), bytes);
    FileIdentity {
        byte_length: bytes,
        sha256: format!("{:x}", hash.finalize()),
    }
}
fn directory_names(parent: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(parent).expect("list source stage names") {
        assert!(names.len() < 128, "source stage has too many entries");
        let name = entry
            .unwrap()
            .file_name()
            .into_string()
            .expect("profile names must be UTF-8");
        assert!(name.len() <= 4096);
        names.push(name);
    }
    names.sort();
    names
}
fn row_counts(path: &Path) -> BTreeMap<String, u64> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("read-only profile row counts");
    connection
        .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
        .unwrap();
    let mut counts = BTreeMap::new();
    for table in [
        "sqlite_schema",
        "take_meta",
        "stream_prefix",
        "chunks",
        "interruptions",
        "recovery_index",
        "sealed_manifests",
        "sqlite_sequence",
    ] {
        let value: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        counts.insert(table.into(), u64::try_from(value).unwrap());
    }
    counts
}
fn append_copy(journal: &mut Journal, members: &[ChunkRecord]) {
    assert!(!members.is_empty() && members.len() <= COPY_MEMBERS);
    let chunks: Vec<_> = members
        .iter()
        .map(|chunk| AppendChunk {
            header: &chunk.header,
            payload: &chunk.payload,
        })
        .collect();
    let receipt = journal
        .append_group(&chunks)
        .expect("copy exact immutable chunk group");
    assert!(!receipt.replayed);
    assert_eq!(receipt.chunks.len(), members.len());
    for (actual, expected) in receipt.chunks.iter().zip(members) {
        assert_eq!(actual.sha256, expected.sha256);
    }
}
#[derive(Serialize)]
struct Phase {
    seconds: f64,
    nanoseconds: u64,
}
impl From<Duration> for Phase {
    fn from(value: Duration) -> Self {
        Self {
            seconds: value.as_secs_f64(),
            nanoseconds: u64::try_from(value.as_nanos())
                .expect("profile duration exceeds u64 nanoseconds"),
        }
    }
}

#[test]
#[ignore = "manual cost profile requires explicit source/receipt env; no devices or native authority"]
fn profile_trusted_storage_seal_cost() {
    let source_argument = PathBuf::from(
        std::env::var_os("OMNIUSONG_SEAL_PROFILE_SOURCE")
            .expect("set explicit OMNIUSONG_SEAL_PROFILE_SOURCE"),
    );
    let receipt_argument = PathBuf::from(
        std::env::var_os("OMNIUSONG_SEAL_PROFILE_RECEIPT")
            .expect("set fresh OMNIUSONG_SEAL_PROFILE_RECEIPT"),
    );
    assert!(source_argument.is_absolute() && receipt_argument.is_absolute());
    assert!(
        !fs::symlink_metadata(&source_argument)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let source_path = fs::canonicalize(&source_argument).unwrap();
    let source_parent = source_path.parent().unwrap();
    let receipt_parent = fs::canonicalize(receipt_argument.parent().unwrap())
        .expect("receipt parent must already exist");
    assert!(
        !receipt_parent.starts_with(source_parent),
        "receipt must be outside source stage"
    );
    let receipt_path = receipt_parent.join(receipt_argument.file_name().unwrap());
    // Reserve before creating a clone; existing receipt refusal has no fixture effects.
    let mut receipt_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&receipt_path)
        .expect("receipt must be fresh");
    let names_before = directory_names(source_parent);
    let source_before = file_identity(&source_path);

    let started = Instant::now();
    let source = CaptureReader::open(&source_path).expect("read-only source open");
    let source_open = started.elapsed();
    let original = source.inspection();
    assert_eq!(original.journal_sha256, source_before.sha256);
    assert_eq!(original.byte_length, source_before.byte_length);
    assert!(original.report.problems.is_empty(), "damaged source prefix");
    assert!(
        original
            .manifest
            .as_ref()
            .is_none_or(|manifest| manifest.body.interrupted.is_none()),
        "recovered source cannot model normal seal cost"
    );
    assert!(matches!(
        original.report.state,
        TakeState::Recording | TakeState::Sealed
    ));
    assert_eq!(
        descriptor_profile(&original.descriptor).unwrap(),
        PerformanceProfile::FinishV2
    );
    assert!(
        original
            .report
            .streams
            .iter()
            .all(|prefix| prefix.gap_units == 0 && prefix.gap_chunks == 0)
    );
    let started = Instant::now();
    let source_semantic = inspect_performance(&source).expect("source complete semantic preflight");
    let source_semantic_preflight = started.elapsed();
    assert!(
        source_semantic.problem.is_none()
            && source_semantic.capture_fault.is_none()
            && !source_semantic.capture_fault_pending
    );
    let finish = source_semantic
        .finish_cut
        .expect("source must have complete real stored Finish evidence");
    let source_selection = source_semantic
        .selection
        .as_ref()
        .expect("source complete checkpoint");
    assert_eq!(
        source_selection.chunk_count, original.report.verified_chunks,
        "no source tail may be skipped"
    );
    assert_eq!(
        source_selection.chunk_index_sha256,
        original.report.chunk_index_sha256
    );
    let checkpoint = source_semantic.checkpoint.as_ref().unwrap();
    assert_eq!(checkpoint.audio_end, finish.cut_frame);
    assert_eq!(checkpoint.last_callback_index, finish.callback_index);
    let source_rows = row_counts(&source_path);
    assert_eq!(
        source_rows["chunks"], original.report.verified_chunks,
        "all physical chunks must be copied"
    );

    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.runtime/rust-bounded-inspection/seal-cost");
    fs::create_dir_all(&base).unwrap();
    let private_directory = base.join(uuid::Uuid::new_v4().to_string());
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&private_directory)
        .expect("fresh private profile directory");
    let private_directory = fs::canonicalize(private_directory).unwrap();
    let clone_path = private_directory.join("take.sqlite3");
    let started = Instant::now();
    let mut journal = Journal::create(&clone_path, &original.descriptor).unwrap();
    let mut members = Vec::with_capacity(COPY_MEMBERS);
    let mut group_bytes = 0usize;
    let mut copy_groups = 0u64;
    for ordinal in 1..=original.report.verified_chunks {
        let chunk = source
            .read_ordinal(ordinal)
            .expect("read every selected source chunk");
        let size = chunk
            .header
            .canonical_bytes()
            .unwrap()
            .len()
            .checked_add(chunk.payload.len())
            .unwrap();
        assert!(size <= COPY_BYTES);
        if !members.is_empty()
            && (members.len() == COPY_MEMBERS
                || group_bytes.checked_add(size).unwrap() > COPY_BYTES)
        {
            append_copy(&mut journal, &members);
            copy_groups += 1;
            members.clear();
            group_bytes = 0;
        }
        group_bytes = group_bytes.checked_add(size).unwrap();
        members.push(chunk);
    }
    if !members.is_empty() {
        append_copy(&mut journal, &members);
        copy_groups += 1;
    }
    drop(members);
    let copying = started.elapsed();
    let clone_before_seal = file_identity(&clone_path);

    // These four phases mirror authentication/storage work, not lifecycle proof.
    let started = Instant::now();
    let view = journal.inspect_owned().expect("profile inspect_owned");
    let inspect_owned = started.elapsed();
    assert_eq!(view.inspection().descriptor, original.descriptor);
    assert_eq!(
        view.inspection().report.chunk_index_sha256,
        original.report.chunk_index_sha256
    );
    assert_eq!(
        view.inspection().original_rows_sha256,
        original.original_rows_sha256,
        "clone must retain the same complete original rows, not omit extensions"
    );
    let started = Instant::now();
    let semantic = inspect_performance_snapshot(view.inspection(), |ordinal| {
        view.read_ordinal(ordinal)
            .map_err(|error| error.to_string())
    })
    .expect("profile held-view semantic scan");
    let semantic_scan = started.elapsed();
    drop(view);
    assert!(
        semantic.problem.is_none()
            && semantic.capture_fault.is_none()
            && !semantic.capture_fault_pending
    );
    assert_eq!(semantic.finish_cut, Some(finish));
    assert_eq!(
        semantic.checkpoint_sha256,
        source_semantic.checkpoint_sha256
    );
    let started = Instant::now();
    let complete = journal.complete_prefix().expect("profile complete_prefix");
    let complete_prefix = started.elapsed();
    assert_eq!(semantic.selection.as_ref(), Some(&complete.prefix));
    assert_eq!(semantic.descriptor_sha256, complete.descriptor_sha256);
    assert_eq!(semantic.original_rows_sha256, complete.original_rows_sha256);
    let started = Instant::now();
    let manifest = journal
        .seal_complete_prefix("trusted-cost-profile", &complete)
        .expect("profile trusted storage seal");
    let seal_complete_prefix = started.elapsed();
    let native_equivalent_total =
        inspect_owned + semantic_scan + complete_prefix + seal_complete_prefix;
    assert!(manifest.body.interrupted.is_none());
    assert_eq!(manifest.body.chunk_count, complete.prefix.chunk_count);
    assert_eq!(
        manifest.body.chunk_index_sha256,
        complete.prefix.chunk_index_sha256
    );
    drop(journal);

    let sealed = SealedJournal::open(&clone_path).expect("verify retained clone seal");
    assert_eq!(sealed.manifest(), &manifest);
    for ordinal in 1..=original.report.verified_chunks {
        assert_eq!(
            sealed.read_ordinal(ordinal).unwrap(),
            source.read_ordinal(ordinal).unwrap(),
            "source chunk/header/payload changed at ordinal{ordinal}"
        );
    }
    let final_inspection = sealed.inspect_snapshot().unwrap();
    assert_eq!(
        final_inspection.original_rows_sha256,
        original.original_rows_sha256
    );
    drop(sealed);
    let clone_after_seal = file_identity(&clone_path);
    let clone_rows = row_counts(&clone_path);
    drop(source);
    let source_after = file_identity(&source_path);
    let names_after = directory_names(source_parent);
    assert_eq!(source_before, source_after, "original source bytes changed");
    assert_eq!(
        names_before, names_after,
        "original source directory entries changed"
    );

    let phases = serde_json::json!({
        "source_open_excluded":Phase::from(source_open),
        "source_semantic_preflight_excluded":Phase::from(source_semantic_preflight),
        "copying_excluded":Phase::from(copying),
        "inspect_owned":Phase::from(inspect_owned),
        "held_view_semantic_scan":Phase::from(semantic_scan),
        "complete_prefix":Phase::from(complete_prefix),
        "seal_complete_prefix":Phase::from(seal_complete_prefix),
        "native_work_equivalent_total":Phase::from(native_equivalent_total)
    });
    let counts = serde_json::json!({
        "source_rows":source_rows,"clone_rows_after_seal":clone_rows,
        "chunks":complete.prefix.chunk_count,"callbacks":finish.callback_index,"frames":finish.cut_frame,
        "sample_rate":manifest.body.descriptor.streams.iter().find_map(|stream|match &stream.kind {omniusong_recording::StreamKind::Audio{format,..}=>Some(format.sample_rate),_=>None}),
        "copy_groups":copy_groups,"copy_group_member_limit":COPY_MEMBERS,"copy_group_byte_limit":COPY_BYTES
    });
    let identities = serde_json::json!({
        "source_before":source_before,"source_after":source_after,"source_directory_names":names_after,
        "clone_before_seal":clone_before_seal,"clone_after_seal":clone_after_seal,
        "descriptor_sha256":complete.descriptor_sha256,"original_rows_sha256":complete.original_rows_sha256,
        "chunk_index_sha256":complete.prefix.chunk_index_sha256,"checkpoint_sha256":semantic.checkpoint_sha256,
        "manifest_sha256":manifest.sha256,"complete_prefix_sha256":hash(&serde_json::to_vec(&complete).unwrap()),
        "finish":finish
    });
    let report = serde_json::json!({
        "version":1,"kind":"trusted_storage_seal_cost_software_fixture","source":source_path,"clone":clone_path,
        "nativeAuthorityMinted":false,"nativeLifecycleProven":false,"deviceOpened":false,"catalogOpened":false,
        "originalUnchanged":true,"allSourceChunksCompared":true,"phases":phases,"counts":counts,"identities":identities,
        "limitations":"Software authentication/storage work only. Source open, preflight, copying and final comparisons are excluded from four-phase total. This is neither native retirement proof nor physical durability/latency qualification. Original source is unchanged; the fresh clone is intentionally normal-sealed by the public trusted-storage API and retained. Failure may retain a partial clone/reserved receipt; no cleanup deletes evidence."
    });
    serde_json::to_writer_pretty(&mut receipt_file, &report).unwrap();
    receipt_file.write_all(b"\n").unwrap();
    receipt_file.sync_all().unwrap();
    eprintln!(
        "Retained trusted-storage cost profile: {}",
        receipt_path.display()
    );
}
