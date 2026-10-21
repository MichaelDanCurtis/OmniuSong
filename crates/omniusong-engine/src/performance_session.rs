//! Native output recording ownership. No driver, SQL, wait or join on IPC calls.
//! Callback/drop proofs remain private; public status is observation only.
use crate::{
    CaptureIdentity, LiveBatch, LiveFaultKind, LiveId, LiveProgress, LiveReady, LiveScope,
    LiveSourceBinding, LiveSourceKind, LiveState, LiveSubmitError,
    live::{LiveAdmission, LiveControl, LiveErrorGuard, LiveRenderer, recording_live_pair},
    performance_records::{PerformanceConfig, PerformanceSource},
    performance_tap::{PerformanceTapOwner, TAP_SLOTS, TapControl, TapFinish},
    performance_types::{
        NativeRetirementStatus, PerformanceIdentity, PerformancePhase, PerformanceStart,
        PerformanceStatus,
    },
    performance_writer::PerformanceWriter,
};
use cpal::traits::{DeviceTrait, StreamTrait};
use omniusong_core::contracts::{ClockEpochId, EndpointId, TakeId};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const SEAL_AUTHORIZED: u8 = 2;
const STARTING: u8 = 1;
const FINISHING: u8 = 2;
const STOPPING: u8 = 3;
const DRAINING: u8 = 4;
const SEALING: u8 = 5;
const WATCH_SHIFT: u32 = 3;
const WATCH_PHASE_MASK: u64 = (1 << WATCH_SHIFT) - 1;
const MAX_WATCH_MILLIS: u64 = u64::MAX >> WATCH_SHIFT;
const START_TIMEOUT: Duration = Duration::from_secs(10);
const FINISH_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const SEAL_TIMEOUT: Duration = Duration::from_secs(10);

fn bounded_error(message: &str) -> String {
    let text: String = message.chars().filter(|c| *c != '\0').take(3_900).collect();
    if text.is_empty() {
        "Recorded output interrupted".into()
    } else {
        text
    }
}

struct Controls {
    live: Arc<LiveControl>,
    tap: Arc<TapControl>,
    outer_renderer: Arc<AtomicBool>,
    outer_error: Arc<AtomicBool>,
}
impl Controls {
    fn destroyed(&self) -> bool {
        self.outer_renderer.load(Ordering::Acquire)
            && self.outer_error.load(Ordering::Acquire)
            && self.live.render_destroyed()
            && self.live.error_destroyed()
            && self.tap.snapshot().is_ok_and(|s| s.producer_destroyed)
    }
    fn observations(&self, paused: bool) -> NativeRetirementStatus {
        NativeRetirementStatus {
            pause_succeeded: paused,
            renderer_destroyed: self.outer_renderer.load(Ordering::Acquire)
                && self.live.render_destroyed(),
            error_destroyed: self.outer_error.load(Ordering::Acquire)
                && self.live.error_destroyed(),
            producer_destroyed: self.tap.snapshot().is_ok_and(|s| s.producer_destroyed),
        }
    }
}
struct Shared {
    status: Mutex<PerformanceStatus>,
    controls: OnceLock<Arc<Controls>>,
    finish: OnceLock<crate::LiveFinishFence>,
    cut: OnceLock<TapFinish>,
    retire: Arc<AtomicBool>,
    gate: AtomicU8,
    stop_requested: AtomicBool,
    owner_gone: AtomicBool,
    native_done: AtomicBool,
    native_safe: AtomicBool,
    writer_done: AtomicBool,
    writer_running: AtomicBool,
    retry: AtomicU64,
    epoch: Instant,
    watch_word: AtomicU64,
    reason: SyncSender<String>,
}
impl Shared {
    fn edit(&self, change: impl FnOnce(&mut PerformanceStatus)) {
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        change(&mut status);
        if let Some(fence) = self.finish.get() {
            status.finish = Some(*fence);
        }
        if let Some(controls) = self.controls.get()
            && let Ok(Some(cut)) = controls.live.finish_ack(&controls.tap)
        {
            let _ = self.cut.set(cut);
        }
        if let Some(cut) = self.cut.get() {
            status.finish = Some(cut.fence);
            status.cut = Some(*cut);
        }
        status.revision = status.revision.saturating_add(1);
    }
    fn error(&self, message: &str) {
        let message = bounded_error(message);
        self.edit(|s| {
            if s.error.is_none() {
                s.error = Some(message);
            }
            if matches!(
                s.phase,
                PerformancePhase::Starting
                    | PerformancePhase::Recording
                    | PerformancePhase::Finishing
            ) {
                s.phase = PerformancePhase::Stopping;
            }
        });
    }
    /// Nonblocking caller path. The first cancel competes with exactly one
    /// normal-seal authorization; later Stop cannot roll that authorization back.
    fn cancel(&self, reason: &str) -> bool {
        self.stop_requested.store(true, Ordering::Release);
        let won = self
            .gate
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if self.gate.load(Ordering::Acquire) == CANCELLED {
            self.watch(STOPPING, STOP_TIMEOUT);
        }
        if won && let Some(controls) = self.controls.get() {
            controls.tap.cancel();
            controls.live.fault(LiveFaultKind::RecordingTap, None);
        }
        let _ = self.reason.try_send(bounded_error(reason));
        won
    }
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
    fn watch(&self, phase: u8, duration: Duration) {
        let deadline = self
            .now()
            .saturating_add(duration.as_millis().min(u128::from(MAX_WATCH_MILLIS)) as u64)
            .clamp(1, MAX_WATCH_MILLIS);
        let next = (deadline << WATCH_SHIFT) | u64::from(phase);
        let mut observed = self.watch_word.load(Ordering::Acquire);
        loop {
            // A late caller must not replace a later lifecycle phase or extend
            // its deadline. Native retirement precedes the STOP -> DRAIN path.
            if observed != 0 && observed & WATCH_PHASE_MASK >= u64::from(phase) {
                return;
            }
            match self.watch_word.compare_exchange_weak(
                observed,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(current) => observed = current,
            }
        }
    }
    fn clear_observed_watch(&self, observed: u64, phase: u8) -> bool {
        observed != 0
            && observed & WATCH_PHASE_MASK == u64::from(phase)
            && self
                .watch_word
                .compare_exchange(observed, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    fn clear_watch_phase(&self, phase: u8) {
        self.clear_observed_watch(self.watch_word.load(Ordering::Acquire), phase);
    }
    fn quiescent(&self) -> bool {
        self.native_done.load(Ordering::Acquire)
            && self.native_safe.load(Ordering::Acquire)
            && self.writer_done.load(Ordering::Acquire)
    }
    fn expire(&self, now: u64) -> bool {
        let observed = self.watch_word.load(Ordering::Acquire);
        let deadline = observed >> WATCH_SHIFT;
        if observed == 0 || now < deadline || self.quiescent() {
            return false;
        }
        // Expiration claims exactly this phase/deadline. A concurrent successful
        // handoff or Stop transition cannot be retired under an older watch.
        if self
            .watch_word
            .compare_exchange(observed, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let phase = (observed & WATCH_PHASE_MASK) as u8;
        let label = match phase {
            STARTING => "starting",
            FINISHING => "finishing",
            STOPPING => "native callback retirement",
            DRAINING => "original drain",
            _ => "sealing",
        };
        self.cancel(&format!(
            "Recorded output {label} deadline expired; worker retired and original retained"
        ));
        self.retire.store(true, Ordering::Release);
        self.edit(|s| {
            s.phase = if s.sealed.is_some() {
                PerformancePhase::Sealed
            } else if self.gate.load(Ordering::Acquire) == SEAL_AUTHORIZED {
                PerformancePhase::SealUncertain
            } else {
                PerformancePhase::Failed
            };
            if s.error.is_none() {
                s.error = Some(format!(
                    "Recorded output {label} deadline expired; original retained"
                ));
            }
        });
        true
    }
}

/// Sole admission owner. Drop never waits and never frees callback backing while
/// either actual native closure may still own it, including failed handoffs.
struct AdmissionLease {
    admission: Option<LiveAdmission>,
    controls: Arc<Controls>,
    shared: Arc<Shared>,
}
impl Drop for AdmissionLease {
    fn drop(&mut self) {
        if let Some(admission) = self.admission.take() {
            if self.controls.destroyed() {
                drop(admission);
            } else {
                self.shared.cancel(
                    "Admission owner ended before native callback retirement; original retained",
                );
                self.shared.retire.store(true, Ordering::Release);
                std::mem::forget(admission);
            }
        }
    }
}
struct Ready {
    lease: AdmissionLease,
    ready: LiveReady,
    descriptor_sha256: String,
    native_sample_format: String,
}
struct Readiness {
    ready: LiveReady,
    descriptor_sha256: String,
    native_sample_format: String,
}
enum WriterEnd {
    Interrupted,
    Native(NativeSealPermit),
}

pub struct PerformanceSession {
    request: PerformanceStart,
    shared: Arc<Shared>,
    ready: Receiver<Ready>,
    admission: Option<AdmissionLease>,
    handoff: Option<Readiness>,
    cached: PerformanceStatus,
    local_error: Option<String>,
}
impl PerformanceSession {
    pub fn start(
        root: &Path,
        generation: LiveId,
        request: PerformanceStart,
        retire: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        if !root.is_absolute()
            || root.to_string_lossy().len() > 3_900
            || root.to_string_lossy().contains('\0')
            || !generation.is_valid()
            || !request.is_valid()
        {
            return Err("Invalid recorded-output Start identity, path or clock".into());
        }
        let path = root
            .join("captures")
            .join(request.directory_name())
            .join("take.sqlite3");
        let cached = initial_status(&request, path);
        let (reason_tx, reason_rx) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            status: Mutex::new(cached.clone()),
            controls: OnceLock::new(),
            finish: OnceLock::new(),
            cut: OnceLock::new(),
            retire,
            gate: AtomicU8::new(ACTIVE),
            stop_requested: AtomicBool::new(false),
            owner_gone: AtomicBool::new(false),
            native_done: AtomicBool::new(false),
            native_safe: AtomicBool::new(true),
            writer_done: AtomicBool::new(true),
            writer_running: AtomicBool::new(false),
            retry: AtomicU64::new(0),
            epoch: Instant::now(),
            watch_word: AtomicU64::new(0),
            reason: reason_tx,
        });
        shared.watch(STARTING, START_TIMEOUT);
        let watch = shared.clone();
        thread::Builder::new()
            .name("performance-deadline".into())
            .spawn(move || {
                while !watch.quiescent() && !watch.retire.load(Ordering::Acquire) {
                    if watch.expire(watch.now()) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            })
            .map_err(|e| e.to_string())?;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let owner_shared = shared.clone();
        let owner_request = request.clone();
        let owner_root = root.to_owned();
        if let Err(error) = thread::Builder::new()
            .name("performance-native-output".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    native::run(
                        &owner_root,
                        generation,
                        owner_request,
                        owner_shared.clone(),
                        ready_tx,
                        reason_rx,
                    )
                }));
                match result {
                    Ok(Ok(())) => {
                        if owner_shared.stop_requested.load(Ordering::Acquire) {
                            owner_shared.edit(|s| {
                                if s.phase == PerformancePhase::Starting {
                                    s.phase = PerformancePhase::Interrupted;
                                }
                            });
                        }
                    }
                    Ok(Err(error)) => {
                        owner_shared.cancel(&error);
                        owner_shared.error(&error);
                        owner_shared.edit(|s| {
                            if s.phase != PerformancePhase::Sealed {
                                s.phase = PerformancePhase::Failed;
                            }
                        });
                    }
                    Err(_) => {
                        owner_shared.cancel("Recorded output owner panicked; original retained");
                        owner_shared.error("Recorded output owner panicked; original retained");
                        owner_shared.retire.store(true, Ordering::Release);
                        owner_shared.edit(|s| s.phase = PerformancePhase::Failed);
                    }
                }
                owner_shared.native_done.store(true, Ordering::Release);
            })
        {
            shared.native_done.store(true, Ordering::Release);
            shared.cancel("Cannot start native output owner");
            return Err(error.to_string());
        }
        Ok(Self {
            request,
            shared,
            ready: ready_rx,
            admission: None,
            handoff: None,
            cached,
            local_error: None,
        })
    }
    pub fn request(&self) -> &PerformanceStart {
        &self.request
    }
    fn receive_ready(&mut self) {
        if self.admission.is_some() {
            return;
        }
        if let Ok(ready) = self.ready.try_recv() {
            // This can also run through admit()/begin_finish(). Keep the last
            // published snapshot untouched until snapshot() compares/commits a
            // complete candidate, including when its shared lock is held.
            self.handoff = Some(Readiness {
                ready: ready.ready,
                descriptor_sha256: ready.descriptor_sha256,
                native_sample_format: ready.native_sample_format,
            });
            self.admission = Some(ready.lease);
        }
    }
    pub fn snapshot(&mut self) -> PerformanceStatus {
        self.receive_ready();
        let mut next = self
            .shared
            .status
            .try_lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| self.cached.clone());
        if let Some(lease) = &self.admission {
            let mut live = lease.admission.as_ref().expect("sole admission").progress();
            if self.is_quiescent() {
                live.state = LiveState::Stopped;
                live.active_voices = 0;
                live.release_tail_voices = 0;
                live.lease_credit = None;
            }
            if next.ready.is_none()
                && let Some(handoff) = &self.handoff
            {
                next.ready = Some(handoff.ready.clone());
                next.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
            }
            if next.native_sample_format.is_none()
                && let Some(handoff) = &self.handoff
            {
                next.native_sample_format = Some(handoff.native_sample_format.clone());
            }
            if next.phase == PerformancePhase::Starting
                && next.error.is_none()
                && !self.shared.stop_requested.load(Ordering::Acquire)
            {
                next.phase = PerformancePhase::Recording;
            }
            next.live = Some(live);
        } else {
            // Shared preparation can be published just before the bounded
            // handoff. Do not advertise note admission until we own its lease.
            next.ready = None;
            next.live = None;
            if next.phase == PerformancePhase::Recording {
                next.phase = PerformancePhase::Starting;
            }
        }
        if let Some(fence) = self.shared.finish.get() {
            next.finish = Some(*fence);
            if next.phase == PerformancePhase::Recording {
                next.phase = PerformancePhase::Finishing;
            }
        }
        if let Some(cut) = self.shared.cut.get() {
            next.cut = Some(*cut);
            next.finish = Some(cut.fence);
        }
        if self.shared.stop_requested.load(Ordering::Acquire)
            && matches!(
                next.phase,
                PerformancePhase::Starting
                    | PerformancePhase::Recording
                    | PerformancePhase::Finishing
            )
        {
            next.phase = if self.shared.gate.load(Ordering::Acquire) == SEAL_AUTHORIZED {
                PerformancePhase::Sealing
            } else {
                PerformancePhase::Stopping
            };
        }
        // A first error already sent to the parent is immutable. A later
        // background failure remains separately visible in live/writer faults.
        if self.cached.error.is_some() {
            next.error = self.cached.error.clone();
        } else if next.error.is_none() {
            next.error = self.local_error.clone();
        }
        next.revision = self.cached.revision;
        if next != self.cached {
            next.revision = next.revision.saturating_add(1);
        }
        self.cached = next;
        self.cached.clone()
    }
    fn admission(&mut self) -> Result<&mut LiveAdmission, LiveSubmitError> {
        self.receive_ready();
        if self.shared.retire.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Retired);
        }
        if self.shared.stop_requested.load(Ordering::Acquire) {
            return Err(LiveSubmitError::Faulted);
        }
        self.admission
            .as_mut()
            .and_then(|lease| lease.admission.as_mut())
            .ok_or(LiveSubmitError::NotLive)
    }
    pub fn admit(&mut self, batch: LiveBatch) -> Result<LiveProgress, LiveSubmitError> {
        self.admission()?.admit(batch)
    }
    pub fn begin_panic(&mut self, scope: LiveScope) -> Result<LiveScope, LiveSubmitError> {
        self.admission()?.begin_panic(scope)
    }
    pub fn finish_panic(&mut self) -> Result<Option<LiveProgress>, LiveSubmitError> {
        self.admission()?.finish_panic()
    }
    pub fn begin_finish(
        &mut self,
        scope: LiveScope,
        id: LiveId,
    ) -> Result<PerformanceStatus, String> {
        if let Some(old) = self.shared.finish.get() {
            if old.scope != scope || old.id != id {
                return Err("Different recorded-output Finish intent".into());
            }
            let current = self.snapshot();
            if current.phase == PerformancePhase::SealUncertain
                && !self.shared.writer_done.load(Ordering::Acquire)
                && !self.shared.stop_requested.load(Ordering::Acquire)
            {
                self.shared.retry.fetch_add(1, Ordering::AcqRel);
            }
            return Ok(self.snapshot());
        }
        let fence = self
            .admission()
            .map_err(|e| e.to_string())?
            .begin_finish(scope, id)
            .map_err(|e| e.to_string())?;
        let _ = self.shared.finish.set(fence);
        self.shared.watch(FINISHING, FINISH_TIMEOUT);
        Ok(self.snapshot())
    }
    pub fn cancel(&mut self, reason: &str) -> PerformanceStatus {
        let current = self.snapshot();
        if current.phase == PerformancePhase::Sealed && current.sealed.is_some() {
            self.shared.stop_requested.store(true, Ordering::Release);
            return current;
        }
        if self.shared.gate.load(Ordering::Acquire) != SEAL_AUTHORIZED {
            self.local_error
                .get_or_insert_with(|| bounded_error(reason));
        }
        self.shared.cancel(reason);
        if self.shared.gate.load(Ordering::Acquire) != SEAL_AUTHORIZED {
            self.shared.watch(STOPPING, STOP_TIMEOUT);
        }
        self.snapshot()
    }
    pub fn fault(
        &mut self,
        scope: LiveScope,
        kind: LiveFaultKind,
        sequence: Option<u64>,
    ) -> Result<LiveProgress, String> {
        if self.is_quiescent() {
            return Err("Recorded output is already quiescent".into());
        }
        let admission = self.admission().map_err(|e| e.to_string())?;
        if admission.scope() != scope || sequence == Some(0) {
            return Err("Stale recorded-output fault scope or sequence".into());
        }
        let control = admission.control();
        // Same linearization as Stop: an IPC fault may not mutate controls
        // after a healthy native seal won. Real native callbacks are already
        // destroyed before that authorization can exist.
        self.shared
            .gate
            .compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|gate| {
                if gate == SEAL_AUTHORIZED {
                    "Native sealing already authorized; late IPC fault rejected".to_string()
                } else {
                    "Recorded output already interrupted".to_string()
                }
            })?;
        self.shared.stop_requested.store(true, Ordering::Release);
        control.fault(kind, sequence);
        if let Some(controls) = self.shared.controls.get() {
            controls.tap.cancel();
        }
        let reason = format!("Recorded output fault: {kind:?}; original retained");
        self.local_error.get_or_insert_with(|| reason.clone());
        let _ = self.shared.reason.try_send(reason);
        self.shared.watch(STOPPING, STOP_TIMEOUT);
        Ok(self
            .admission
            .as_ref()
            .and_then(|lease| lease.admission.as_ref())
            .expect("faulted sole admission")
            .progress())
    }
    pub fn is_quiescent(&self) -> bool {
        self.shared.quiescent()
    }
}
impl Drop for PerformanceSession {
    fn drop(&mut self) {
        self.shared.owner_gone.store(true, Ordering::Release);
        self.shared
            .cancel("Recorded-output owner closed; original retained");
        // No joins, native calls or SQL. AdmissionLease conservatively retains
        // backing if a ready handoff or callback lifetime is still unresolved.
    }
}
fn initial_status(request: &PerformanceStart, path: PathBuf) -> PerformanceStatus {
    PerformanceStatus {
        revision: 1,
        identity: PerformanceIdentity {
            start_id: request.id,
            capture: CaptureIdentity {
                journal_path: path,
                take_id: request.take_id(),
            },
            descriptor_sha256: None,
        },
        phase: PerformancePhase::Starting,
        ready: None,
        live: None,
        writer: None,
        finish: None,
        cut: None,
        native: NativeRetirementStatus::default(),
        native_sample_format: None,
        sealed: None,
        error: None,
    }
}

/// The constructor and concrete CPAL lifetime proof are confined to this module.
mod native {
    use super::*;
    pub(crate) struct NativeSealPermit {
        descriptor: String,
        take: String,
        start: LiveId,
        cut: TapFinish,
        controls: Arc<Controls>,
        shared: Arc<Shared>,
    }
    impl NativeSealPermit {
        pub(crate) fn descriptor_sha256(&self) -> &str {
            &self.descriptor
        }
        pub(crate) fn take_id(&self) -> &str {
            &self.take
        }
        pub(crate) fn start_id(&self) -> LiveId {
            self.start
        }
        pub(crate) fn cut(&self) -> TapFinish {
            self.cut
        }
        pub(crate) fn authorize_seal(&self) -> Result<(), String> {
            if !self.controls.destroyed() || !self.shared.native_safe.load(Ordering::Acquire) {
                return Err("Actual recorded-output callback retirement is unproven".into());
            }
            if self
                .controls
                .live
                .finish_ack(&self.controls.tap)
                .map_err(|e| e.to_string())?
                != Some(self.cut)
            {
                return Err("Recorded-output cut or final fault changed before sealing".into());
            }
            match self.shared.gate.compare_exchange(
                ACTIVE,
                SEAL_AUTHORIZED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) | Err(SEAL_AUTHORIZED) => Ok(()),
                _ => Err(
                    "Stop won before native seal authorization; original retained unsealed".into(),
                ),
            }
        }
    }
    struct RenderState {
        renderer: Option<LiveRenderer>,
        outer: Arc<AtomicBool>,
    }
    impl RenderState {
        fn f32(&mut self, data: &mut [f32]) {
            self.renderer
                .as_mut()
                .expect("actual renderer")
                .render_into(data);
        }
        fn i16(&mut self, data: &mut [i16]) {
            self.renderer
                .as_mut()
                .expect("actual renderer")
                .render_i16(data);
        }
        fn u16(&mut self, data: &mut [u16]) {
            self.renderer
                .as_mut()
                .expect("actual renderer")
                .render_u16(data);
        }
    }
    impl Drop for RenderState {
        fn drop(&mut self) {
            drop(self.renderer.take());
            self.outer.store(true, Ordering::Release);
        }
    }
    struct ErrorState {
        guard: Option<LiveErrorGuard>,
        outer: Arc<AtomicBool>,
    }
    impl ErrorState {
        fn fault(&self) {
            self.guard.as_ref().expect("actual error guard").fault();
        }
    }
    impl Drop for ErrorState {
        fn drop(&mut self) {
            drop(self.guard.take());
            self.outer.store(true, Ordering::Release);
        }
    }
    #[cfg(test)]
    pub(super) fn drop_software_renderer(renderer: LiveRenderer, controls: &Controls) {
        drop(RenderState {
            renderer: Some(renderer),
            outer: controls.outer_renderer.clone(),
        });
    }
    #[cfg(test)]
    pub(super) fn drop_software_error(error: LiveErrorGuard, controls: &Controls) {
        drop(ErrorState {
            guard: Some(error),
            outer: controls.outer_error.clone(),
        });
    }
    struct Prepared {
        renderer: Option<RenderState>,
        errors: Option<ErrorState>,
        lease: Option<AdmissionLease>,
        shared: Arc<Shared>,
        controls: Arc<Controls>,
    }
    impl Drop for Prepared {
        fn drop(&mut self) {
            drop(self.renderer.take());
            drop(self.errors.take());
            if self.controls.destroyed() {
                self.shared.native_safe.store(true, Ordering::Release);
            }
            drop(self.lease.take());
        }
    }
    struct Output {
        stream: Option<cpal::Stream>,
        shared: Arc<Shared>,
    }
    impl Output {
        fn stop(&mut self, controls: &Controls) -> Result<(), String> {
            self.shared.watch(STOPPING, STOP_TIMEOUT);
            let stream = self
                .stream
                .take()
                .ok_or("Recorded output stream ownership missing")?;
            if let Err(error) = stream.pause() {
                std::mem::forget(stream);
                self.shared.retire.store(true, Ordering::Release);
                return Err(format!(
                    "Recorded output pause failed; worker retired: {error}"
                ));
            }
            drop(stream);
            let deadline = Instant::now() + STOP_TIMEOUT;
            while !controls.destroyed() {
                self.shared.edit(|s| s.native = controls.observations(true));
                if Instant::now() >= deadline || self.shared.retire.load(Ordering::Acquire) {
                    self.shared.retire.store(true, Ordering::Release);
                    return Err(
                        "Recorded output outer callback retirement timed out; worker retired"
                            .into(),
                    );
                }
                thread::sleep(Duration::from_millis(1));
            }
            self.shared.native_safe.store(true, Ordering::Release);
            self.shared.edit(|s| s.native = controls.observations(true));
            Ok(())
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            if let Some(stream) = self.stream.take() {
                self.shared.retire.store(true, Ordering::Release);
                std::mem::forget(stream);
            }
        }
    }
    fn ordinary(path: &Path) -> Result<(), String> {
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || fs::canonicalize(path).map_err(|e| e.to_string())? != path
        {
            return Err("Recorded-output directory must be canonical and ordinary".into());
        }
        Ok(())
    }
    fn create_private(path: &Path) -> Result<(), String> {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).map_err(|e| e.to_string())?;
        ordinary(path)
    }
    fn prepare_path(root: &Path, request: &PerformanceStart) -> Result<PathBuf, String> {
        ordinary(root)?;
        let captures = root.join("captures");
        match fs::symlink_metadata(&captures) {
            Ok(_) => ordinary(&captures)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_private(&captures)?
            }
            Err(error) => return Err(error.to_string()),
        }
        let stage = captures.join(request.directory_name());
        // A new worker never resumes/replaces even an empty prior stage.
        create_private(&stage)?;
        Ok(stage.join("take.sqlite3"))
    }
    pub(super) fn run(
        root: &Path,
        generation: LiveId,
        request: PerformanceStart,
        shared: Arc<Shared>,
        ready: SyncSender<Ready>,
        reason: Receiver<String>,
    ) -> Result<(), String> {
        if shared.stop_requested.load(Ordering::Acquire) {
            return Ok(());
        }
        let path = prepare_path(root, &request)?;
        if shared.stop_requested.load(Ordering::Acquire) {
            return Ok(());
        }
        let device = crate::worker::output_device(request.output_device.as_deref())?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let config = supported.config();
        let sample_format = match supported.sample_format() {
            cpal::SampleFormat::F32 => "f32",
            cpal::SampleFormat::I16 => "i16",
            cpal::SampleFormat::U16 => "u16",
            _ => return Err("Recorded output supports only native f32/i16/u16 conversion".into()),
        };
        let scope = LiveScope {
            worker_generation: generation,
            session_id: request.id,
            panic_generation: 1,
        };
        let (admission, renderer, tap) =
            recording_live_pair(scope, config.sample_rate, config.channels)?;
        shared.native_safe.store(false, Ordering::Release);
        let controls = Arc::new(Controls {
            live: admission.control(),
            tap: tap.control(),
            outer_renderer: Arc::new(AtomicBool::new(false)),
            outer_error: Arc::new(AtomicBool::new(false)),
        });
        let _ = shared.controls.set(controls.clone());
        let mut prepared = Prepared {
            renderer: Some(RenderState {
                renderer: Some(renderer),
                outer: controls.outer_renderer.clone(),
            }),
            errors: Some(ErrorState {
                guard: Some(LiveErrorGuard::new(controls.live.clone())),
                outer: controls.outer_error.clone(),
            }),
            lease: Some(AdmissionLease {
                admission: Some(admission),
                controls: controls.clone(),
                shared: shared.clone(),
            }),
            shared: shared.clone(),
            controls: controls.clone(),
        };
        let kinds = [
            LiveSourceKind::ComputerKeys,
            LiveSourceKind::PointerKeys,
            LiveSourceKind::Pads,
            LiveSourceKind::AccessiblePulse,
        ];
        let recording = PerformanceConfig {
            take_id: TakeId(request.take_id()),
            info: tap.info(),
            gui_clock: request.gui_clock.clone(),
            output_epoch: ClockEpochId(format!("performance-output-{}", request.directory_name())),
            sources: std::array::from_fn(|index| PerformanceSource {
                binding: LiveSourceBinding {
                    kind: kinds[index],
                    handle: (index + 1) as u8,
                    connection_generation: 1,
                },
                endpoint: EndpointId(format!(
                    "performance-gui-{}-{}",
                    index + 1,
                    request.directory_name()
                )),
            }),
        };
        let writer = PerformanceWriter::create_finishable(&path, recording.clone())?;
        let descriptor = writer.descriptor().sha256().map_err(|e| e.to_string())?;
        shared.edit(|s| {
            s.identity.descriptor_sha256 = Some(descriptor.clone());
            s.native_sample_format = Some(sample_format.into());
        });
        let (end_tx, end_rx) = mpsc::sync_channel(1);
        shared.writer_done.store(false, Ordering::Release);
        let writer_shared = shared.clone();
        if let Err(error) = thread::Builder::new()
            .name("performance-original-writer".into())
            .spawn(move || {
                writer_shared.writer_running.store(true, Ordering::Release);
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    writer_run(writer, tap, end_rx, writer_shared.clone())
                }));
                if outcome.is_err() {
                    writer_shared.cancel("Performance writer panicked; original retained");
                    writer_shared.error("Performance writer panicked; original retained");
                    writer_shared.retire.store(true, Ordering::Release);
                }
                writer_shared.writer_done.store(true, Ordering::Release);
            })
        {
            shared.writer_done.store(true, Ordering::Release);
            return Err(format!("Cannot start performance drainer: {error}"));
        }
        while !shared.writer_running.load(Ordering::Acquire) {
            if shared.retire.load(Ordering::Acquire)
                || shared.stop_requested.load(Ordering::Acquire)
            {
                return Err("Recorded-output setup cancelled before drainer readiness".into());
            }
            thread::sleep(Duration::from_millis(1));
        }
        if shared.stop_requested.load(Ordering::Acquire) {
            let _ = end_tx.try_send(WriterEnd::Interrupted);
            return Ok(());
        }
        let mut render = prepared.renderer.take().expect("concrete render wrapper");
        let errors = prepared.errors.take().expect("concrete error wrapper");
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                config,
                move |data: &mut [f32], _| render.f32(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                config,
                move |data: &mut [i16], _| render.i16(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                config,
                move |data: &mut [u16], _| render.u16(data),
                move |_| errors.fault(),
                Some(Duration::from_secs(2)),
            ),
            _ => unreachable!(),
        }
        .map_err(|e| {
            format!(
                "Recorded-output setup failed; original retained at {}: {e}",
                path.display()
            )
        })?;
        let mut output = Output {
            stream: Some(stream),
            shared: shared.clone(),
        };
        if let Err(error) = output.stream.as_ref().expect("actual output").play() {
            shared.cancel("Recorded-output start failed; original retained");
            let stopped = output.stop(&controls);
            let _ = end_tx.try_send(WriterEnd::Interrupted);
            stopped?;
            return Err(format!("Recorded-output start failed: {error}"));
        }
        let live_ready = LiveReady {
            scope,
            gui_clock: request.gui_clock.clone(),
            sources: recording.sources.iter().map(|s| s.binding).collect(),
            output: crate::DeviceInfo {
                id: device.id().map_err(|e| e.to_string())?.to_string(),
                name: device.to_string(),
            },
            sample_rate: config.sample_rate,
            channels: config.channels,
            progress: prepared
                .lease
                .as_ref()
                .and_then(|l| l.admission.as_ref())
                .expect("sole setup admission")
                .progress(),
        };
        if !live_ready.is_valid() {
            shared.cancel("Recorded output faulted before Ready; original retained");
            output.stop(&controls)?;
            let _ = end_tx.try_send(WriterEnd::Interrupted);
            return Err("Recorded output faulted before Ready".into());
        }
        shared.edit(|s| {
            s.ready = Some(live_ready.clone());
            s.live = Some(live_ready.progress);
            s.phase = PerformancePhase::Recording;
        });
        let handoff = Ready {
            lease: prepared.lease.take().expect("one admission handoff"),
            ready: live_ready,
            descriptor_sha256: descriptor.clone(),
            native_sample_format: sample_format.into(),
        };
        let starting_watch = shared.watch_word.load(Ordering::Acquire);
        let mut failed_handoff = None;
        if let Err(error) = ready.try_send(handoff) {
            failed_handoff = Some(match error {
                TrySendError::Full(value) | TrySendError::Disconnected(value) => value,
            });
            shared.cancel("Recorded output Ready handoff failed; original retained");
        } else {
            // Ready is actually queued. CAS only the observed START watch: an
            // intervening Stop/Finish owns its own deadline and must survive.
            shared.clear_observed_watch(starting_watch, STARTING);
        }
        let mut cut = None;
        loop {
            while let Ok(message) = reason.try_recv() {
                shared.error(&message);
            }
            if shared.stop_requested.load(Ordering::Acquire)
                || shared.retire.load(Ordering::Acquire)
            {
                shared.edit(|s| s.phase = PerformancePhase::Stopping);
                break;
            }
            match controls.live.finish_ack(&controls.tap) {
                Ok(Some(finish)) => {
                    let _ = shared.cut.set(finish);
                    shared.edit(|s| {
                        s.finish = Some(finish.fence);
                        s.cut = Some(finish);
                        s.phase = PerformancePhase::Stopping;
                    });
                    cut = Some(finish);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    shared.cancel(&format!("Recorded output callback/tap fault: {error}"));
                }
            }
            if shared.finish.get().is_some() {
                shared.edit(|s| s.phase = PerformancePhase::Finishing);
            }
            thread::sleep(Duration::from_millis(1));
        }
        output.stop(&controls)?;
        drop(failed_handoff);
        shared.watch(DRAINING, DRAIN_TIMEOUT);
        let terminal = if let Some(cut) = cut {
            if controls.live.finish_ack(&controls.tap) == Ok(Some(cut))
                && shared.gate.load(Ordering::Acquire) == ACTIVE
            {
                WriterEnd::Native(NativeSealPermit {
                    descriptor,
                    take: request.take_id(),
                    start: request.id,
                    cut,
                    controls: controls.clone(),
                    shared: shared.clone(),
                })
            } else {
                WriterEnd::Interrupted
            }
        } else {
            WriterEnd::Interrupted
        };
        if end_tx.try_send(terminal).is_err() {
            return Err("Performance drainer ended before native retirement delivery".into());
        }
        Ok(())
    }
}
pub(crate) use native::NativeSealPermit;

fn writer_run(
    mut writer: PerformanceWriter,
    mut tap: PerformanceTapOwner,
    end: Receiver<WriterEnd>,
    shared: Arc<Shared>,
) {
    let mut terminal = None;
    let mut failed = false;
    loop {
        let mut empty = false;
        if !failed {
            for _ in 0..TAP_SLOTS {
                match tap.try_pop() {
                    Ok(Some(packet)) => {
                        if let Err(error) = writer.push_packet(packet) {
                            shared.error(&error);
                            shared.cancel(&error);
                            failed = true;
                            break;
                        }
                    }
                    Ok(None) => {
                        empty = true;
                        break;
                    }
                    Err(crate::performance_tap::TapError::Busy) => break,
                    Err(error) => {
                        shared.error(&error.to_string());
                        shared.cancel(&error.to_string());
                        failed = true;
                        break;
                    }
                }
            }
        }
        let progress = writer.progress();
        shared.edit(|s| s.writer = Some(progress));
        if terminal.is_none() {
            match end.try_recv() {
                Ok(value) => terminal = Some(value),
                Err(TryRecvError::Disconnected) => terminal = Some(WriterEnd::Interrupted),
                Err(TryRecvError::Empty) => {}
            }
        }
        let destroyed = tap.control().snapshot().is_ok_and(|s| s.producer_destroyed);
        if terminal.is_some() && destroyed && (empty || failed) {
            match terminal.take().expect("terminal decision") {
                WriterEnd::Native(permit)
                    if !failed && shared.gate.load(Ordering::Acquire) != CANCELLED =>
                {
                    let mut retry = shared.retry.load(Ordering::Acquire);
                    loop {
                        shared.edit(|s| s.phase = PerformancePhase::Sealing);
                        shared.watch(SEALING, SEAL_TIMEOUT);
                        match writer.seal_native(&permit) {
                            Ok(sealed) => {
                                shared.edit(|s| {
                                    s.writer = Some(writer.progress());
                                    s.sealed = Some(sealed);
                                    s.phase = PerformancePhase::Sealed;
                                });
                                shared.clear_watch_phase(SEALING);
                                return;
                            }
                            Err(error) => {
                                shared.error(&error);
                                shared.edit(|s| s.phase = PerformancePhase::SealUncertain);
                                // Exact writer + private permit are retained. No
                                // generic recovery, new ID, or rerender occurs.
                                while shared.retry.load(Ordering::Acquire) == retry {
                                    if shared.stop_requested.load(Ordering::Acquire)
                                        || shared.owner_gone.load(Ordering::Acquire)
                                        || shared.retire.load(Ordering::Acquire)
                                    {
                                        return;
                                    }
                                    thread::sleep(Duration::from_millis(2));
                                }
                                retry = shared.retry.load(Ordering::Acquire);
                            }
                        }
                    }
                }
                _ => {
                    match writer.close_interrupted() {
                        Ok(retained) => shared.edit(|s| {
                            s.writer = Some(retained.progress);
                            s.phase = PerformancePhase::Interrupted;
                        }),
                        Err(error) => {
                            shared.error(&error);
                            shared.edit(|s| s.phase = PerformancePhase::Failed);
                        }
                    }
                    shared.clear_watch_phase(DRAINING);
                    return;
                }
            }
        }
        if shared.retire.load(Ordering::Acquire) && shared.native_done.load(Ordering::Acquire) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LiveFinishFence;
    use crate::{LiveClock, LiveEvent, LiveEventKind, LivePatch};

    fn request() -> PerformanceStart {
        PerformanceStart {
            id: LiveId([7; 16]),
            output_device: None,
            gui_clock: LiveClock {
                domain: "software-session-clock".into(),
                epoch: "software-session-epoch".into(),
                ticks_per_second_numerator: 1_000_000_000,
                ticks_per_second_denominator: 1,
            },
        }
    }
    fn shell() -> (PerformanceSession, SyncSender<Ready>, Receiver<String>) {
        let request = request();
        let cached = initial_status(
            &request,
            PathBuf::from("/software-only/captures/07070707070707070707070707070707/take.sqlite3"),
        );
        let (reason, reasons) = mpsc::sync_channel(1);
        let shared = Arc::new(Shared {
            status: Mutex::new(cached.clone()),
            controls: OnceLock::new(),
            finish: OnceLock::new(),
            cut: OnceLock::new(),
            retire: Arc::new(AtomicBool::new(false)),
            gate: AtomicU8::new(ACTIVE),
            stop_requested: AtomicBool::new(false),
            owner_gone: AtomicBool::new(false),
            native_done: AtomicBool::new(false),
            native_safe: AtomicBool::new(true),
            writer_done: AtomicBool::new(true),
            writer_running: AtomicBool::new(false),
            retry: AtomicU64::new(0),
            epoch: Instant::now(),
            watch_word: AtomicU64::new(0),
            reason,
        });
        let (ready, receiver) = mpsc::sync_channel(1);
        (
            PerformanceSession {
                request,
                shared,
                ready: receiver,
                admission: None,
                handoff: None,
                cached,
                local_error: None,
            },
            ready,
            reasons,
        )
    }
    fn software_pair(
        shared: &Arc<Shared>,
    ) -> (
        AdmissionLease,
        LiveRenderer,
        LiveErrorGuard,
        PerformanceTapOwner,
    ) {
        let scope = LiveScope {
            worker_generation: LiveId([9; 16]),
            session_id: request().id,
            panic_generation: 1,
        };
        let (admission, renderer, tap) = recording_live_pair(scope, 48_000, 2).unwrap();
        let controls = Arc::new(Controls {
            live: admission.control(),
            tap: tap.control(),
            outer_renderer: Arc::new(AtomicBool::new(false)),
            outer_error: Arc::new(AtomicBool::new(false)),
        });
        shared.native_safe.store(false, Ordering::Release);
        assert!(shared.controls.set(controls.clone()).is_ok());
        let error = LiveErrorGuard::new(controls.live.clone());
        (
            AdmissionLease {
                admission: Some(admission),
                controls,
                shared: shared.clone(),
            },
            renderer,
            error,
            tap,
        )
    }
    fn software_ready(lease: AdmissionLease) -> Ready {
        let scope = lease.admission.as_ref().unwrap().scope();
        Ready {
            ready: LiveReady {
                scope,
                gui_clock: request().gui_clock,
                sources: [
                    LiveSourceKind::ComputerKeys,
                    LiveSourceKind::PointerKeys,
                    LiveSourceKind::Pads,
                    LiveSourceKind::AccessiblePulse,
                ]
                .into_iter()
                .enumerate()
                .map(|(i, kind)| LiveSourceBinding {
                    kind,
                    handle: (i + 1) as u8,
                    connection_generation: 1,
                })
                .collect(),
                output: crate::DeviceInfo {
                    id: "software-only".into(),
                    name: "No device opened".into(),
                },
                sample_rate: 48_000,
                channels: 2,
                progress: lease.admission.as_ref().unwrap().progress(),
            },
            lease,
            descriptor_sha256: "a".repeat(64),
            native_sample_format: "f32".into(),
        }
    }

    #[test]
    fn starting_identity_is_stable_and_no_early_admission_or_implicit_retry_exists() {
        let (mut session, _ready, _reasons) = shell();
        let start = session.snapshot();
        assert!(start.is_valid());
        assert_eq!(start.identity.start_id, request().id);
        assert_eq!(start.identity.capture.take_id, request().take_id());
        assert_eq!(start.identity.descriptor_sha256, None);
        assert_eq!(session.request(), &request());
        let scope = LiveScope {
            worker_generation: LiveId([9; 16]),
            session_id: request().id,
            panic_generation: 1,
        };
        assert_eq!(
            session.admit(LiveBatch::empty(scope, 1, None)),
            Err(LiveSubmitError::NotLive)
        );
        assert!(!session.is_quiescent());
        let stopped = session.cancel("Explicit software-only Stop");
        assert_eq!(stopped.phase, PerformancePhase::Stopping);
        assert!(stopped.is_valid());
        assert_eq!(stopped.identity, start.identity);
        assert!(session.admit(LiveBatch::empty(scope, 1, None)).is_err());
    }

    #[test]
    fn snapshot_and_drop_do_not_wait_for_a_held_background_status_lock() {
        let (mut session, _ready, _reasons) = shell();
        let shared = session.shared.clone();
        let held = shared.status.lock().unwrap();
        assert_eq!(session.snapshot().phase, PerformancePhase::Starting);
        let start = Instant::now();
        drop(session);
        assert!(start.elapsed() < Duration::from_millis(500));
        assert!(shared.owner_gone.load(Ordering::Acquire));
        assert_eq!(shared.gate.load(Ordering::Acquire), CANCELLED);
        drop(held);
    }

    #[test]
    fn phase_deadline_is_independent_and_repeated_stop_cannot_extend_it() {
        let (mut session, _ready, _reasons) = shell();
        session.cancel("Stop one");
        let watch = session.shared.watch_word.load(Ordering::Acquire);
        session.cancel("Stop two");
        assert_eq!(session.shared.watch_word.load(Ordering::Acquire), watch);
        assert!(session.shared.expire(watch >> WATCH_SHIFT));
        assert!(session.shared.retire.load(Ordering::Acquire));
        assert!(!session.is_quiescent());
        let snapshot = session.snapshot();
        assert_eq!(snapshot.phase, PerformancePhase::Failed);
        assert!(snapshot.is_valid());
    }

    #[test]
    fn real_software_closures_are_independently_required_and_do_not_claim_native_quiescence() {
        let (session, _ready, _reasons) = shell();
        let (lease, renderer, error, tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        native::drop_software_renderer(renderer, &controls);
        assert!(controls.live.render_destroyed());
        assert!(tap.control().snapshot().unwrap().producer_destroyed);
        assert!(controls.outer_renderer.load(Ordering::Acquire));
        assert!(!controls.destroyed(), "actual error wrapper remains alive");
        assert!(!controls.outer_error.load(Ordering::Acquire));
        native::drop_software_error(error, &controls);
        assert!(controls.live.error_destroyed());
        assert!(controls.outer_error.load(Ordering::Acquire));
        assert!(controls.destroyed());
        drop(lease);
        assert!(!session.shared.retire.load(Ordering::Acquire));
        assert!(
            !session.is_quiescent(),
            "software drops do not manufacture a native-owner completion"
        );
    }

    #[test]
    fn abandoned_queued_ready_retains_backing_and_retires_without_waiting() {
        let (session, ready, _reasons) = shell();
        let (lease, renderer, error, tap) = software_pair(&session.shared);
        let shared = session.shared.clone();
        let controls = lease.controls.clone();
        assert!(ready.try_send(software_ready(lease)).is_ok());
        drop(session); // Drops queued AdmissionLease before either closure.
        assert!(shared.retire.load(Ordering::Acquire));
        assert!(tap.control().snapshot().unwrap().cancelled);
        assert!(!controls.live.render_destroyed());
        // Deliberate bounded owner retention is the exceptional path. Actual
        // process retirement, not this test, is responsible for reclamation.
        drop(renderer);
        drop(error);
        drop(tap);
    }

    #[test]
    fn finish_uses_real_accepted_watermark_and_same_intent_retry_never_reopens_notes() {
        let (mut session, ready, _reasons) = shell();
        let (lease, mut renderer, error, mut tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        let handoff = software_ready(lease);
        session.shared.edit(|s| {
            s.ready = Some(handoff.ready.clone());
            s.live = Some(handoff.ready.progress);
            s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
            s.phase = PerformancePhase::Recording;
        });
        assert!(ready.try_send(handoff).is_ok());
        let initial = session.snapshot();
        assert!(initial.is_valid());
        let scope = initial.ready.unwrap().scope;
        let mut batch = LiveBatch::empty(scope, 1, None);
        batch
            .try_push(LiveEvent {
                source: 1,
                connection_generation: 1,
                observed_ticks: 9_007_199_254_740_993,
                event: LiveEventKind::NoteOn {
                    note: 1,
                    voice: 1,
                    key: 60,
                    velocity16: u16::MAX,
                    patch: LivePatch::Keys,
                },
            })
            .unwrap();
        session.admit(batch).unwrap();
        let id = LiveId([0x44; 16]);
        let finishing = session.begin_finish(scope, id).unwrap();
        assert_eq!(finishing.finish.unwrap().accepted_sequence, Some(1));
        assert!(finishing.is_valid());
        assert_eq!(
            session.begin_finish(scope, id).unwrap().finish,
            finishing.finish
        );
        assert!(session.begin_finish(scope, LiveId([0x45; 16])).is_err());
        assert_eq!(
            session.admit(LiveBatch::empty(scope, 2, None)),
            Err(LiveSubmitError::FinishPending)
        );
        renderer.render_into(&mut [0.0; 512]);
        while tap.try_pop().unwrap().is_some() {}
        let cut = controls.live.finish_ack(&controls.tap).unwrap().unwrap();
        assert_eq!(cut.fence, finishing.finish.unwrap());
        assert_eq!(cut.voices_cut, 1);
        assert!(!session.is_quiescent());
        native::drop_software_renderer(renderer, &controls);
        native::drop_software_error(error, &controls);
        session.shared.native_safe.store(true, Ordering::Release);
        session.shared.native_done.store(true, Ordering::Release);
        assert!(session.is_quiescent());
        let stopped = session.snapshot();
        assert_eq!(stopped.live.unwrap().state, LiveState::Stopped);
        assert!(stopped.is_valid());
    }

    #[test]
    fn cancel_and_seal_authorization_have_one_winner_without_rollback_claim() {
        let (mut cancelled, _ready, _reasons) = shell();
        cancelled.cancel("Stop before authorization");
        assert_eq!(
            cancelled.shared.gate.compare_exchange(
                ACTIVE,
                SEAL_AUTHORIZED,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Err(CANCELLED)
        );
        let (mut authorized, _ready, _reasons) = shell();
        assert_eq!(
            authorized.shared.gate.compare_exchange(
                ACTIVE,
                SEAL_AUTHORIZED,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Ok(ACTIVE)
        );
        let status = authorized.cancel("Stop after authorization; commit may already exist");
        assert_eq!(
            authorized.shared.gate.load(Ordering::Acquire),
            SEAL_AUTHORIZED
        );
        assert_eq!(status.phase, PerformancePhase::Sealing);
        assert!(status.sealed.is_none());
        assert!(
            status.error.is_none(),
            "Stop observation is separate from an actual storage failure"
        );
    }

    #[test]
    fn first_published_error_survives_a_later_background_cause() {
        let (mut session, _ready, _reasons) = shell();
        let first = session.cancel("First published local interruption A");
        assert_eq!(
            first.error.as_deref(),
            Some("First published local interruption A")
        );
        session.shared.error("Later background cause B");
        session.shared.edit(|s| s.phase = PerformancePhase::Failed);
        let later = session.snapshot();
        assert_eq!(later.phase, PerformancePhase::Failed);
        assert_eq!(later.error, first.error);
        assert!(later.is_valid());
    }

    fn committed_observation_fixture()
    -> (PerformanceSession, LiveScope, LiveId, crate::CapturedTake) {
        let (mut session, ready, _reasons) = shell();
        let (lease, mut renderer, error, mut tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        let handoff = software_ready(lease);
        session.shared.edit(|s| {
            s.ready = Some(handoff.ready.clone());
            s.live = Some(handoff.ready.progress);
            s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
            s.phase = PerformancePhase::Recording;
        });
        assert!(ready.try_send(handoff).is_ok());
        let scope = session.snapshot().ready.unwrap().scope;
        let finish_id = LiveId([0x61; 16]);
        session.begin_finish(scope, finish_id).unwrap();
        renderer.render_into(&mut [0.0; 256]);
        while tap.try_pop().unwrap().is_some() {}
        let cut = controls.live.finish_ack(&controls.tap).unwrap().unwrap();
        let _ = session.shared.cut.set(cut);
        native::drop_software_renderer(renderer, &controls);
        native::drop_software_error(error, &controls);
        session.shared.native_safe.store(true, Ordering::Release);
        session.shared.native_done.store(true, Ordering::Release);
        session
            .shared
            .gate
            .store(SEAL_AUTHORIZED, Ordering::Release);
        // A public observation fixture ONLY: no NativeSealPermit is constructed
        // and no journal is sealed by these device-free software drops.
        let captured = crate::CapturedTake {
            journal_path: session.cached.identity.capture.journal_path.clone(),
            take_id: request().take_id(),
            manifest_sha256: "b".repeat(64),
            frames: cut.cut_frame,
            sample_rate: 48_000,
            channels: 2,
            gap_frames: 0,
        };
        let progress = crate::performance_writer::WriterProgress {
            received_packets: 4,
            complete_callbacks: 1,
            complete_frames: cut.cut_frame,
            durable_callbacks: 1,
            durable_frames: cut.cut_frame,
            durable_prefixes: crate::performance_records::STREAM_IDS
                .iter()
                .enumerate()
                .map(|(i, id)| omniusong_recording::StreamPrefix {
                    stream_id: (*id).into(),
                    next_sequence: if i == 0 { 1 } else { 0 },
                    source_end: if i == 0 {
                        omniusong_recording::SourcePosition::AudioFrames(cut.cut_frame)
                    } else {
                        omniusong_recording::SourcePosition::Events(0)
                    },
                    recorded_units: if i == 0 { cut.cut_frame } else { 0 },
                    gap_units: 0,
                    gap_chunks: 0,
                })
                .collect(),
            checkpoint_sha256: Some("c".repeat(64)),
            first_error: None,
            capture_fault: None,
            capture_fault_pending: false,
            finish_cut: Some(cut),
        };
        session.shared.edit(|s| {
            s.phase = PerformancePhase::Sealed;
            s.native = controls.observations(true);
            s.sealed = Some(captured.clone());
            s.writer = Some(progress);
        });
        (session, scope, finish_id, captured)
    }

    #[test]
    fn stop_after_committed_observation_keeps_success_and_exact_finish_identity() {
        let (mut session, scope, finish_id, captured) = committed_observation_fixture();
        let before = session.snapshot();
        assert!(before.is_valid());
        let after = session.cancel("Reliable Stop after the committed observation");
        assert_eq!(after, before);
        assert!(after.error.is_none());
        assert_eq!(after.sealed, Some(captured));
        assert_eq!(session.begin_finish(scope, finish_id).unwrap(), before);
        assert!(session.begin_finish(scope, LiveId([0x62; 16])).is_err());
        assert_eq!(session.shared.retry.load(Ordering::Acquire), 0);
    }

    #[test]
    fn stale_finish_watch_cannot_replace_stop_or_later_disk_deadlines() {
        let (session, _ready, _reasons) = shell();
        // Simulate begin_finish paused after fence publication, while a fault
        // has already installed the shorter native-retirement deadline.
        session
            .shared
            .finish
            .set(LiveFinishFence {
                id: LiveId([0x63; 16]),
                scope: LiveScope {
                    worker_generation: LiveId([9; 16]),
                    session_id: request().id,
                    panic_generation: 1,
                },
                accepted_sequence: None,
            })
            .unwrap();
        session
            .shared
            .cancel("Fault while Finish publication is paused");
        let stop = session.shared.watch_word.load(Ordering::Acquire);
        assert_eq!(stop & WATCH_PHASE_MASK, u64::from(STOPPING));
        session.shared.watch(FINISHING, FINISH_TIMEOUT);
        assert_eq!(session.shared.watch_word.load(Ordering::Acquire), stop);

        // The shell has never owned callbacks. Mark that empty native owner
        // retired before exercising the production STOP -> DRAIN transition.
        session.shared.native_done.store(true, Ordering::Release);
        session.shared.writer_done.store(false, Ordering::Release);
        session.shared.watch(DRAINING, DRAIN_TIMEOUT);
        let drain = session.shared.watch_word.load(Ordering::Acquire);
        assert_eq!(drain & WATCH_PHASE_MASK, u64::from(DRAINING));
        for phase in [STARTING, FINISHING, STOPPING, DRAINING] {
            session.shared.watch(phase, Duration::from_secs(60));
            assert_eq!(session.shared.watch_word.load(Ordering::Acquire), drain);
        }
        session.shared.watch(SEALING, SEAL_TIMEOUT);
        let seal = session.shared.watch_word.load(Ordering::Acquire);
        assert_eq!(seal & WATCH_PHASE_MASK, u64::from(SEALING));
        for phase in [STARTING, FINISHING, STOPPING, DRAINING, SEALING] {
            session.shared.watch(phase, Duration::from_secs(60));
            assert_eq!(session.shared.watch_word.load(Ordering::Acquire), seal);
        }
        session.shared.writer_done.store(true, Ordering::Release);
    }

    #[test]
    fn sealing_deadline_after_manifest_publication_preserves_known_sealed_status() {
        let (mut session, _scope, _finish_id, captured) = committed_observation_fixture();
        let before = session.snapshot();
        assert!(before.is_valid());
        // Deterministic interval after the validated manifest is published but
        // before the writer's final destructor has acknowledged completion.
        session.shared.writer_done.store(false, Ordering::Release);
        session.shared.watch(SEALING, SEAL_TIMEOUT);
        let seal = session.shared.watch_word.load(Ordering::Acquire);
        assert!(!session.is_quiescent());
        assert!(session.shared.expire(seal >> WATCH_SHIFT));
        assert!(session.shared.retire.load(Ordering::Acquire));
        let after = session.snapshot();
        assert_eq!(after.phase, PerformancePhase::Sealed);
        assert_eq!(after.sealed, Some(captured));
        assert_eq!(after.finish, before.finish);
        assert_eq!(after.cut, before.cut);
        assert_eq!(after.native, before.native);
        assert_eq!(after.writer, before.writer);
        assert!(after.error.is_some(), "cleanup timeout remains observable");
        assert!(after.revision > before.revision);
        assert!(after.is_valid());
        session.shared.writer_done.store(true, Ordering::Release);
    }

    #[test]
    fn ipc_fault_wins_gate_before_requested_cause_or_cannot_mutate_authorized_controls() {
        for already_authorized in [false, true] {
            let (mut session, ready, _reasons) = shell();
            let (lease, renderer, error, tap) = software_pair(&session.shared);
            let controls = lease.controls.clone();
            let handoff = software_ready(lease);
            session.shared.edit(|s| {
                s.ready = Some(handoff.ready.clone());
                s.live = Some(handoff.ready.progress);
                s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
                s.phase = PerformancePhase::Recording;
            });
            assert!(ready.try_send(handoff).is_ok());
            let scope = session.snapshot().ready.unwrap().scope;
            if already_authorized {
                session
                    .shared
                    .gate
                    .store(SEAL_AUTHORIZED, Ordering::Release);
            }
            let result = session.fault(scope, LiveFaultKind::QueueOverflow, Some(1));
            if already_authorized {
                assert!(result.is_err());
                assert!(
                    session
                        .admission
                        .as_ref()
                        .unwrap()
                        .admission
                        .as_ref()
                        .unwrap()
                        .progress()
                        .fault
                        .is_none()
                );
                assert!(!tap.control().snapshot().unwrap().cancelled);
            } else {
                let fault = result.unwrap().fault.unwrap();
                assert_eq!(fault.kind, LiveFaultKind::QueueOverflow);
                assert_eq!(fault.sequence, Some(1));
                assert_eq!(session.shared.gate.load(Ordering::Acquire), CANCELLED);
                assert!(tap.control().snapshot().unwrap().cancelled);
            }
            native::drop_software_renderer(renderer, &controls);
            native::drop_software_error(error, &controls);
            session.shared.native_safe.store(true, Ordering::Release);
            session.shared.native_done.store(true, Ordering::Release);
        }
    }

    #[test]
    fn shared_preparation_cannot_publish_ready_before_the_admission_handoff() {
        let (mut session, ready, _reasons) = shell();
        let (lease, renderer, error, _tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        let handoff = software_ready(lease);
        let scope = handoff.ready.scope;
        session.shared.edit(|s| {
            s.ready = Some(handoff.ready.clone());
            s.live = Some(handoff.ready.progress);
            s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
            s.native_sample_format = Some(handoff.native_sample_format.clone());
            s.phase = PerformancePhase::Recording;
        });
        // Model native-owner preemption between publishing preparation and
        // sending its sole admission lease. No devices or threads are opened.
        let prepared = session.snapshot();
        assert_eq!(prepared.phase, PerformancePhase::Starting);
        assert!(prepared.ready.is_none() && prepared.live.is_none());
        assert!(prepared.identity.descriptor_sha256.is_some());
        assert_eq!(
            session.admit(LiveBatch::empty(scope, 1, None)),
            Err(LiveSubmitError::NotLive)
        );
        assert!(ready.try_send(handoff).is_ok());
        let admitted = session.snapshot();
        assert_eq!(admitted.phase, PerformancePhase::Recording);
        assert!(admitted.ready.is_some() && admitted.live.is_some());
        assert_eq!(admitted.native_sample_format.as_deref(), Some("f32"));
        assert!(admitted.revision > prepared.revision);
        assert!(admitted.is_valid());
        native::drop_software_renderer(renderer, &controls);
        native::drop_software_error(error, &controls);
        session.shared.native_safe.store(true, Ordering::Release);
        session.shared.native_done.store(true, Ordering::Release);
    }

    #[test]
    fn matching_shared_ready_candidate_advances_the_last_published_revision() {
        let (mut session, ready, _reasons) = shell();
        let (lease, renderer, error, _tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        let handoff = software_ready(lease);
        session.shared.edit(|s| {
            s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
            s.native_sample_format = Some(handoff.native_sample_format.clone());
        });
        let before = session.snapshot();
        session.shared.edit(|s| {
            s.ready = Some(handoff.ready.clone());
            s.live = Some(handoff.ready.progress);
            s.phase = PerformancePhase::Recording;
        });
        assert!(ready.try_send(handoff).is_ok());
        session.receive_ready();
        // The previous implementation changed this payload here without a
        // revision, then found an equal shared candidate and published it.
        assert_eq!(session.cached, before);
        let after = session.snapshot();
        assert_ne!(after, before);
        assert!(after.revision > before.revision);
        assert_eq!(after.phase, PerformancePhase::Recording);
        assert!(after.is_valid());
        assert_eq!(session.snapshot(), after);
        native::drop_software_renderer(renderer, &controls);
        native::drop_software_error(error, &controls);
        session.shared.native_safe.store(true, Ordering::Release);
        session.shared.native_done.store(true, Ordering::Release);
    }

    #[test]
    fn held_shared_lock_handoff_via_admit_or_finish_has_complete_metadata_and_new_revision() {
        for via_finish in [false, true] {
            let (mut session, ready, _reasons) = shell();
            let (lease, renderer, error, _tap) = software_pair(&session.shared);
            let controls = lease.controls.clone();
            let handoff = software_ready(lease);
            let scope = handoff.ready.scope;
            let before = session.snapshot();
            assert!(before.native_sample_format.is_none());
            session.shared.edit(|s| {
                s.ready = Some(handoff.ready.clone());
                s.live = Some(handoff.ready.progress);
                s.identity.descriptor_sha256 = Some(handoff.descriptor_sha256.clone());
                s.native_sample_format = Some(handoff.native_sample_format.clone());
                s.phase = PerformancePhase::Recording;
            });
            assert!(ready.try_send(handoff).is_ok());
            let shared = session.shared.clone();
            let held = shared.status.lock().unwrap();
            let after = if via_finish {
                session.begin_finish(scope, LiveId([0x72; 16])).unwrap()
            } else {
                session.admit(LiveBatch::empty(scope, 1, None)).unwrap();
                assert_eq!(session.cached, before);
                session.snapshot()
            };
            assert!(after.revision > before.revision);
            assert!(after.ready.is_some() && after.live.is_some());
            assert_eq!(after.identity.descriptor_sha256, Some("a".repeat(64)));
            assert_eq!(after.native_sample_format.as_deref(), Some("f32"));
            assert!(after.is_valid());
            assert_eq!(session.snapshot(), after);
            drop(held);
            assert_eq!(
                session.snapshot(),
                after,
                "shared state cannot replace newer actual admission progress"
            );
            native::drop_software_renderer(renderer, &controls);
            native::drop_software_error(error, &controls);
            session.shared.native_safe.store(true, Ordering::Release);
            session.shared.native_done.store(true, Ordering::Release);
        }
    }

    #[test]
    fn stop_between_ready_watch_observation_and_clear_keeps_its_exact_deadline() {
        let (mut session, ready, _reasons) = shell();
        let (lease, renderer, error, _tap) = software_pair(&session.shared);
        let controls = lease.controls.clone();
        session.shared.watch(STARTING, START_TIMEOUT);
        let observed_start = session.shared.watch_word.load(Ordering::Acquire);
        assert!(ready.try_send(software_ready(lease)).is_ok());
        // Deterministic interleaving of a successful native Ready enqueue,
        // reliable Stop, and the native owner's delayed START-clear CAS.
        session.cancel("Stop between Ready enqueue and START clear");
        let stop = session.shared.watch_word.load(Ordering::Acquire);
        assert_eq!(stop & WATCH_PHASE_MASK, u64::from(STOPPING));
        assert_ne!(stop, observed_start);
        assert!(
            !session
                .shared
                .clear_observed_watch(observed_start, STARTING)
        );
        assert_eq!(session.shared.watch_word.load(Ordering::Acquire), stop);
        session.shared.watch(STOPPING, Duration::from_secs(60));
        assert_eq!(session.shared.watch_word.load(Ordering::Acquire), stop);
        session.shared.clear_watch_phase(SEALING);
        assert_eq!(session.shared.watch_word.load(Ordering::Acquire), stop);
        assert!(session.shared.expire(stop >> WATCH_SHIFT));
        assert!(session.shared.retire.load(Ordering::Acquire));
        native::drop_software_renderer(renderer, &controls);
        native::drop_software_error(error, &controls);
        session.shared.native_safe.store(true, Ordering::Release);
        session.shared.native_done.store(true, Ordering::Release);
    }
}
