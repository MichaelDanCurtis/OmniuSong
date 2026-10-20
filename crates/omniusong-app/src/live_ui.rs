//! Output-only live input. No project, device, process or filesystem work occurs here.
use crate::audio_ui::EnginePort;
use eframe::egui;
use omniusong_engine::*;
use std::time::{Duration, Instant};

const KEYS: [egui::Key; 13] = [
    egui::Key::A,
    egui::Key::W,
    egui::Key::S,
    egui::Key::E,
    egui::Key::D,
    egui::Key::F,
    egui::Key::T,
    egui::Key::G,
    egui::Key::Y,
    egui::Key::H,
    egui::Key::U,
    egui::Key::J,
    egui::Key::K,
];
const KEY_NAMES: [&str; 13] = [
    "A", "W", "S", "E", "D", "F", "T", "G", "Y", "H", "U", "J", "K",
];
const NOTE_NAMES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];
const PAD_KEYS: [egui::Key; 16] = [
    egui::Key::Num1,
    egui::Key::Num2,
    egui::Key::Num3,
    egui::Key::Num4,
    egui::Key::Q,
    egui::Key::W,
    egui::Key::E,
    egui::Key::R,
    egui::Key::A,
    egui::Key::S,
    egui::Key::D,
    egui::Key::F,
    egui::Key::Z,
    egui::Key::X,
    egui::Key::C,
    egui::Key::V,
];
const PAD_NAMES: [&str; 16] = [
    "Kick",
    "Snare",
    "Closed hat",
    "Open hat",
    "Clap",
    "Low tom",
    "Mid tom",
    "High tom",
    "Crash",
    "Ride",
    "Rim",
    "Cowbell",
    "Shaker",
    "Tambourine",
    "Claves",
    "Percussion",
];
const PAD_LABELS: [&str; 16] = [
    "1", "2", "3", "4", "Q", "W", "E", "R", "A", "S", "D", "F", "Z", "X", "C", "V",
];
const PAD_NOTES: [u8; 16] = [
    36, 38, 42, 46, 39, 45, 47, 50, 49, 51, 37, 56, 70, 54, 75, 67,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Keys,
    Pads,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Computer(egui::Key),
    PointerKey(u8),
    PointerPad(u8),
}
#[derive(Clone, Copy)]
struct Held {
    origin: Origin,
    source: LiveSourceBinding,
    voice: u64,
    key: u8,
    patch: LivePatch,
}

pub(super) struct LiveUi {
    ready: Option<LiveReady>,
    clock: Option<LiveClock>,
    epoch: Instant,
    starting: bool,
    cancelled_start: bool,
    stop: Option<UrgentTicket>,
    panic: Option<(UrgentTicket, LiveScope)>,
    worker: Option<LiveId>,
    update_revision: u64,
    latest_update: Option<EngineUpdate>,
    next_sequence: u64,
    parent_sequence: Option<u64>,
    credit: Option<LeaseCredit>,
    credit_issue: Option<Option<u64>>,
    next_handle: [u64; LIVE_MAX_SOURCES],
    held: Vec<Held>,
    events: [Option<LiveEvent>; LIVE_BATCH_EVENTS],
    event_count: usize,
    keyboard_armed: bool,
    mode: Mode,
    octave: i32,
    patch: LivePatch,
    velocity: u16,
    release_velocity: u16,
    sustain_latch: bool,
    sustain_key: bool,
    sustain_effective: [bool; 2],
    pointer_latched: bool,
    focus_ids: Vec<egui::Id>,
    last_lease: Instant,
    pub(super) error: Option<String>,
}

impl LiveUi {
    pub(super) fn new() -> Self {
        Self {
            ready: None,
            clock: None,
            epoch: Instant::now(),
            starting: false,
            cancelled_start: false,
            stop: None,
            panic: None,
            worker: None,
            update_revision: 0,
            latest_update: None,
            next_sequence: 1,
            parent_sequence: None,
            credit: None,
            credit_issue: None,
            next_handle: [1; LIVE_MAX_SOURCES],
            held: Vec::with_capacity(LIVE_VOICES),
            events: [None; LIVE_BATCH_EVENTS],
            event_count: 0,
            keyboard_armed: false,
            mode: Mode::Keys,
            octave: 4,
            patch: LivePatch::Keys,
            velocity: 49152,
            release_velocity: 32768,
            sustain_latch: false,
            sustain_key: false,
            sustain_effective: [false; 2],
            pointer_latched: false,
            focus_ids: Vec::with_capacity(31),
            last_lease: Instant::now(),
            error: None,
        }
    }
    pub(super) fn begin(&mut self) -> LiveClock {
        self.clear_local();
        self.epoch = Instant::now();
        let clock = LiveClock {
            domain: "omniusong.gui-monotonic".into(),
            epoch: uuid::Uuid::new_v4().to_string(),
            ticks_per_second_numerator: 1_000_000_000,
            ticks_per_second_denominator: 1,
        };
        self.clock = Some(clock.clone());
        self.starting = true;
        self.cancelled_start = false;
        self.error = None;
        clock
    }
    pub(super) fn start_failed(&mut self) {
        self.starting = false;
    }
    pub(super) fn cancelled_start(&self) -> bool {
        self.cancelled_start
    }
    pub(super) fn engaged(&self) -> bool {
        self.starting || self.ready.is_some() || self.stop.is_some()
    }
    pub(super) fn running(&self) -> bool {
        self.ready
            .as_ref()
            .is_some_and(|r| r.progress.state == LiveState::Running)
            && self.stop.is_none()
            && self.panic.is_none()
            && self.error.is_none()
    }
    pub(super) fn activate(&mut self, ready: LiveReady) -> Result<bool, String> {
        if self.cancelled_start {
            self.starting = false;
            return Ok(false);
        }
        if !self.starting
            || !ready.is_valid()
            || self.clock.as_ref() != Some(&ready.gui_clock)
            || self
                .worker
                .is_some_and(|w| w != ready.scope.worker_generation)
        {
            return Err("Live start did not match its prepared clock, sources or worker.".into());
        }
        self.starting = false;
        self.worker = Some(ready.scope.worker_generation);
        self.next_sequence = ready
            .progress
            .next_sequence()
            .ok_or("Live sequence exhausted")?;
        self.credit = ready.progress.lease_credit;
        self.credit_issue = self.credit.map(|c| c.issued_after_sequence);
        self.next_handle = [1; LIVE_MAX_SOURCES];
        self.parent_sequence = None;
        self.last_lease = Instant::now();
        self.ready = Some(ready);
        // A cumulative fault/progress update can arrive before the ordinary
        // StartLive reply. Installing that older reply must not hide it.
        if let Some(update) = self.latest_update {
            self.apply_update(update);
        }
        Ok(true)
    }
    fn clear_local(&mut self) {
        self.held.clear();
        self.events.fill(None);
        self.event_count = 0;
        self.keyboard_armed = false;
        self.sustain_latch = false;
        self.sustain_key = false;
        self.sustain_effective = [false; 2];
        self.pointer_latched = true;
    }
    pub(super) fn retired(&mut self) {
        self.clear_local();
        self.ready = None;
        self.starting = false;
        self.stop = None;
        self.panic = None;
        self.credit = None;
    }
    pub(super) fn request_stop(&mut self, port: &mut dyn EnginePort) {
        self.clear_local();
        self.cancelled_start = true;
        self.credit = None;
        if self.stop.is_none() {
            match port.request_stop() {
                Ok(ticket) => self.stop = Some(ticket),
                Err(error) => {
                    self.error.get_or_insert(error);
                }
            }
        }
    }
    pub(super) fn cleanup(&mut self, port: &mut dyn EnginePort) {
        self.clear_local();
        self.credit = None;
        let Some(scope) = self.ready.as_ref().map(|r| r.scope) else {
            return;
        };
        if self.stop.is_none() && self.panic.is_none() {
            match port.request_panic_live(scope) {
                Ok(ticket) => self.panic = Some((ticket, scope)),
                Err(error) => {
                    self.error.get_or_insert_with(|| error.to_string());
                }
            }
        }
    }
    fn fail(&mut self, error: impl ToString, port: &mut dyn EnginePort) {
        self.error = Some(error.to_string());
        self.cleanup(port);
    }
    pub(super) fn update(&mut self, update: EngineUpdate) -> bool {
        if !update.is_valid()
            || self.worker.is_some_and(|w| w != update.worker_generation)
            || update.revision <= self.update_revision
        {
            return false;
        }
        self.worker = Some(update.worker_generation);
        self.update_revision = update.revision;
        self.latest_update = Some(update);
        self.apply_update(update)
    }
    fn apply_update(&mut self, update: EngineUpdate) -> bool {
        if let Some(fault) = update.first_fault.filter(|f| {
            self.ready
                .as_ref()
                .is_some_and(|r| r.scope.session_id == f.scope.session_id)
        }) {
            self.error.get_or_insert_with(||format!("Live audition stopped sounding: {:?} at output frame {}. Stop, then start explicitly; this performance was not recorded.",fault.kind,fault.frame));
            self.clear_local();
            self.credit = None;
        }
        if self
            .stop
            .is_some_and(|ticket| update.urgent.stop.is_some_and(|ack| ack.ticket == ticket))
        {
            self.retired();
            return true;
        }
        if let Some((ticket, scope)) = self.panic
            && let Some(ack) = update
                .urgent
                .panic
                .filter(|ack| ack.ticket == ticket && ack.requested_scope == scope)
            && let PanicOutcome::Applied { new_scope } = ack.outcome
            && let Some(progress) = update.live.filter(|p| {
                p.scope == new_scope
                    && p.state == LiveState::Running
                    && p.lease_credit.is_some()
                    && p.active_voices == 0
                    && p.next_sequence() == Some(1)
            })
            && self.error.is_none()
        {
            self.clear_local();
            self.panic = None;
            self.next_handle = [1; LIVE_MAX_SOURCES];
            self.parent_sequence = None;
            self.next_sequence = 1;
            self.credit = progress.lease_credit;
            self.credit_issue = self.credit.map(|c| c.issued_after_sequence);
            self.last_lease = Instant::now();
            if let Some(ready) = &mut self.ready {
                ready.scope = new_scope;
                ready.progress = progress;
            }
        }
        if let Some(progress) = update.live
            && let Some(ready) = &mut self.ready
            && progress.scope == ready.scope
        {
            if progress.worker_accepted_sequence < ready.progress.worker_accepted_sequence
                || progress.callback_applied_sequence < ready.progress.callback_applied_sequence
            {
                return false;
            }
            if progress.state == LiveState::Faulted {
                self.error.get_or_insert_with(|| {
                    format!(
                        "Live audition fault: {:?}. Stop before starting again. Not recorded.",
                        progress.fault.map(|f| f.kind)
                    )
                });
            }
            ready.progress = progress;
            if self.panic.is_none()
                && self.stop.is_none()
                && self.error.is_none()
                && let Some(credit) = progress.lease_credit
                && self
                    .credit_issue
                    .is_none_or(|issue| credit.issued_after_sequence > issue)
            {
                self.credit = Some(credit);
                self.credit_issue = Some(credit.issued_after_sequence);
            }
        }
        false
    }
    fn binding(&self, kind: LiveSourceKind) -> Option<LiveSourceBinding> {
        self.ready
            .as_ref()?
            .sources
            .iter()
            .find(|s| s.kind == kind)
            .copied()
    }
    fn handles(&mut self, source: LiveSourceBinding) -> Option<(u64, u64)> {
        let next = self
            .next_handle
            .get_mut(usize::from(source.handle).checked_sub(1)?)?;
        let id = *next;
        *next = next.checked_add(1)?;
        Some((id, id))
    }
    fn event(
        &self,
        source: LiveSourceBinding,
        event: LiveEventKind,
    ) -> Result<LiveEvent, &'static str> {
        Ok(LiveEvent {
            source: source.handle,
            connection_generation: source.connection_generation,
            observed_ticks: u64::try_from(self.epoch.elapsed().as_nanos())
                .map_err(|_| "GUI monotonic clock exhausted")?,
            event,
        })
    }
    fn stage(&mut self, event: Result<LiveEvent, &'static str>, port: &mut dyn EnginePort) -> bool {
        if !self.running() {
            return false;
        }
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                self.fail(error, port);
                return false;
            }
        };
        if self.event_count == LIVE_BATCH_EVENTS && !self.flush(port, false) {
            return false;
        }
        self.events[self.event_count] = Some(event);
        self.event_count += 1;
        true
    }
    fn flush(&mut self, port: &mut dyn EnginePort, renew: bool) -> bool {
        if !self.running() || self.event_count == 0 && !renew {
            return false;
        }
        let Some(ready) = self.ready.as_ref() else {
            return false;
        };
        let Some(next) = self.next_sequence.checked_add(1) else {
            self.fail("Live batch sequence exhausted", port);
            return false;
        };
        let token = self.credit.map(|c| c.token);
        let mut batch = LiveBatch::empty(ready.scope, self.next_sequence, token);
        batch.len = self.event_count as u8;
        batch.events = self.events;
        match port.try_push_live(batch) {
            Ok(queued) if queued.scope == batch.scope && queued.sequence == batch.sequence => {
                self.next_sequence = next;
                self.parent_sequence = Some(batch.sequence);
                self.events.fill(None);
                self.event_count = 0;
                if token.is_some() {
                    self.credit = None;
                    self.last_lease = Instant::now();
                }
                true
            }
            Ok(_) => {
                self.fail(
                    "Live parent admission did not match the submitted batch",
                    port,
                );
                false
            }
            Err(error) => {
                self.fail(error, port);
                false
            }
        }
    }
    fn press(
        &mut self,
        origin: Origin,
        kind: LiveSourceKind,
        key: u8,
        patch: LivePatch,
        port: &mut dyn EnginePort,
    ) {
        if !self.running() || self.held.iter().any(|h| h.origin == origin) {
            return;
        }
        if self.held.len() == LIVE_VOICES {
            self.fail("Too many held live controls", port);
            return;
        }
        let Some(source) = self.binding(kind) else {
            return;
        };
        let Some((note, voice)) = self.handles(source) else {
            self.fail("Live note handles exhausted", port);
            return;
        };
        if self.stage(
            self.event(
                source,
                LiveEventKind::NoteOn {
                    note,
                    voice,
                    key,
                    velocity16: self.velocity,
                    patch,
                },
            ),
            port,
        ) {
            self.held.push(Held {
                origin,
                source,
                voice,
                key,
                patch,
            });
        }
    }
    fn release(&mut self, origin: Origin, port: &mut dyn EnginePort) {
        if let Some(index) = self.held.iter().position(|h| h.origin == origin) {
            let held = self.held.remove(index);
            self.stage(
                self.event(
                    held.source,
                    LiveEventKind::NoteOff {
                        voice: held.voice,
                        release_velocity16: self.release_velocity,
                    },
                ),
                port,
            );
        }
    }
    fn pulse(&mut self, key: u8, patch: LivePatch, port: &mut dyn EnginePort) {
        if !self.running() {
            return;
        }
        let Some(source) = self.binding(LiveSourceKind::AccessiblePulse) else {
            return;
        };
        let Some((note, voice)) = self.handles(source) else {
            self.fail("Live pulse handles exhausted", port);
            return;
        };
        let duration_frames = self.ready.as_ref().unwrap().sample_rate / 10;
        self.stage(
            self.event(
                source,
                LiveEventKind::Pulse {
                    note,
                    voice,
                    key,
                    velocity16: self.velocity,
                    release_velocity16: self.release_velocity,
                    patch,
                    duration_frames,
                },
            ),
            port,
        );
    }
    fn sustain(&mut self, port: &mut dyn EnginePort) {
        for (index, kind) in [LiveSourceKind::ComputerKeys, LiveSourceKind::PointerKeys]
            .into_iter()
            .enumerate()
        {
            let down = self.sustain_latch || (index == 0 && self.sustain_key);
            if self.sustain_effective[index] != down
                && let Some(source) = self.binding(kind)
                && self.stage(self.event(source, LiveEventKind::Sustain { down }), port)
            {
                self.sustain_effective[index] = down;
            }
        }
    }
    fn computer_event(
        &mut self,
        key: egui::Key,
        identity: egui::Key,
        pressed: bool,
        repeat: bool,
        modifiers: egui::Modifiers,
        port: &mut dyn EnginePort,
    ) {
        if !pressed {
            self.release(Origin::Computer(identity), port);
            if key == egui::Key::Space {
                self.sustain_key = false;
                self.sustain(port);
            }
            return;
        }
        if !self.keyboard_armed || repeat || modifiers.any() || !self.running() {
            return;
        }
        if key == egui::Key::Space && self.mode == Mode::Keys {
            self.sustain_key = true;
            self.sustain(port);
            return;
        }
        if key == egui::Key::Escape {
            self.cleanup(port);
            return;
        }
        match self.mode {
            Mode::Keys => {
                if let Some(index) = KEYS.iter().position(|k| *k == key) {
                    self.press(
                        Origin::Computer(identity),
                        LiveSourceKind::ComputerKeys,
                        (12 * (self.octave + 1) + index as i32) as u8,
                        self.patch,
                        port,
                    )
                }
            }
            Mode::Pads => {
                if let Some(index) = PAD_KEYS.iter().position(|k| *k == key) {
                    self.press(
                        Origin::Computer(identity),
                        LiveSourceKind::Pads,
                        PAD_NOTES[index],
                        LivePatch::Drums,
                        port,
                    )
                }
            }
        }
    }
    pub(super) fn input(&mut self, ctx: &egui::Context, blocked: bool, port: &mut dyn EnginePort) {
        if !self.engaged() {
            return;
        }
        let focused = ctx.input(|i| i.focused);
        let field = ctx.text_edit_focused();
        let popup = ctx.any_popup_open();
        let focus = ctx.memory(|m| m.focused());
        let in_zone = focus.is_some_and(|id| self.focus_ids.contains(&id));
        if (!focused || field || popup || blocked || self.keyboard_armed && !in_zone)
            && (self.keyboard_armed
                || !self.held.is_empty()
                || self.sustain_latch
                || self.event_count > 0)
        {
            self.cleanup(port);
        }
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed,
                        repeat,
                        modifiers,
                    } => {
                        // Space on a focused key/pad is its normal bounded
                        // accessibility pulse, never an additional sustain.
                        if *key != egui::Key::Space
                            || !*pressed
                            || focus == self.focus_ids.first().copied()
                        {
                            self.computer_event(
                                *key,
                                physical_key.unwrap_or(*key),
                                *pressed,
                                *repeat,
                                *modifiers,
                                port,
                            )
                        }
                    }
                    egui::Event::WindowFocused(false) | egui::Event::PointerGone => {
                        if self.running()
                            && (self.keyboard_armed
                                || !self.held.is_empty()
                                || self.sustain_latch
                                || self.event_count > 0)
                        {
                            self.cleanup(port)
                        }
                    }
                    egui::Event::Ime(_)
                        if self.keyboard_armed || !self.held.is_empty() || self.event_count > 0 =>
                    {
                        self.cleanup(port)
                    }
                    _ => {}
                }
            }
        });
        if self.running() {
            let renew =
                self.credit.is_some() && self.last_lease.elapsed() >= Duration::from_millis(250);
            self.flush(port, renew);
        }
    }
    fn surface_button(
        &mut self,
        ui: &mut egui::Ui,
        label: String,
        note: (Origin, LiveSourceKind, u8, LivePatch),
        port: &mut dyn EnginePort,
    ) {
        let (origin, kind, key, patch) = note;
        let response = ui.add_enabled(
            self.running() && ui.input(|i| i.focused),
            egui::Button::new(label)
                .selected(
                    self.held
                        .iter()
                        .any(|held| held.key == key && held.patch == patch),
                )
                .min_size(egui::vec2(72.0, 42.0)),
        );
        self.focus_ids.push(response.id);
        let down = response.is_pointer_button_down_on()
            && response
                .interact_pointer_pos()
                .is_some_and(|p| response.rect.contains(p));
        if down && !self.pointer_latched {
            self.pointer_latched = true;
            self.press(origin, kind, key, patch, port)
        }
        if !down {
            self.release(origin, port)
        }
        let repeated_or_modified=ui.input(|i|i.events.iter().any(|event|matches!(event,egui::Event::Key{key:egui::Key::Space|egui::Key::Enter,pressed:true,repeat,modifiers,..}if *repeat||modifiers.any())));
        if response.clicked()
            && !response.clicked_by(egui::PointerButton::Primary)
            && !repeated_or_modified
        {
            self.pulse(key, patch, port);
        }
    }
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, port: &mut dyn EnginePort) {
        if !self.engaged() && self.error.is_none() {
            return;
        }
        ui.separator();
        ui.strong("Live audition · output only · not recording or saved");
        ui.small("Procedural instruments, not AI. No microphone, monitoring, journal or session edit. Stop before changing sessions.");
        if let Some(ready) = &self.ready {
            ui.label(format!(
                "{} · {} Hz · {} output channels",
                ready.output.name, ready.sample_rate, ready.channels
            ));
            ui.small(format!("Parent queued {:?} · worker accepted {:?} · callback applied {:?} · {} active voices ({} release tails)",self.parent_sequence,ready.progress.worker_accepted_sequence,ready.progress.callback_applied_sequence,ready.progress.active_voices,ready.progress.release_tail_voices));
        }
        if self.stop.is_some() {
            ui.label("Stop requested. Waiting for output closure acknowledgement.");
        }
        if self.panic.is_some() {
            ui.label("All notes off requested. Fresh notes wait for callback-confirmed cleanup.");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        self.focus_ids.clear();
        if !ui.input(|i| i.pointer.any_down()) {
            self.pointer_latched = false;
        }
        ui.horizontal_wrapped(|ui| {
            let old = self.mode;
            ui.add_enabled_ui(self.running(), |ui| {
                ui.selectable_value(&mut self.mode, Mode::Keys, "Computer mapping: Keys");
                ui.selectable_value(&mut self.mode, Mode::Pads, "Computer mapping: Pads");
            });
            if old != self.mode {
                self.cleanup(port);
            }
            let response = ui.add_enabled(
                self.running(),
                egui::Button::new(if self.keyboard_armed {
                    "Computer keys enabled · focus here to play"
                } else {
                    "Enable computer keys"
                }),
            );
            self.focus_ids.push(response.id);
            if response.clicked() {
                self.keyboard_armed = true;
                response.request_focus();
            }
            if ui
                .add_enabled(
                    self.ready.is_some() && self.stop.is_none(),
                    egui::Button::new("All notes off"),
                )
                .clicked()
            {
                self.cleanup(port);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let old = self.octave;
            ui.add_enabled_ui(self.running(), |ui| {
                ui.label("Octave");
                ui.add(egui::DragValue::new(&mut self.octave).range(0..=8));
                egui::ComboBox::from_id_salt("live-patch")
                    .selected_text(format!("{:?}", self.patch))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.patch, LivePatch::Keys, "Keys");
                        ui.selectable_value(&mut self.patch, LivePatch::Bass, "Bass");
                        ui.selectable_value(&mut self.patch, LivePatch::Pad, "Pad");
                    })
                    .response
                    .accessible_name("Live keyboard patch");
                ui.label("Velocity 16-bit");
                ui.add(egui::DragValue::new(&mut self.velocity).range(0..=u16::MAX))
                    .accessible_name("Live note velocity");
                ui.label("Release velocity");
                ui.add(egui::DragValue::new(&mut self.release_velocity).range(0..=u16::MAX))
                    .accessible_name("Live release velocity");
            });
            if old != self.octave {
                self.cleanup(port);
            }
            let sustain_response = ui.add_enabled(
                self.running(),
                egui::Checkbox::new(&mut self.sustain_latch, "Keys sustain"),
            );
            self.focus_ids.push(sustain_response.id);
            if sustain_response.changed() {
                self.sustain(port);
            }
        });
        ui.small("Keys: A W S E D F T G Y H U J K · Space holds computer-key sustain. Click the performance area to enable; typing and shortcuts never play. Accessibility activation plays a 100 ms pulse.");
        ui.horizontal_wrapped(|ui| {
            for (index, key_name) in KEY_NAMES.iter().enumerate() {
                let key = (12 * (self.octave + 1) + index as i32) as u8;
                let octave = i32::from(key) / 12 - 1;
                self.surface_button(
                    ui,
                    format!(
                        "{}{} · {}",
                        NOTE_NAMES[usize::from(key % 12)],
                        octave,
                        key_name
                    ),
                    (
                        Origin::PointerKey(index as u8),
                        LiveSourceKind::PointerKeys,
                        key,
                        self.patch,
                    ),
                    port,
                );
            }
        });
        egui::Grid::new("live-pads").num_columns(4).show(ui, |ui| {
            for (index, pad_name) in PAD_NAMES.iter().enumerate() {
                self.surface_button(
                    ui,
                    format!("Pad {} · {} · {}", index + 1, pad_name, PAD_LABELS[index]),
                    (
                        Origin::PointerPad(index as u8),
                        LiveSourceKind::Pads,
                        PAD_NOTES[index],
                        LivePatch::Drums,
                    ),
                    port,
                );
                if index % 4 == 3 {
                    ui.end_row();
                }
            }
        });
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    pub(crate) fn ready(clock: LiveClock) -> LiveReady {
        let scope = LiveScope {
            worker_generation: LiveId([1; 16]),
            session_id: LiveId([2; 16]),
            panic_generation: 1,
        };
        LiveReady {
            scope,
            gui_clock: clock,
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
            output: DeviceInfo {
                id: "synthetic-output".into(),
                name: "Synthetic output only".into(),
            },
            sample_rate: 48_000,
            channels: 2,
            progress: progress(scope),
        }
    }
    pub(crate) fn progress(scope: LiveScope) -> LiveProgress {
        LiveProgress {
            scope,
            state: LiveState::Running,
            worker_accepted_sequence: None,
            callback_applied_sequence: None,
            active_voices: 0,
            release_tail_voices: 0,
            late_releases: 0,
            rejected_batches: 0,
            overflow_batches: 0,
            frames: 0,
            peak: 0.0,
            lease_deadline_frame: 48_000,
            lease_credit: Some(LeaseCredit {
                scope,
                token: LiveId([3; 16]),
                issued_after_sequence: None,
            }),
            fault: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    #[derive(Default)]
    struct Port {
        batches: Vec<LiveBatch>,
        panics: Vec<(UrgentTicket, LiveScope)>,
        stops: Vec<UrgentTicket>,
        reject: bool,
    }
    impl EnginePort for Port {
        fn try_submit(&mut self, _: EngineCommand) -> Result<(), String> {
            Err("control path deliberately held".into())
        }
        fn try_poll(&mut self) -> Option<EngineReply> {
            None
        }
        fn is_busy(&self) -> bool {
            true
        }
        fn is_retired(&self) -> bool {
            false
        }
        fn try_push_live(&mut self, batch: LiveBatch) -> Result<LiveQueued, LiveSubmitError> {
            if self.reject {
                return Err(LiveSubmitError::Saturated);
            }
            self.batches.push(batch);
            Ok(LiveQueued {
                scope: batch.scope,
                sequence: batch.sequence,
            })
        }
        fn request_stop(&mut self) -> Result<UrgentTicket, String> {
            let ticket = UrgentTicket {
                worker_generation: LiveId([1; 16]),
                id: (self.stops.len() + self.panics.len() + 1) as u64,
            };
            self.stops.push(ticket);
            Ok(ticket)
        }
        fn request_panic_live(
            &mut self,
            scope: LiveScope,
        ) -> Result<UrgentTicket, LiveSubmitError> {
            let ticket = UrgentTicket {
                worker_generation: scope.worker_generation,
                id: (self.stops.len() + self.panics.len() + 1) as u64,
            };
            self.panics.push((ticket, scope));
            Ok(ticket)
        }
        fn try_poll_update(&mut self) -> Option<EngineUpdate> {
            None
        }
    }
    fn active() -> (LiveUi, Port) {
        let mut live = LiveUi::new();
        let clock = live.begin();
        live.activate(ready(clock)).unwrap();
        (live, Port::default())
    }
    fn events(port: &Port) -> Vec<LiveEvent> {
        port.batches
            .iter()
            .flat_map(|b| b.events[..usize::from(b.len)].iter().copied().flatten())
            .collect()
    }
    fn key(live: &mut LiveUi, port: &mut Port, key: egui::Key, pressed: bool) {
        live.computer_event(key, key, pressed, false, egui::Modifiers::NONE, port)
    }
    fn update(
        live: &LiveUi,
        revision: u64,
        progress: LiveProgress,
        urgent: UrgentProgress,
    ) -> EngineUpdate {
        EngineUpdate {
            worker_generation: live.ready.as_ref().unwrap().scope.worker_generation,
            revision,
            live: Some(progress),
            urgent,
            first_fault: None,
        }
    }
    fn frame(
        live: &mut LiveUi,
        port: &mut Port,
        ctx: &egui::Context,
        input: egui::RawInput,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(input, |ui| live.ui(ui, port));
        output.textures_delta.clear();
        output
    }
    fn raw(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1200.0),
            )),
            focused: true,
            events,
            ..Default::default()
        }
    }
    #[test]
    fn ordered_on_off_on_and_source_handles_preserve_full_velocities_while_control_busy() {
        let (mut live, mut port) = active();
        live.keyboard_armed = true;
        live.velocity = 0;
        live.release_velocity = u16::MAX;
        key(&mut live, &mut port, egui::Key::A, true);
        key(&mut live, &mut port, egui::Key::A, false);
        live.velocity = u16::MAX;
        key(&mut live, &mut port, egui::Key::A, true);
        live.press(
            Origin::PointerKey(0),
            LiveSourceKind::PointerKeys,
            60,
            LivePatch::Bass,
            &mut port,
        );
        live.flush(&mut port, false);
        let e = events(&port);
        assert_eq!(e.len(), 4);
        assert!(matches!(
            e[0].event,
            LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                velocity16: 0,
                ..
            }
        ));
        assert!(matches!(
            e[1].event,
            LiveEventKind::NoteOff {
                voice: 1,
                release_velocity16: u16::MAX
            }
        ));
        assert!(matches!(
            e[2].event,
            LiveEventKind::NoteOn {
                note: 2,
                voice: 2,
                velocity16: u16::MAX,
                ..
            }
        ));
        assert!(matches!(
            e[3].event,
            LiveEventKind::NoteOn {
                note: 1,
                voice: 1,
                patch: LivePatch::Bass,
                ..
            }
        ));
        assert_ne!(e[2].source, e[3].source);
        assert!(port.is_busy());
    }
    #[test]
    fn lease_credit_is_consumed_once_and_only_new_matching_watermark_replaces_it() {
        let (mut live, mut port) = active();
        let scope = live.ready.as_ref().unwrap().scope;
        live.last_lease = Instant::now() - Duration::from_secs(1);
        assert!(live.flush(&mut port, true));
        assert!(port.batches[0].lease_token.is_some());
        let unchanged = progress(scope);
        live.update(update(&live, 1, unchanged, UrgentProgress::default()));
        assert!(live.credit.is_none());
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        live.flush(&mut port, false);
        assert!(port.batches[1].lease_token.is_none());
        let mut fresh = unchanged;
        fresh.worker_accepted_sequence = Some(1);
        fresh.callback_applied_sequence = Some(1);
        fresh.lease_credit = Some(LeaseCredit {
            scope,
            token: LiveId([4; 16]),
            issued_after_sequence: Some(1),
        });
        live.update(update(&live, 2, fresh, UrgentProgress::default()));
        assert_eq!(live.credit.unwrap().token, LiveId([4; 16]));
        live.flush(&mut port, true);
        assert!(live.credit.is_none());
        live.update(update(&live, 3, fresh, UrgentProgress::default()));
        assert!(live.credit.is_none());
        assert_eq!(port.batches.len(), 3);
    }
    #[test]
    fn panic_waits_for_exact_ticket_callback_scope_credit_and_fresh_human_press() {
        let (mut live, mut port) = active();
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        live.cleanup(&mut port);
        assert!(live.held.is_empty());
        assert_eq!(live.event_count, 0);
        let (ticket, old) = port.panics[0];
        let new = LiveScope {
            panic_generation: 2,
            ..old
        };
        let p = progress(new);
        let wrong = UrgentProgress {
            stop: None,
            panic: Some(PanicAck {
                ticket: UrgentTicket {
                    id: ticket.id + 1,
                    ..ticket
                },
                requested_scope: old,
                outcome: PanicOutcome::Applied { new_scope: new },
            }),
        };
        live.update(update(&live, 1, p, wrong));
        assert!(!live.running());
        let ack = UrgentProgress {
            stop: None,
            panic: Some(PanicAck {
                ticket,
                requested_scope: old,
                outcome: PanicOutcome::Applied { new_scope: new },
            }),
        };
        live.update(update(&live, 2, p, ack));
        assert!(live.running());
        assert!(!live.keyboard_armed);
        assert_eq!(live.next_sequence, 1);
        key(&mut live, &mut port, egui::Key::A, true);
        assert_eq!(live.event_count, 0);
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        assert_eq!(live.event_count, 1);
    }
    #[test]
    fn full_parent_queue_clears_held_and_requests_urgent_panic_without_retry() {
        let (mut live, mut port) = active();
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        port.reject = true;
        assert!(!live.flush(&mut port, false));
        assert!(live.held.is_empty());
        assert_eq!(
            live.error.as_deref(),
            Some(LiveSubmitError::Saturated.to_string().as_str())
        );
        assert_eq!(port.panics.len(), 1);
        assert!(port.batches.is_empty());
        port.reject = false;
        key(&mut live, &mut port, egui::Key::A, false);
        key(&mut live, &mut port, egui::Key::A, true);
        assert!(!live.flush(&mut port, false));
        assert!(port.batches.is_empty());
    }
    #[test]
    fn repeat_shortcuts_and_sustain_reasons_are_scoped_and_releases_still_work() {
        let (mut live, mut port) = active();
        live.keyboard_armed = true;
        live.computer_event(
            egui::Key::A,
            egui::Key::A,
            true,
            true,
            egui::Modifiers::NONE,
            &mut port,
        );
        live.computer_event(
            egui::Key::S,
            egui::Key::S,
            true,
            false,
            egui::Modifiers::COMMAND,
            &mut port,
        );
        assert!(live.held.is_empty());
        key(&mut live, &mut port, egui::Key::A, true);
        live.computer_event(
            egui::Key::A,
            egui::Key::A,
            false,
            false,
            egui::Modifiers::COMMAND,
            &mut port,
        );
        assert!(live.held.is_empty());
        live.sustain_latch = true;
        live.sustain(&mut port);
        key(&mut live, &mut port, egui::Key::Space, true);
        key(&mut live, &mut port, egui::Key::Space, false);
        assert_eq!(live.sustain_effective, [true, true]);
        live.sustain_latch = false;
        live.sustain(&mut port);
        live.flush(&mut port, false);
        let sustain: Vec<_> = events(&port)
            .into_iter()
            .filter(|e| matches!(e.event, LiveEventKind::Sustain { .. }))
            .collect();
        assert_eq!(sustain.len(), 4);
        assert_ne!(sustain[0].source, sustain[1].source);
        assert!(matches!(
            sustain[2].event,
            LiveEventKind::Sustain { down: false }
        ));
    }
    #[test]
    fn accessibility_has_named_keys_and_pads_and_uses_one_callback_owned_pulse() {
        let (mut live, mut port) = active();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![]));
        let output = frame(&mut live, &mut port, &ctx, raw(vec![]));
        let tree = output.platform_output.accesskit_update.unwrap();
        let (target, _) = tree
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some("C4 · A"))
            .expect("Named accessible key");
        assert!(
            tree.nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Pad 16 · Percussion · V"))
        );
        let _ = frame(
            &mut live,
            &mut port,
            &ctx,
            raw(vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target_tree: tree.tree_id,
                    target_node: *target,
                    data: None,
                },
            )]),
        );
        live.flush(&mut port, false);
        let e = events(&port);
        assert_eq!(e.len(), 1);
        assert!(matches!(
            e[0].event,
            LiveEventKind::Pulse {
                duration_frames: 4800,
                ..
            }
        ));
        assert!(live.held.is_empty());
    }
    #[test]
    fn focused_text_input_and_window_loss_cleanup_before_more_input_or_lease() {
        let (mut live, mut port) = active();
        let ctx = egui::Context::default();
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![]));
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        let mut text = String::new();
        let mut output = ctx.run_ui(raw(vec![]), |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
        });
        output.textures_delta.clear();
        live.input(&ctx, false, &mut port);
        assert!(live.held.is_empty());
        assert_eq!(port.panics.len(), 1);
        assert!(port.batches.is_empty());
        let (mut live, mut port) = active();
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        let _ = frame(
            &mut live,
            &mut port,
            &ctx,
            raw(vec![egui::Event::WindowFocused(false)]),
        );
        live.input(&ctx, false, &mut port);
        assert!(live.held.is_empty());
        assert!(!live.keyboard_armed);
        assert_eq!(port.panics.len(), 1);
    }
    #[test]
    fn raw_keyboard_events_keep_same_frame_order_and_pads_use_exclusive_mapping() {
        let (mut live, mut port) = active();
        let ctx = egui::Context::default();
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![]));
        ctx.memory_mut(|m| m.request_focus(live.focus_ids[0]));
        live.keyboard_armed = true;
        let event = |pressed| egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(
            &mut live,
            &mut port,
            &ctx,
            raw(vec![event(true), event(false), event(true)]),
        );
        live.input(&ctx, false, &mut port);
        assert_eq!(events(&port).len(), 3);
        assert!(matches!(
            events(&port)[1].event,
            LiveEventKind::NoteOff { voice: 1, .. }
        ));
        let (mut live, mut port) = active();
        live.mode = Mode::Pads;
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        live.flush(&mut port, false);
        assert!(matches!(
            events(&port)[0].event,
            LiveEventKind::NoteOn {
                key: 49,
                patch: LivePatch::Drums,
                ..
            }
        ));
    }
    #[test]
    fn pointer_release_does_not_double_sound_and_pointer_cancel_uses_panic_latch() {
        let (mut live, mut port) = active();
        let ctx = egui::Context::default();
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![]));
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![]));
        let pos = ctx.read_response(live.focus_ids[2]).unwrap().rect.center();
        let event = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(
            &mut live,
            &mut port,
            &ctx,
            raw(vec![egui::Event::PointerMoved(pos), event(true)]),
        );
        live.flush(&mut port, false);
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![event(false)]));
        live.flush(&mut port, false);
        let e = events(&port);
        assert_eq!(e.len(), 2);
        assert!(matches!(e[0].event, LiveEventKind::NoteOn { .. }));
        assert!(matches!(e[1].event, LiveEventKind::NoteOff { .. }));
        assert!(live.held.is_empty());
        let _ = frame(&mut live, &mut port, &ctx, raw(vec![event(true)]));
        live.flush(&mut port, false);
        let _ = frame(
            &mut live,
            &mut port,
            &ctx,
            raw(vec![egui::Event::PointerGone]),
        );
        live.input(&ctx, false, &mut port);
        assert!(live.held.is_empty());
        assert_eq!(port.panics.len(), 1);
        assert_eq!(
            events(&port).len(),
            3,
            "Cancelled release is replaced by acknowledged Panic, not retried"
        );
    }
    #[test]
    fn fault_update_before_start_reply_cannot_be_overwritten_by_older_ready_snapshot() {
        let mut live = LiveUi::new();
        let clock = live.begin();
        let ready = ready(clock);
        let fault = LiveFault {
            scope: ready.scope,
            kind: LiveFaultKind::LeaseExpired,
            frame: 48_000,
            sequence: None,
        };
        let mut progress = ready.progress;
        progress.state = LiveState::Faulted;
        progress.lease_credit = None;
        progress.fault = Some(fault);
        live.update(EngineUpdate {
            worker_generation: ready.scope.worker_generation,
            revision: 1,
            live: Some(progress),
            urgent: UrgentProgress::default(),
            first_fault: Some(fault),
        });
        live.activate(ready).unwrap();
        assert!(!live.running());
        assert!(live.error.as_ref().unwrap().contains("LeaseExpired"));
        let mut port = Port::default();
        live.keyboard_armed = true;
        key(&mut live, &mut port, egui::Key::A, true);
        assert_eq!(live.event_count, 0);
        assert!(port.batches.is_empty());
    }
}
