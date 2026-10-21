//! Read-only performance-stage discovery and bounded inspection. No recovery,
//! publication, input-device access, or reconstructed recording intentions.
use crate::{FailureKind, NativeBackend, Reply, ServiceError};
use omniusong_engine::{
    performance_records::{PerformanceConfig, descriptor_profile, from_descriptor},
    performance_writer::{PerformanceInspection, inspect_performance_bounded},
};
use omniusong_recording::{
    CaptureReader, InspectionBudget, InspectionLimits, InspectionUsage, RecordingError,
    RecoveryProblem, TakeState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

const MAX_ENTRIES: usize = 128;
const MAX_REPLY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSource {
    pub stage_id: String,
    pub journal_path: PathBuf,
    /// Directory discovery does not open SQLite or establish source validity.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceScan {
    pub sources: Vec<PerformanceSource>,
    pub examined_entries: usize,
    pub skipped_generic: usize,
    pub skipped_unknown: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceSourceReview {
    pub version: u32,
    pub stage_id: String,
    pub journal_path: PathBuf,
    pub take_id: String,
    pub profile_version: u16,
    pub config: PerformanceConfig,
    pub state: TakeState,
    pub byte_length: u64,
    pub manifest_sha256: Option<String>,
    pub manifest_interrupted: bool,
    pub verified_payload_bytes: u64,
    pub generic_problems: Vec<RecoveryProblem>,
    pub performance: PerformanceInspection,
    pub usage: InspectionUsage,
    /// Read-only review identity, never a recovery or normal-seal capability.
    pub review_sha256: String,
}

struct ByteBudget(usize);
impl Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_REPLY_BYTES.saturating_sub(self.0) {
            return Err(std::io::Error::other(
                "Performance source reply exceeds 64 KiB",
            ));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bounded<T: Serialize>(value: &T) -> Result<(), ServiceError> {
    serde_json::to_writer(ByteBudget(0), value).map_err(|e| unavailable(e.to_string()))
}
fn unavailable(message: impl Into<String>) -> ServiceError {
    ServiceError {
        kind: FailureKind::SourceUnavailable,
        message: message.into(),
        outcome_unknown: false,
    }
}
fn source_error(error: RecordingError) -> ServiceError {
    let kind = match error {
        RecordingError::InspectionCancelled => FailureKind::InspectionCancelled,
        RecordingError::InspectionLimit { .. } => FailureKind::InspectionLimit,
        RecordingError::Owned => FailureKind::SourceBusy,
        _ => FailureKind::SourceUnavailable,
    };
    ServiceError {
        kind,
        message: error.to_string(),
        outcome_unknown: false,
    }
}
fn require(ok: bool, message: &str) -> Result<(), ServiceError> {
    if ok {
        Ok(())
    } else {
        Err(unavailable(message))
    }
}
pub(super) fn validate_stage_id(id: &str) -> Result<(), ServiceError> {
    require(
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && id.bytes().any(|b| b != b'0'),
        "Choose an exact retained performance identity from its source list",
    )
}
fn directory(path: &Path) -> Result<(), ServiceError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| unavailable(e.to_string()))?;
    require(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "Performance source directory must be ordinary and non-symlink",
    )?;
    require(
        fs::canonicalize(path).map_err(|e| unavailable(e.to_string()))? == path,
        "Performance source directory redirects outside its owned namespace",
    )
}
fn ordinary(path: &Path) -> Result<(), ServiceError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| unavailable(e.to_string()))?;
    require(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Performance original must be an ordinary non-symlink file",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        require(
            metadata.nlink() == 1,
            "Hard-linked performance originals are unsupported",
        )?;
    }
    require(
        fs::canonicalize(path).map_err(|e| unavailable(e.to_string()))? == path,
        "Performance original redirects outside its owned stage",
    )
}
fn capture_root(root: &Path) -> Result<Option<PathBuf>, ServiceError> {
    directory(root)?;
    let path = root.join("captures");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(unavailable(e.to_string())),
        Ok(_) => {
            directory(&path)?;
            Ok(Some(path))
        }
    }
}
fn source_path(root: &Path, id: &str) -> Result<PathBuf, ServiceError> {
    validate_stage_id(id)?;
    let root = capture_root(root)?
        .ok_or_else(|| unavailable("No retained performance directory exists"))?;
    let stage = root.join(id);
    directory(&stage)?;
    let path = stage.join("take.sqlite3");
    ordinary(&path)?;
    Ok(path)
}
fn scan(root: &Path) -> Result<PerformanceSourceScan, ServiceError> {
    let mut result = PerformanceSourceScan::default();
    let Some(path) = capture_root(root)? else {
        return Ok(result);
    };
    let mut entries = fs::read_dir(&path).map_err(|e| unavailable(e.to_string()))?;
    for entry in entries.by_ref().take(MAX_ENTRIES) {
        result.examined_entries += 1;
        let entry = entry.map_err(|e| unavailable(e.to_string()))?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            result.skipped_unknown += 1;
            continue;
        };
        if uuid::Uuid::parse_str(&id).is_ok_and(|u| u.hyphenated().to_string() == id) {
            result.skipped_generic += 1;
            continue;
        }
        if validate_stage_id(&id).is_err() {
            result.skipped_unknown += 1;
            continue;
        }
        let checked = source_path(root, &id);
        result.sources.push(PerformanceSource {
            stage_id: id.clone(),
            journal_path: path.join(id).join("take.sqlite3"),
            problem: checked.err().map(|e| e.message.chars().take(512).collect()),
        });
        if bounded(&result).is_err() {
            result.sources.pop();
            result.truncated = true;
            break;
        }
    }
    // One unexamined sentinel entry detects truncation; no metadata/source is read.
    if entries.next().is_some() {
        result.truncated = true;
    }
    result.sources.sort_by(|a, b| a.stage_id.cmp(&b.stage_id));
    bounded(&result)?;
    Ok(result)
}
fn inspect(
    root: &Path,
    stage_id: &str,
    cancelled: Arc<AtomicBool>,
    limits: InspectionLimits,
) -> Result<PerformanceSourceReview, ServiceError> {
    let budget = InspectionBudget::new(limits, cancelled).map_err(source_error)?;
    budget.checkpoint().map_err(source_error)?;
    let path = source_path(root, stage_id)?;
    let reader = CaptureReader::open_bounded(&path, &budget).map_err(source_error)?;
    let source = reader.inspection();
    let config = from_descriptor(&source.descriptor).map_err(unavailable)?;
    require(
        config.take_id.0 == format!("performance-{stage_id}"),
        "Performance take identity differs from its owned stage",
    )?;
    let profile = descriptor_profile(&source.descriptor).map_err(unavailable)?;
    let performance = inspect_performance_bounded(&reader).map_err(source_error)?;
    budget.checkpoint().map_err(source_error)?;
    let mut review = PerformanceSourceReview {
        version: 1,
        stage_id: stage_id.into(),
        journal_path: path,
        take_id: config.take_id.0.clone(),
        profile_version: profile.version(),
        config,
        state: source.report.state,
        byte_length: source.byte_length,
        manifest_sha256: source.manifest.as_ref().map(|m| m.sha256.clone()),
        manifest_interrupted: source
            .manifest
            .as_ref()
            .is_some_and(|m| m.body.interrupted.is_some()),
        verified_payload_bytes: source.report.verified_payload_bytes,
        generic_problems: source.report.problems.clone(),
        performance,
        usage: budget.usage(),
        review_sha256: String::new(),
    };
    bounded(&review)?;
    let bytes = serde_json::to_vec(&review).map_err(|e| unavailable(e.to_string()))?;
    let mut hash = Sha256::new();
    hash.update(b"OmniuSong.performance-source-review.v1\0");
    hash.update(bytes);
    review.review_sha256 = format!("{:x}", hash.finalize());
    bounded(&review)?;
    budget.checkpoint().map_err(source_error)?;
    Ok(review)
}

impl NativeBackend {
    pub(super) fn scan_performance_sources(&mut self) -> Result<Reply, ServiceError> {
        let root = self.store()?.root().to_path_buf();
        Ok(Reply::PerformanceSources(scan(&root)?))
    }
    pub(super) fn inspect_performance_source(
        &mut self,
        stage_id: &str,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Reply, ServiceError> {
        let root = self.store()?.root().to_path_buf();
        Ok(Reply::PerformanceSourceInspected(Box::new(inspect(
            &root,
            stage_id,
            cancelled,
            InspectionLimits::default(),
        )?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Backend, Command, LegacyScan, Library, Paths, RequestId, SessionService, SubmitFailure,
    };
    use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
    use omniusong_engine::{
        LiveClock, LiveId, LiveScope, LiveSourceBinding, LiveSourceKind,
        performance_records::PerformanceSource as OriginalSource, performance_tap::TapInfo,
        performance_writer::PerformanceWriter,
    };
    use std::{
        sync::{atomic::Ordering, mpsc},
        time::{Duration, Instant},
    };

    const ID: &str = "12121212121212121212121212121212";
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/performance-source-session-tests")
                .join(uuid::Uuid::new_v4().to_string());
            fs::create_dir_all(&path).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
        fn backend(&self) -> NativeBackend {
            let mut backend = NativeBackend {
                paths: Paths {
                    native: self.0.join("native"),
                    legacy: self.0.join("legacy"),
                },
                store: None,
                legacy: LegacyScan::default(),
                legacy_notice: None,
                require_existing: false,
            };
            backend.handle(&Command::Initialize).unwrap();
            backend
        }
        fn source(&self, root: &Path, id: &str) -> PathBuf {
            let directory = root.join("captures").join(id);
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("take.sqlite3");
            let config = PerformanceConfig {
                take_id: TakeId(format!("performance-{id}")),
                info: TapInfo {
                    scope: LiveScope {
                        worker_generation: LiveId([1; 16]),
                        session_id: LiveId([2; 16]),
                        panic_generation: 1,
                    },
                    sample_rate: 48_000,
                    channels: 1,
                },
                gui_clock: LiveClock {
                    domain: "synthetic-gui".into(),
                    epoch: "synthetic-epoch".into(),
                    ticks_per_second_numerator: 1_000_000_000,
                    ticks_per_second_denominator: 1,
                },
                output_epoch: ClockEpochId("synthetic-output".into()),
                sources: std::array::from_fn(|i| OriginalSource {
                    binding: LiveSourceBinding {
                        kind: [
                            LiveSourceKind::ComputerKeys,
                            LiveSourceKind::PointerKeys,
                            LiveSourceKind::Pads,
                            LiveSourceKind::AccessiblePulse,
                        ][i],
                        handle: i as u8 + 1,
                        connection_generation: 1,
                    },
                    endpoint: EndpointId(format!("synthetic-source-{i}")),
                }),
            };
            let mut writer = PerformanceWriter::create_finishable(&path, config.clone()).unwrap();
            let (_admission, mut renderer, mut owner) =
                omniusong_engine::live::recording_live_pair(config.info.scope, 48_000, 1).unwrap();
            renderer.render_into(&mut [0.; 128]);
            while let Some(packet) = owner.try_pop().unwrap() {
                writer.push_packet(packet).unwrap();
            }
            writer.flush().unwrap();
            drop((renderer, owner, writer));
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn token() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn digest(path: &Path) -> String {
        format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
    }
    fn names(path: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn discovery_distinguishes_namespaces_and_never_opens_the_source() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        let path = f.source(&root, ID);
        fs::create_dir(root.join("captures").join(uuid::Uuid::new_v4().to_string())).unwrap();
        fs::create_dir(root.join("captures").join("unknown")).unwrap();
        let original = digest(&path);
        let listing = names(path.parent().unwrap());
        let s = scan(&root).unwrap();
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.examined_entries, 3);
        assert_eq!(s.skipped_generic, 1);
        assert_eq!(s.skipped_unknown, 1);
        assert!(!s.truncated);
        assert_eq!(s.sources[0].stage_id, ID);
        assert!(s.sources[0].problem.is_none());
        assert_eq!(original, digest(&path));
        assert_eq!(listing, names(path.parent().unwrap()));
    }
    #[test]
    fn discovery_is_bounded_by_all_entries_and_known_stage_remains_inspectable() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        let path = f.source(&root, ID);
        for i in 0..140 {
            fs::create_dir(root.join("captures").join(format!("unrelated-{i}"))).unwrap();
        }
        let s = scan(&root).unwrap();
        assert_eq!(s.examined_entries, 128);
        assert!(s.truncated);
        assert!(inspect(&root, ID, token(), InspectionLimits::default()).is_ok());
        assert!(path.is_file());
    }
    #[test]
    fn review_preserves_all_source_bytes_and_has_a_strict_bounded_fingerprint() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        let path = f.source(&root, ID);
        let original = digest(&path);
        let listing = names(path.parent().unwrap());
        let review = inspect(&root, ID, token(), InspectionLimits::default()).unwrap();
        assert_eq!(review.profile_version, 2);
        assert_eq!(review.take_id, format!("performance-{ID}"));
        assert_eq!(review.performance.journal_sha256_at_review, original);
        assert!(review.performance.selection.is_some());
        assert!(review.performance.unknown_tail);
        assert!(review.manifest_sha256.is_none());
        assert!(review.usage.record_visits > 0);
        assert!(serde_json::to_vec(&review).unwrap().len() < MAX_REPLY_BYTES);
        let mut canonical = review.clone();
        canonical.review_sha256.clear();
        let mut h = Sha256::new();
        h.update(b"OmniuSong.performance-source-review.v1\0");
        h.update(serde_json::to_vec(&canonical).unwrap());
        assert_eq!(review.review_sha256, format!("{:x}", h.finalize()));
        let mut bad = serde_json::to_value(&review).unwrap();
        bad["invented"] = true.into();
        assert!(serde_json::from_value::<PerformanceSourceReview>(bad).is_err());
        assert_eq!(original, digest(&path));
        assert_eq!(listing, names(path.parent().unwrap()));
    }
    #[test]
    fn cancellation_and_limit_are_local_failures_and_catalog_remains_usable() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        let path = f.source(&root, ID);
        let original = digest(&path);
        let cancelled = token();
        cancelled.store(true, Ordering::Release);
        assert_eq!(
            inspect(&root, ID, cancelled, InspectionLimits::default())
                .unwrap_err()
                .kind,
            FailureKind::InspectionCancelled
        );
        let limits = InspectionLimits {
            max_file_bytes: 0,
            ..InspectionLimits::default()
        };
        let e = inspect(&root, ID, token(), limits).unwrap_err();
        assert_eq!(e.kind, FailureKind::InspectionLimit);
        assert!(!e.outcome_unknown);
        assert!(
            b.handle(&Command::Create {
                name: "Healthy after refused original".into()
            })
            .is_ok()
        );
        assert_eq!(original, digest(&path));
    }
    #[test]
    fn busy_damaged_and_missing_sources_do_not_quarantine_catalog() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        let path = f.source(&root, ID);
        let owner = CaptureReader::open(&path).unwrap();
        let e = b
            .handle(&Command::InspectPerformanceSource {
                stage_id: ID.into(),
            })
            .unwrap_err();
        assert_eq!(e.kind, FailureKind::SourceBusy);
        drop(owner);
        fs::write(&path, b"deliberately malformed retained original").unwrap();
        let before = digest(&path);
        assert_eq!(
            b.handle(&Command::InspectPerformanceSource {
                stage_id: ID.into()
            })
            .unwrap_err()
            .kind,
            FailureKind::SourceUnavailable
        );
        assert_eq!(digest(&path), before);
        assert_eq!(
            b.handle(&Command::InspectPerformanceSource {
                stage_id: "13131313131313131313131313131313".into()
            })
            .unwrap_err()
            .kind,
            FailureKind::SourceUnavailable
        );
        assert!(b.handle(&Command::Refresh).is_ok());
    }
    #[test]
    fn arbitrary_or_zero_stage_id_is_rejected_before_submission() {
        for id in [
            "",
            "../outside",
            "00000000000000000000000000000000",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "12121212-1212-1212-1212-121212121212",
        ] {
            assert!(
                Command::InspectPerformanceSource {
                    stage_id: id.into()
                }
                .validate()
                .is_err()
            );
        }
        for command in [
            Command::ScanPerformanceSources,
            Command::InspectPerformanceSource {
                stage_id: ID.into(),
            },
        ] {
            assert!(command.retry_safe());
            assert!(!command.mutates());
        }
    }
    #[cfg(unix)]
    #[test]
    fn redirected_stage_is_reported_without_following_or_modifying_it() {
        let f = Fixture::new();
        let mut b = f.backend();
        let root = b.store().unwrap().root().to_path_buf();
        fs::create_dir(root.join("captures")).unwrap();
        let outside = f.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("take.sqlite3"), b"outside preserved").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("captures").join(ID)).unwrap();
        assert!(scan(&root).unwrap().sources[0].problem.is_some());
        assert!(inspect(&root, ID, token(), InspectionLimits::default()).is_err());
        assert_eq!(
            fs::read(outside.join("take.sqlite3")).unwrap(),
            b"outside preserved"
        );
    }

    struct PausedInspection {
        entered: mpsc::SyncSender<Arc<AtomicBool>>,
        release: mpsc::Receiver<()>,
    }
    impl Backend for PausedInspection {
        fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
            Ok(Reply::Library(Library::default()))
        }
        fn handle_with_cancel(
            &mut self,
            command: &Command,
            cancelled: &Arc<AtomicBool>,
        ) -> Result<Reply, ServiceError> {
            if matches!(command, Command::InspectPerformanceSource { .. }) {
                self.entered.send(Arc::clone(cancelled)).unwrap();
                self.release.recv_timeout(Duration::from_secs(3)).unwrap();
                Err(ServiceError {
                    kind: if cancelled.load(Ordering::Acquire) {
                        FailureKind::InspectionCancelled
                    } else {
                        FailureKind::SourceUnavailable
                    },
                    message: "isolated inspection completion".into(),
                    outcome_unknown: false,
                })
            } else {
                self.handle(command)
            }
        }
    }
    fn paused() -> (
        SessionService,
        mpsc::Receiver<Arc<AtomicBool>>,
        mpsc::SyncSender<()>,
    ) {
        let (entered, receive) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        (
            SessionService::spawn_internal(PausedInspection {
                entered,
                release: wait,
            })
            .unwrap(),
            receive,
            release,
        )
    }
    fn completion(service: &mut SessionService) -> crate::Completion {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(reply) = service.try_poll().unwrap() {
                return reply;
            }
            assert!(Instant::now() < end);
            std::thread::yield_now();
        }
    }
    #[test]
    fn correlated_cancel_retains_credit_and_stale_cancel_cannot_reach_next_request() {
        let (mut service, entered, release) = paused();
        let first = service
            .try_submit(Command::InspectPerformanceSource {
                stage_id: ID.into(),
            })
            .unwrap();
        let flag = entered.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!service.cancel_source_inspection(RequestId {
            sequence: first.sequence + 1,
            ..first
        }));
        assert!(!flag.load(Ordering::Acquire));
        assert!(service.cancel_source_inspection(first));
        assert!(service.is_busy());
        assert_eq!(
            service.try_submit(Command::Refresh).unwrap_err().kind,
            SubmitFailure::Busy
        );
        release.send(()).unwrap();
        let reply = completion(&mut service);
        assert_eq!(reply.id, first);
        assert_eq!(
            reply.result.unwrap_err().kind,
            FailureKind::InspectionCancelled
        );
        assert!(!service.is_disconnected());
        let second = service
            .try_submit(Command::InspectPerformanceSource {
                stage_id: ID.into(),
            })
            .unwrap();
        let next = entered.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(!next.load(Ordering::Acquire));
        assert!(!service.cancel_source_inspection(first));
        assert!(!next.load(Ordering::Acquire));
        release.send(()).unwrap();
        assert_eq!(completion(&mut service).id, second);
        service.try_submit(Command::Refresh).unwrap();
        assert!(completion(&mut service).result.is_ok());
    }
    #[test]
    fn cancellation_cannot_target_a_mutation_and_drop_does_not_wait() {
        let (mut service, entered, release) = paused();
        let id = service
            .try_submit(Command::InspectPerformanceSource {
                stage_id: ID.into(),
            })
            .unwrap();
        let flag = entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let before = Instant::now();
        drop(service);
        assert!(before.elapsed() < Duration::from_millis(100));
        assert!(flag.load(Ordering::Acquire));
        release.send(()).unwrap();
        let (mut client, peer) = SessionService::test_pair();
        let request = client
            .try_submit(Command::Create {
                name: "mutation".into(),
            })
            .unwrap();
        assert!(!client.cancel_source_inspection(request));
        assert!(!client.cancel_source_inspection(id));
        drop(peer);
    }
}
