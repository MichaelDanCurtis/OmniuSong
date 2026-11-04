use super::*;
use crate::{
    live_types::{LIVE_VOICES, LiveBatch, LiveEventKind},
    performance_records::{PerformanceCheckpoint, PerformanceConfig},
    performance_tap::{
        BatchDisposition, TapBegin, TapClear, TapFinish, TapTransition, TapTransitionKind,
    },
};
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Default)]
struct Source {
    note: u64,
    voice: u64,
    ticks: Option<u64>,
    sustain: bool,
    gui_ordinal: u64,
}
struct Observation {
    identity: PerformanceVoiceIdentity,
    pitch: u8,
    velocity: u16,
    patch: LivePatch,
    pulse: Option<PerformancePulseShape>,
    onset: u64,
    ticks: u64,
    location: PerformanceRecordLocation,
    key_up: Option<PerformancePhysicalRelease>,
    gate: Option<(u64, PerformanceGateEnd)>,
    ended: Option<PerformanceObservedVoiceEnd>,
    cut: Option<PerformanceHardCutObservation>,
}
#[derive(Clone, Copy)]
struct Voice {
    observation: Option<usize>,
    source: u8,
    id: u64,
    held: bool,
    sustained: bool,
    releasing: bool,
    envelope: f32,
    release_step: f32,
    last_frame: u64,
    auto_due: Option<u64>,
    auto_velocity: u16,
    pending_velocity: u16,
    patch: LivePatch,
}
struct Callback {
    index: u64,
    begin: TapBegin,
    batches: u16,
    applied: u16,
    rejected: bool,
    clears: Vec<(TapClear, PerformanceRecordLocation)>,
    transitions: Vec<(TapTransition, PerformanceRecordLocation)>,
    finish: Option<(TapFinish, PerformanceRecordLocation)>,
}
struct ReleaseObservation<'a> {
    cause: PerformanceGateEndCause,
    ticks: Option<u64>,
    record: &'a PerformanceRecordLocation,
}
fn reserve_slot<T>(rows: &mut Vec<T>, maximum: usize, budget: &InspectionBudget) -> Result<()> {
    if rows.len() == rows.capacity() {
        let capacity = rows.capacity().max(1).saturating_mul(2).min(maximum);
        charge(
            budget,
            (capacity + rows.len())
                .checked_mul(std::mem::size_of::<T>())
                .ok_or_else(|| invalid("Projection callback allocation overflow"))?,
        )?;
        rows.reserve_exact(capacity - rows.len());
    }
    Ok(())
}
pub(super) struct Lifecycle {
    take: String,
    rate: u32,
    channels: u16,
    scope: LiveScope,
    frame: u64,
    next_callback: u64,
    sources: [Source; 4],
    voices: [Option<Voice>; LIVE_VOICES],
    observations: Vec<Observation>,
    // A pre-region held gate may eventually cross into the view. Its details
    // occupy only its actual live slot until that fact becomes known.
    provisional: [Option<Observation>; LIVE_VOICES],
    indices: BTreeMap<(u64, u8, u64), usize>,
    callback: Option<Callback>,
    region: PerformanceFrameRegion,
    selected_sources: [bool; 4],
    unselected: Sha256,
    counts: PerformanceProjectionCounts,
}
impl Lifecycle {
    pub(super) fn new(
        config: &PerformanceConfig,
        request: &PerformanceProjectionRequest,
        budget: &InspectionBudget,
    ) -> Result<Self> {
        require(
            config.sources.iter().enumerate().all(|(i, s)| {
                s.binding.handle == i as u8 + 1 && s.binding.connection_generation == 1
            }),
            "Projection source bindings differ from the renderer's prepared ownership",
        )?;
        let reservation = (MAX_PROJECTION_OBSERVATIONS + LIVE_VOICES)
            .checked_mul(std::mem::size_of::<Observation>() + config.take_id.0.len() + 128)
            .ok_or_else(|| invalid("Projection observation reservation overflow"))?;
        charge(budget, reservation)?;
        let mut selected_sources = [false; 4];
        for source in &request.sources {
            selected_sources[usize::from(*source - 1)] = true;
        }
        charge(budget, 128)?;
        let mut unselected = Sha256::new();
        unselected.update(b"OmniuSong.performance-projection-unselected.v1\0");
        Ok(Self {
            take: config.take_id.0.clone(),
            rate: config.info.sample_rate,
            channels: config.info.channels,
            scope: config.info.scope,
            frame: 0,
            next_callback: 1,
            sources: [Source::default(); 4],
            voices: [None; LIVE_VOICES],
            provisional: std::array::from_fn(|_| None),
            observations: Vec::with_capacity(MAX_PROJECTION_OBSERVATIONS),
            indices: BTreeMap::new(),
            callback: None,
            region: request.region,
            selected_sources,
            unselected,
            counts: Default::default(),
        })
    }
    fn retain(&mut self, observation: Observation, budget: &InspectionBudget) -> Result<usize> {
        require(
            self.observations.len() < MAX_PROJECTION_OBSERVATIONS,
            "Performance projection exceeds 20,000 selected observations",
        )?;
        charge(budget, 128)?;
        let index = self.observations.len();
        require(
            self.indices
                .insert(
                    (
                        observation.identity.scope.panic_generation,
                        observation.identity.source,
                        observation.identity.voice,
                    ),
                    index,
                )
                .is_none(),
            "Voice identity was reused within its full scope",
        )?;
        self.observations.push(observation);
        Ok(index)
    }
    fn unselected(
        &mut self,
        observation: Observation,
        reason: PerformanceProjectionOmissionReason,
        budget: &InspectionBudget,
    ) -> Result<()> {
        match reason {
            PerformanceProjectionOmissionReason::ExcludedSource => self.counts.excluded_source += 1,
            PerformanceProjectionOmissionReason::OutsideRegion => self.counts.outside_region += 1,
            _ => {
                return Err(invalid(
                    "Selected omission was routed into exclusion accounting",
                ));
            }
        }
        let compact = (
            &observation.identity,
            observation.onset,
            observation.gate.as_ref().map(|g| g.0),
            reason,
            &observation.location,
        );
        let row = digest(
            b"OmniuSong.performance-projection-unselected-row.v1\0",
            &compact,
            4096,
            budget,
        )?;
        charge(budget, row.len())?;
        self.unselected.update(row.as_bytes());
        Ok(())
    }
    fn observation_mut(&mut self, slot: usize) -> Option<&mut Observation> {
        match self.voices[slot].and_then(|v| v.observation) {
            Some(index) => self.observations.get_mut(index),
            None => self.provisional[slot].as_mut(),
        }
    }
    fn close_gate(
        &mut self,
        slot: usize,
        frame: u64,
        ending: PerformanceGateEnd,
        budget: &InspectionBudget,
    ) -> Result<()> {
        if let Some(observation) = self.observation_mut(slot) {
            require(
                observation.gate.is_none(),
                "A voice gate was already closed",
            )?;
            observation.gate = Some((frame, ending));
        }
        if let Some(observation) = self.provisional[slot].take() {
            if frame <= self.region.start {
                self.unselected(
                    observation,
                    PerformanceProjectionOmissionReason::OutsideRegion,
                    budget,
                )?;
            } else {
                let index = self.retain(observation, budget)?;
                self.voices[slot]
                    .as_mut()
                    .expect("live provisional voice")
                    .observation = Some(index);
            }
        }
        Ok(())
    }
    fn advance(&mut self, target: u64, budget: &InspectionBudget) -> Result<()> {
        for voice in self.voices.iter_mut().flatten() {
            require(
                target >= voice.last_frame,
                "Projection voice frame regressed",
            )?;
            require(
                voice.releasing || voice.auto_due.is_none_or(|due| due >= target),
                "Missing observed AutoRelease before the next lifecycle boundary",
            )?;
            let elapsed = target - voice.last_frame;
            if voice.releasing {
                let steps = elapsed.min(1_000_000);
                budget.charge_work(
                    steps
                        .checked_mul(8)
                        .ok_or_else(|| invalid("Envelope work overflow"))?,
                )?;
                for _ in 0..steps {
                    voice.envelope -= voice.release_step;
                    require(
                        voice.envelope > 0.0,
                        "Missing or late observed VoiceEnded transition",
                    )?;
                }
                require(
                    steps == elapsed,
                    "Release envelope exceeds its bounded renderer duration",
                )?;
            } else if voice.envelope < 1.0 {
                let steps = elapsed.min(32768);
                budget.charge_work(steps * 8)?;
                let attack = if voice.patch == LivePatch::Pad {
                    0.04f32
                } else {
                    0.003f32
                };
                let step = 1.0f32 / (attack * self.rate as f32);
                for _ in 0..steps {
                    voice.envelope = (voice.envelope + step).min(1.0);
                    if voice.envelope == 1.0 {
                        break;
                    }
                }
                require(
                    steps == elapsed || voice.envelope == 1.0,
                    "Attack envelope failed to reach its steady bound",
                )?;
            }
            // A saturated held/sustained span costs no per-frame rerender.
            voice.last_frame = target;
        }
        Ok(())
    }
    fn release(
        &mut self,
        slot: usize,
        frame: u64,
        velocity: u16,
        observation: ReleaseObservation<'_>,
        budget: &InspectionBudget,
    ) -> Result<()> {
        charge(budget, 512)?;
        let voice = self.voices[slot]
            .as_mut()
            .ok_or_else(|| invalid("Release refers to an inactive voice"))?;
        require(
            !voice.releasing,
            "A release cannot restart an existing envelope",
        )?;
        voice.held = false;
        voice.sustained = false;
        voice.releasing = true;
        let seconds = if voice.patch == LivePatch::Pad {
            0.30f32
        } else {
            0.035f32 + 0.12f32 * (1.0 - f32::from(velocity) / 65535.0)
        };
        voice.release_step = voice.envelope.max(0.001) / (seconds * self.rate as f32).max(1.0);
        require(
            voice.release_step.is_finite() && voice.release_step > 0.0,
            "Invalid projected release envelope",
        )?;
        self.close_gate(
            slot,
            frame,
            PerformanceGateEnd {
                cause: observation.cause,
                release_velocity16: Some(velocity),
                observed_ticks: observation.ticks,
                record: observation.record.clone(),
            },
            budget,
        )
    }
    fn batch(
        &mut self,
        batch: &LiveBatch,
        frame: u64,
        ordinal: u16,
        location: &PerformanceRecordLocation,
        budget: &InspectionBudget,
    ) -> Result<()> {
        batch.validate_shape(self.rate).map_err(invalid)?;
        require(
            batch.scope == self.scope,
            "Applied batch belongs to a different voice scope",
        )?;
        for (event_index, event) in batch.events[..usize::from(batch.len)].iter().enumerate() {
            budget.visit_records(1)?;
            charge(budget, 1024)?;
            let event = event.ok_or_else(|| invalid("Missing applied event"))?;
            // Never index a discarded/rejected packet, or an unchecked source.
            require(
                (1..=4).contains(&event.source) && event.connection_generation == 1,
                "Applied event has unsupported source ownership",
            )?;
            let si = usize::from(event.source - 1);
            require(
                self.sources[si]
                    .ticks
                    .is_none_or(|old| event.observed_ticks >= old),
                "Applied GUI ticks regressed",
            )?;
            self.sources[si].ticks = Some(event.observed_ticks);
            let mut at = location.clone();
            at.batch_ordinal = Some(ordinal);
            at.event_index = Some(event_index as u8);
            at.applied_source_ordinal = Some(self.sources[si].gui_ordinal);
            self.sources[si].gui_ordinal = self.sources[si]
                .gui_ordinal
                .checked_add(1)
                .ok_or_else(|| invalid("Applied GUI ordinal overflow"))?;
            self.counts.applied_events += 1;
            match event.event {
                LiveEventKind::NoteOn {
                    note,
                    voice,
                    key,
                    velocity16,
                    patch,
                }
                | LiveEventKind::Pulse {
                    note,
                    voice,
                    key,
                    velocity16,
                    patch,
                    ..
                } => {
                    require(
                        self.sources[si].note.checked_add(1) == Some(note)
                            && self.sources[si].voice.checked_add(1) == Some(voice),
                        "Applied note/voice counters do not follow renderer ownership",
                    )?;
                    let slot = self
                        .voices
                        .iter()
                        .position(Option::is_none)
                        .ok_or_else(|| invalid("Applied note exceeds actual renderer polyphony"))?;
                    let pulse = match event.event {
                        LiveEventKind::Pulse {
                            duration_frames,
                            release_velocity16,
                            ..
                        } => Some(PerformancePulseShape {
                            duration_frames,
                            release_velocity16,
                        }),
                        _ => None,
                    };
                    let duration = pulse.map(|p| u64::from(p.duration_frames)).or_else(|| {
                        (patch == LivePatch::Drums).then_some(u64::from(self.rate) / 5)
                    });
                    let auto_due = duration
                        .map(|d| {
                            frame
                                .checked_add(d)
                                .ok_or_else(|| invalid("Automatic release frame overflow"))
                        })
                        .transpose()?;
                    charge(budget, self.take.len() + 512)?;
                    let identity = PerformanceVoiceIdentity {
                        take_id: self.take.clone(),
                        scope: self.scope,
                        source: event.source,
                        connection_generation: event.connection_generation,
                        note,
                        voice,
                    };
                    let observation = Observation {
                        identity,
                        pitch: key,
                        velocity: velocity16,
                        patch,
                        pulse,
                        onset: frame,
                        ticks: event.observed_ticks,
                        location: at,
                        key_up: None,
                        gate: None,
                        ended: None,
                        cut: None,
                    };
                    self.counts.observations += 1;
                    let retained = if !self.selected_sources[si] {
                        self.unselected(
                            observation,
                            PerformanceProjectionOmissionReason::ExcludedSource,
                            budget,
                        )?;
                        None
                    } else if frame >= self.region.end {
                        self.unselected(
                            observation,
                            PerformanceProjectionOmissionReason::OutsideRegion,
                            budget,
                        )?;
                        None
                    } else if frame < self.region.start {
                        self.provisional[slot] = Some(observation);
                        None
                    } else {
                        Some(self.retain(observation, budget)?)
                    };
                    self.voices[slot] = Some(Voice {
                        observation: retained,
                        source: event.source,
                        id: voice,
                        held: true,
                        sustained: false,
                        releasing: false,
                        envelope: 0.0,
                        release_step: 0.0,
                        last_frame: frame,
                        auto_due,
                        auto_velocity: pulse.map_or(0, |p| p.release_velocity16),
                        pending_velocity: 0,
                        patch,
                    });
                    self.sources[si].note = note;
                    self.sources[si].voice = voice;
                }
                LiveEventKind::NoteOff {
                    voice,
                    release_velocity16,
                } => {
                    require(
                        voice <= self.sources[si].voice,
                        "Applied NoteOff names a future voice",
                    )?;
                    if let Some(slot) = self
                        .voices
                        .iter()
                        .position(|v| v.is_some_and(|v| v.source == event.source && v.id == voice))
                    {
                        if let Some(observation) = self.observation_mut(slot)
                            && observation.key_up.is_none()
                        {
                            observation.key_up = Some(PerformancePhysicalRelease {
                                frame,
                                release_velocity16,
                                observed_ticks: event.observed_ticks,
                                record: at.clone(),
                            });
                        }
                        let active = self.voices[slot].as_mut().expect("active slot");
                        if active.held {
                            active.held = false;
                            if self.sources[si].sustain && active.auto_due.is_none() {
                                active.sustained = true;
                                active.pending_velocity = release_velocity16;
                            } else {
                                self.release(
                                    slot,
                                    frame,
                                    release_velocity16,
                                    ReleaseObservation {
                                        cause: PerformanceGateEndCause::NoteOff {},
                                        ticks: Some(event.observed_ticks),
                                        record: &at,
                                    },
                                    budget,
                                )?;
                            }
                        } else {
                            self.counts.repeated_note_offs += 1;
                        }
                    } else {
                        if let Some(&index) =
                            self.indices
                                .get(&(self.scope.panic_generation, event.source, voice))
                        {
                            let observation = &mut self.observations[index];
                            if observation.key_up.is_none() {
                                observation.key_up = Some(PerformancePhysicalRelease {
                                    frame,
                                    release_velocity16,
                                    observed_ticks: event.observed_ticks,
                                    record: at.clone(),
                                });
                            }
                        }
                        self.counts.late_note_offs += 1;
                    }
                }
                LiveEventKind::Sustain { down } => {
                    self.counts.sustain_events += 1;
                    self.sources[si].sustain = down;
                    if !down {
                        for slot in 0..self.voices.len() {
                            if let Some(voice) = self.voices[slot]
                                && voice.source == event.source
                                && voice.sustained
                            {
                                self.release(
                                    slot,
                                    frame,
                                    voice.pending_velocity,
                                    ReleaseObservation {
                                        cause: PerformanceGateEndCause::SustainUp {},
                                        ticks: Some(event.observed_ticks),
                                        record: &at,
                                    },
                                    budget,
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn cut(
        &mut self,
        frame: u64,
        cause: PerformanceGateEndCause,
        location: &PerformanceRecordLocation,
        budget: &InspectionBudget,
    ) -> Result<()> {
        self.advance(frame, budget)?;
        for slot in 0..self.voices.len() {
            if self.voices[slot].is_some() {
                charge(budget, 1024)?;
                let needs_gate = self.observation_mut(slot).is_some_and(|o| o.gate.is_none());
                if needs_gate {
                    self.close_gate(
                        slot,
                        frame,
                        PerformanceGateEnd {
                            cause: cause.clone(),
                            release_velocity16: None,
                            observed_ticks: None,
                            record: location.clone(),
                        },
                        budget,
                    )?;
                }
                if let Some(observation) = self.observation_mut(slot) {
                    observation.cut = Some(PerformanceHardCutObservation {
                        frame,
                        cause: cause.clone(),
                        record: location.clone(),
                    });
                }
                self.voices[slot] = None;
                self.counts.hard_cuts += 1;
            }
        }
        for source in &mut self.sources {
            source.sustain = false;
        }
        Ok(())
    }
    fn transition(
        &mut self,
        transition: TapTransition,
        location: &PerformanceRecordLocation,
        last: &mut Option<(u64, usize)>,
        budget: &InspectionBudget,
    ) -> Result<()> {
        require(
            transition.scope == self.scope && (1..=4).contains(&transition.source),
            "Voice transition has another source scope",
        )?;
        self.advance(transition.frame, budget)?;
        let slot = self
            .voices
            .iter()
            .position(|v| {
                v.is_some_and(|v| v.source == transition.source && v.id == transition.voice)
            })
            .ok_or_else(|| invalid("Transition refers to an unobserved or inactive voice"))?;
        if let Some((frame, previous_slot)) = *last {
            require(
                transition.frame > frame || slot >= previous_slot,
                "Same-frame voice transitions contradict renderer slot order",
            )?;
        }
        *last = Some((transition.frame, slot));
        let voice = self.voices[slot].expect("active slot");
        match transition.kind {
            TapTransitionKind::AutoRelease { release_velocity16 } => {
                require(
                    !voice.releasing
                        && voice.auto_due == Some(transition.frame)
                        && voice.auto_velocity == release_velocity16,
                    "Observed AutoRelease differs from its active schedule or was suppressed by an earlier key release",
                )?;
                self.release(
                    slot,
                    transition.frame,
                    release_velocity16,
                    ReleaseObservation {
                        cause: PerformanceGateEndCause::AutoRelease {},
                        ticks: None,
                        record: location,
                    },
                    budget,
                )?;
            }
            TapTransitionKind::VoiceEnded => {
                charge(budget, 512)?;
                require(
                    voice.releasing && voice.envelope - voice.release_step <= 0.0,
                    "VoiceEnded contradicts its actual release envelope",
                )?;
                if let Some(observation) = self.observation_mut(slot) {
                    require(
                        observation.gate.is_some() && observation.ended.is_none(),
                        "VoiceEnded has no unique preceding gate release",
                    )?;
                    observation.ended = Some(PerformanceObservedVoiceEnd {
                        frame: transition.frame,
                        record: location.clone(),
                    });
                }
                self.voices[slot] = None;
            }
        }
        Ok(())
    }
    pub(super) fn record(
        &mut self,
        record: PerformanceRecord,
        mut location: PerformanceRecordLocation,
        budget: &InspectionBudget,
    ) -> Result<()> {
        match record {
            PerformanceRecord::Begin {
                callback_index,
                begin,
            } => {
                require(
                    self.callback.is_none()
                        && callback_index == self.next_callback
                        && begin.scope == self.scope
                        && begin.start_frame == self.frame
                        && begin.sample_rate == self.rate
                        && begin.channels == self.channels,
                    "Projection callback identity/clock regressed",
                )?;
                self.advance(begin.start_frame, budget)?;
                charge(budget, 128)?;
                self.callback = Some(Callback {
                    index: callback_index,
                    begin,
                    batches: 0,
                    applied: 0,
                    rejected: false,
                    clears: Vec::new(),
                    transitions: Vec::new(),
                    finish: None,
                });
            }
            PerformanceRecord::Batch {
                callback_index,
                packet_ordinal,
                batch,
                disposition,
            } => {
                let callback = self
                    .callback
                    .as_mut()
                    .ok_or_else(|| invalid("Batch outside a projection callback"))?;
                require(
                    callback.index == callback_index
                        && callback.batches < 4
                        && packet_ordinal == callback.batches + 1,
                    "Projection batch ordinal differs from its callback",
                )?;
                callback.batches += 1;
                location.callback_index = callback_index;
                match disposition {
                    BatchDisposition::Applied => {
                        require(
                            !callback.rejected,
                            "A callback cannot apply events after a rejected batch",
                        )?;
                        callback.applied += 1;
                        let frame = callback.begin.start_frame;
                        self.batch(&batch, frame, packet_ordinal, &location, budget)?;
                    }
                    BatchDisposition::Rejected { .. } => {
                        callback.rejected = true;
                        self.counts.rejected_batches += 1;
                    }
                    BatchDisposition::DiscardedPanic | BatchDisposition::DiscardedStale => {
                        self.counts.discarded_batches += 1
                    }
                }
            }
            PerformanceRecord::Clear {
                callback_index,
                clear,
            } => {
                let cb = self
                    .callback
                    .as_mut()
                    .ok_or_else(|| invalid("Clear outside callback"))?;
                require(
                    cb.index == callback_index && cb.clears.len() < 2,
                    "Invalid projection clear count",
                )?;
                reserve_slot(&mut cb.clears, 2, budget)?;
                location.callback_index = callback_index;
                cb.clears.push((clear, location));
            }
            PerformanceRecord::Transition {
                callback_index,
                transition,
            } => {
                let cb = self
                    .callback
                    .as_mut()
                    .ok_or_else(|| invalid("Transition outside callback"))?;
                require(
                    cb.index == callback_index && cb.transitions.len() < 128,
                    "Invalid projection transition count",
                )?;
                reserve_slot(&mut cb.transitions, 128, budget)?;
                location.callback_index = callback_index;
                cb.transitions.push((transition, location));
            }
            PerformanceRecord::Finish {
                callback_index,
                finish,
            } => {
                let cb = self
                    .callback
                    .as_mut()
                    .ok_or_else(|| invalid("Finish outside callback"))?;
                require(
                    cb.index == callback_index && cb.finish.is_none(),
                    "Duplicate projection Finish",
                )?;
                location.callback_index = callback_index;
                cb.finish = Some((finish, location));
            }
            PerformanceRecord::End {
                callback_index,
                end_frame,
                scope_after,
                active_voices,
                release_tail_voices,
                clear_count,
                transition_count,
                ..
            } => {
                let cb = self
                    .callback
                    .take()
                    .ok_or_else(|| invalid("End outside callback"))?;
                require(
                    cb.index == callback_index
                        && cb.clears.len() == usize::from(clear_count)
                        && cb.transitions.len() == usize::from(transition_count)
                        && end_frame == cb.begin.start_frame + u64::from(cb.begin.frames),
                    "Projection callback End count/clock mismatch",
                )?;
                require(
                    cb.clears
                        .windows(2)
                        .all(|pair| pair[0].0.frame <= pair[1].0.frame),
                    "Projection clears are out of chronological order",
                )?;
                let panic = cb
                    .clears
                    .iter()
                    .any(|(clear, _)| matches!(clear.reason, TapClearReason::Panic { .. }));
                require(
                    !panic || cb.applied == 0,
                    "Panic callback cannot apply queued events before clearing",
                )?;
                let mut clears = cb.clears.iter().peekable();
                let mut last = None;
                for (transition, at) in &cb.transitions {
                    while clears
                        .peek()
                        .is_some_and(|(clear, _)| clear.frame <= transition.frame)
                    {
                        let (clear, at) = clears.next().expect("peeked clear");
                        self.clear(*clear, at, cb.begin.start_frame, scope_after, budget)?;
                    }
                    self.transition(*transition, at, &mut last, budget)?;
                }
                for (clear, at) in clears {
                    self.clear(*clear, at, cb.begin.start_frame, scope_after, budget)?;
                }
                self.advance(end_frame, budget)?;
                if let Some((finish, at)) = cb.finish {
                    let active = self.voices.iter().flatten().count();
                    let tails = self.voices.iter().flatten().filter(|v| v.releasing).count();
                    require(
                        finish.cut_frame == end_frame
                            && usize::from(finish.voices_cut) == active
                            && usize::from(finish.release_tails_cut) == tails,
                        "Finish cut counts differ from authenticated voice lifecycle",
                    )?;
                    self.cut(end_frame, PerformanceGateEndCause::Finish {}, &at, budget)?;
                }
                require(
                    self.voices.iter().flatten().count() == usize::from(active_voices)
                        && self.voices.iter().flatten().filter(|v| v.releasing).count()
                            == usize::from(release_tail_voices),
                    "End active/release-tail counts differ from actual note lifecycle",
                )?;
                if scope_after != self.scope {
                    require(
                        panic
                            && self.scope.panic_generation.checked_add(1)
                                == Some(scope_after.panic_generation)
                            && scope_after.worker_generation == self.scope.worker_generation
                            && scope_after.session_id == self.scope.session_id,
                        "Voice scope advanced without the exact Panic acknowledgement",
                    )?;
                    for source in &mut self.sources {
                        source.note = 0;
                        source.voice = 0;
                        source.sustain = false;
                    }
                }
                self.scope = scope_after;
                self.frame = end_frame;
                self.next_callback = self
                    .next_callback
                    .checked_add(1)
                    .ok_or_else(|| invalid("Projection callback counter overflow"))?;
            }
            PerformanceRecord::AppliedEvent { .. } | PerformanceRecord::Checkpoint { .. } => {
                return Err(invalid(
                    "Unexpected GUI/checkpoint record in control callback projection",
                ));
            }
        }
        Ok(())
    }
    fn clear(
        &mut self,
        clear: TapClear,
        at: &PerformanceRecordLocation,
        start: u64,
        scope_after: LiveScope,
        budget: &InspectionBudget,
    ) -> Result<()> {
        require(
            clear.scope == self.scope || clear.scope == scope_after,
            "Clear belongs to another voice scope",
        )?;
        if let TapClearReason::Panic { next_generation } = clear.reason {
            require(
                clear.scope == self.scope
                    && clear.frame == start
                    && self.scope.panic_generation.checked_add(1) == Some(next_generation),
                "Panic clear has another generation or frame",
            )?;
        }
        self.cut(
            clear.frame,
            PerformanceGateEndCause::Clear {
                reason: clear.reason,
            },
            at,
            budget,
        )
    }
    pub(super) fn checkpoint(&self, checkpoint: &PerformanceCheckpoint) -> Result<()> {
        require(
            self.callback.is_none()
                && checkpoint.audio_end == self.frame
                && checkpoint.scope_after == self.scope
                && checkpoint.last_callback_index.checked_add(1) == Some(self.next_callback),
            "Projected control lifecycle differs from its selected checkpoint",
        )
    }
    pub(super) fn finish(
        mut self,
        request: &PerformanceProjectionRequest,
        profile: u16,
        clock: &LiveClock,
        checkpoint: &PerformanceCheckpoint,
        unknown_tail: bool,
        budget: &InspectionBudget,
    ) -> Result<PerformanceProjectionReview> {
        self.checkpoint(checkpoint)?;
        for slot in 0..self.provisional.len() {
            if let Some(observation) = self.provisional[slot].take() {
                self.retain(observation, budget)?;
            }
        }
        // Provisional pre-region gates become selected only when they cross the
        // boundary. Restore original onset/event ordering after promotion.
        charge(
            budget,
            self.observations
                .len()
                .checked_mul(16 * 64)
                .ok_or_else(|| invalid("Projection ordering work overflow"))?,
        )?;
        self.observations.sort_unstable_by_key(|o| {
            (
                o.onset,
                o.location.stream_event_ordinal,
                o.location.event_index,
            )
        });
        let tempo = arithmetic::Tempo::new(request.tempo_bits)?;
        let n = self.observations.len();
        charge(
            budget,
            n.checked_mul(
                std::mem::size_of::<ProjectedPerformanceNote>()
                    + std::mem::size_of::<PerformanceProjectionOmission>(),
            )
            .ok_or_else(|| invalid("Projection output reservation overflow"))?,
        )?;
        let mut notes = Vec::with_capacity(n);
        let mut omissions = Vec::with_capacity(n);
        for observation in self.observations {
            budget.checkpoint()?;
            charge(budget, 4096)?;
            let gate_frame = observation.gate.as_ref().map(|(frame, _)| *frame);
            let reason = if gate_frame.is_none() {
                require(
                    request.incomplete == PerformanceProjectionPolicy::OmitWithDisclosure,
                    "Selected note has no actual observed gate ending",
                )?;
                if observation.onset < request.region.start {
                    require(
                        request.region_crossing == PerformanceProjectionPolicy::OmitWithDisclosure,
                        "Incomplete selected note crosses the region boundary",
                    )?;
                }
                Some(PerformanceProjectionOmissionReason::Incomplete)
            } else if observation.onset < request.region.start
                || gate_frame.is_some_and(|end| end > request.region.end)
            {
                require(
                    request.region_crossing == PerformanceProjectionPolicy::OmitWithDisclosure,
                    "Selected note crosses the requested region; projection never trims it",
                )?;
                Some(PerformanceProjectionOmissionReason::RegionCrossing)
            } else {
                None
            };
            let identity_sha256 = digest(
                b"OmniuSong.performance-projected-note.v1\0",
                &observation.identity,
                4096,
                budget,
            )?;
            let mut values = None;
            let reason = if reason.is_none() {
                let end = gate_frame.expect("complete selected gate");
                require(
                    end >= observation.onset,
                    "Observed note gate ends before its onset",
                )?;
                let start_tick = request
                    .destination_start_tick
                    .checked_add(tempo.ticks(observation.onset - request.region.start, self.rate)?)
                    .ok_or_else(|| invalid("Projection start tick overflow"))?;
                let end_tick = request
                    .destination_start_tick
                    .checked_add(tempo.ticks(end - request.region.start, self.rate)?)
                    .ok_or_else(|| invalid("Projection end tick overflow"))?;
                if end_tick == start_tick {
                    require(
                        request.collapsed == PerformanceProjectionPolicy::OmitWithDisclosure,
                        "Selected note collapses to zero duration at 960 PPQ",
                    )?;
                    Some(PerformanceProjectionOmissionReason::CollapsedTicks)
                } else {
                    let note = NoteValues {
                        pitch: observation.pitch,
                        start_tick,
                        duration_ticks: end_tick
                            .checked_sub(start_tick)
                            .ok_or_else(|| invalid("Projection note duration underflow"))?,
                        velocity16: observation.velocity,
                    };
                    note.validate().map_err(invalid)?;
                    values = Some(note);
                    None
                }
            } else {
                reason
            };
            if let Some(reason) = reason {
                match reason {
                    PerformanceProjectionOmissionReason::ExcludedSource => {
                        self.counts.excluded_source += 1
                    }
                    PerformanceProjectionOmissionReason::OutsideRegion => {
                        self.counts.outside_region += 1
                    }
                    PerformanceProjectionOmissionReason::RegionCrossing => {
                        self.counts.region_crossing += 1
                    }
                    PerformanceProjectionOmissionReason::Incomplete => self.counts.incomplete += 1,
                    PerformanceProjectionOmissionReason::CollapsedTicks => {
                        self.counts.collapsed += 1
                    }
                }
                omissions.push(PerformanceProjectionOmission {
                    identity: observation.identity,
                    identity_sha256,
                    onset_frame: observation.onset,
                    gate_end_frame: gate_frame,
                    reason,
                    onset_record: observation.location,
                    physical_key_up: observation.key_up,
                    gate_end: observation.gate.map(|(_, ending)| ending),
                    voice_ended: observation.ended,
                    hard_cut: observation.cut,
                });
            } else {
                let (gate_end_frame, gate_end) = observation.gate.expect("complete selected gate");
                notes.push(ProjectedPerformanceNote {
                    identity: observation.identity,
                    identity_sha256,
                    values: values.expect("positive note values"),
                    onset_frame: observation.onset,
                    gate_end_frame,
                    onset_gui_ticks: observation.ticks,
                    patch: observation.patch,
                    pulse: observation.pulse,
                    onset_record: observation.location,
                    physical_key_up: observation.key_up,
                    gate_end,
                    voice_ended: observation.ended,
                    hard_cut: observation.cut,
                });
            }
        }
        self.counts.notes = notes.len() as u32;
        charge(budget, 4096)?;
        let mut disclosures = vec![
            PerformanceProjectionDisclosure::ProceduralDerivationNotAi,
            PerformanceProjectionDisclosure::NearestTiesUp960Ppq,
            PerformanceProjectionDisclosure::ReleaseGateNotEnvelope,
            PerformanceProjectionDisclosure::SustainFlattenedToGate,
            PerformanceProjectionDisclosure::PatchAndPulseNotEncodedInNoteValues,
            PerformanceProjectionDisclosure::GuiTicksAreUncalibratedObservations,
            PerformanceProjectionDisclosure::SelectedSourcesAndRegionOnly,
        ];
        if unknown_tail {
            disclosures.push(PerformanceProjectionDisclosure::UnknownTailExcluded);
        }
        if notes.iter().any(|n| n.hard_cut.is_some())
            || omissions.iter().any(|n| n.hard_cut.is_some())
        {
            disclosures.push(PerformanceProjectionDisclosure::ObservedHardCutsRetained);
        }
        let unselected_observations_sha256 = format!("{:x}", self.unselected.finalize());
        let mut review = PerformanceProjectionReview {
            version: 1,
            request: request.clone(),
            profile_version: profile,
            sample_rate: self.rate,
            gui_clock: clock.clone(),
            notes,
            omissions,
            unselected_observations_sha256,
            counts: self.counts,
            disclosures,
            review_sha256: String::new(),
        };
        // Include the eventual 64 hex characters in the complete review bound.
        review.review_sha256 = digest(
            b"OmniuSong.performance-projection-review.v1\0",
            &review,
            MAX_PROJECTION_REVIEW_BYTES - 64,
            budget,
        )?;
        budget.checkpoint()?;
        Ok(review)
    }
}
