//! Window-local note drafts. Durable mutation and full validation stay on the
//! session worker; display borrows only bounded known source fields.
use eframe::egui;
use omniusong_core::contracts::{
    EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId, Validate,
};
use omniusong_core::{DocumentNoteAddress, NoteTimelinePolicy, NoteValues, ProjectDocument};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NoteSelection {
    pub project_id: String,
    pub address: DocumentNoteAddress,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NoteDraft {
    pub selection: NoteSelection,
    pub base_revision: u64,
    pub pitch: String,
    pub start: String,
    pub duration: String,
    pub velocity: String,
    pub extend_loop: bool,
    pub allow_upgrade: bool,
    pub is_new: bool,
    original: [String; 4],
}
impl NoteDraft {
    fn new(selection: NoteSelection, revision: u64, values: NoteValues, is_new: bool) -> Self {
        let original = [
            values.pitch.to_string(),
            values.start_tick.to_string(),
            values.duration_ticks.to_string(),
            values.velocity16.to_string(),
        ];
        Self {
            selection,
            base_revision: revision,
            pitch: original[0].clone(),
            start: original[1].clone(),
            duration: original[2].clone(),
            velocity: original[3].clone(),
            original,
            extend_loop: false,
            allow_upgrade: false,
            is_new,
        }
    }
    pub fn values(&self) -> Result<NoteValues, String> {
        fn integer<T: std::str::FromStr>(raw: &str, label: &str) -> Result<T, String> {
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{label} must be a whole nonnegative integer"));
            }
            raw.parse()
                .map_err(|_| format!("{label} is outside its integer range"))
        }
        let values = NoteValues {
            pitch: integer(&self.pitch, "Pitch")?,
            start_tick: integer(&self.start, "Start")?,
            duration_ticks: integer(&self.duration, "Length")?,
            velocity16: integer(&self.velocity, "Velocity")?,
        };
        values.validate().map_err(|_| {
            "Pitch must be 0–127, start 0–1000000000, length 1–100000000 and velocity 0–65535"
                .to_string()
        })?;
        Ok(values)
    }
    pub fn dirty(&self) -> bool {
        self.is_new
            || self.pitch != self.original[0]
            || self.start != self.original[1]
            || self.duration != self.original[2]
            || self.velocity != self.original[3]
            || self.extend_loop
    }
    fn timeline(&self) -> NoteTimelinePolicy {
        if self.extend_loop {
            NoteTimelinePolicy::ExtendLoop {
                allow_native_schema_upgrade: self.allow_upgrade,
            }
        } else {
            NoteTimelinePolicy::PreserveLoop
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum NoteAction {
    Select(NoteSelection),
    Track(String),
    Add(NoteSelection),
    BeginMetadata,
    BeginNote,
    Save,
    Discard,
    Delete(NoteSelection),
    Restore(NoteSelection, NoteTimelinePolicy),
    Retry,
    Suspend,
    ReviewLatest,
    NewChoice,
    ReleaseRejected,
}
#[derive(Clone)]
pub(super) struct NoteFailure {
    pub intent: ProjectEditIntent,
    pub draft: Option<NoteDraft>,
    pub message: String,
    pub known_rejected: bool,
    pub suspended: bool,
    reviewed_revision: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NoteContext {
    selection: Option<NoteSelection>,
    pub draft: Option<NoteDraft>,
    serial: u64,
}
#[derive(Default)]
pub(super) struct NoteEditor {
    pub selection: Option<NoteSelection>,
    pub draft: Option<NoteDraft>,
    pub failure: Option<NoteFailure>,
    serial: u64,
    pub notice: Option<String>,
    // One resolved choice is retained for inspection, never used as a new request.
    released: Option<NoteFailure>,
}
pub(super) fn track<'a>(doc: &'a ProjectDocument, id: &str) -> Option<&'a Value> {
    doc.tracks()
        .iter()
        .find(|track| track["id"].as_str() == Some(id))
}
pub(super) fn note<'a>(doc: &'a ProjectDocument, selection: &NoteSelection) -> Option<&'a Value> {
    if doc.id() != selection.project_id {
        return None;
    }
    let track = track(doc, &selection.address.track_id)?;
    if track["kind"] != "notes" {
        return None;
    }
    track["notes"]
        .as_array()?
        .iter()
        .find(|note| note["id"].as_str() == Some(&selection.address.note_id))
}
fn values(note: &Value) -> Option<NoteValues> {
    Some(NoteValues {
        pitch: u8::try_from(note["pitch"].as_u64()?).ok()?,
        start_tick: note["startTick"].as_u64()?,
        duration_ticks: note["durationTicks"].as_u64()?,
        velocity16: u16::try_from(note["velocity16"].as_u64()?).ok()?,
    })
}
impl NoteEditor {
    fn changed(&mut self) {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("note draft serial exhausted");
    }
    pub fn context(&self) -> NoteContext {
        NoteContext {
            selection: self.selection.clone(),
            draft: self.draft.clone(),
            serial: self.serial,
        }
    }
    pub fn dirty(&self) -> bool {
        self.draft.as_ref().is_some_and(NoteDraft::dirty)
    }
    pub fn unresolved(&self) -> bool {
        self.failure.is_some()
    }
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.draft = None;
        self.changed();
    }
    pub fn select(
        &mut self,
        doc: &ProjectDocument,
        selection: NoteSelection,
    ) -> Result<(), String> {
        let saved = note(doc, &selection).ok_or("Selected note is no longer present")?;
        let value = values(saved).ok_or("Selected note has no editable values")?;
        self.selection = Some(selection.clone());
        self.draft = Some(NoteDraft::new(selection, doc.revision(), value, false));
        self.changed();
        Ok(())
    }
    pub fn add(&mut self, doc: &ProjectDocument, selection: NoteSelection) -> Result<(), String> {
        let selected_track =
            track(doc, &selection.address.track_id).ok_or("Track is no longer present")?;
        if selected_track["kind"] != "notes"
            || selected_track["locked"] != false
            || selection.project_id != doc.id()
        {
            return Err("Choose an unlocked notes track".into());
        }
        self.selection = Some(selection.clone());
        self.draft = Some(NoteDraft::new(
            selection,
            doc.revision(),
            NoteValues {
                pitch: 60,
                start_tick: 0,
                duration_ticks: 960,
                velocity16: 49152,
            },
            true,
        ));
        self.changed();
        Ok(())
    }
    pub fn discard(&mut self, doc: &ProjectDocument) {
        if self.failure.is_some() {
            return;
        }
        if self.draft.as_ref().is_some_and(|d| d.is_new) {
            self.draft = None;
            self.selection = None;
            self.changed();
            return;
        }
        let selected = self.selection.clone();
        self.draft = None;
        if let Some(selection) = selected {
            let _ = self.select(doc, selection);
        }
        self.changed();
    }
    pub fn document_installed(&mut self, doc: &ProjectDocument) {
        // A missing identity remains selected as a tombstone for Undo; it must
        // never silently select an array neighbour.
        if self
            .selection
            .as_ref()
            .is_some_and(|s| s.project_id != doc.id())
        {
            self.selection = None;
        }
        self.draft = None;
        if let Some(selection) = self.selection.clone() {
            let _ = self.select(doc, selection);
        }
        if let Some(failure) = &mut self.failure
            && failure.suspended
            && failure.known_rejected
            && failure.intent.project_id.0 == doc.id()
        {
            failure.reviewed_revision = Some(doc.revision());
        }
        self.changed();
    }
    pub fn intent(
        &self,
        doc: &ProjectDocument,
        action: &NoteAction,
    ) -> Result<ProjectEditIntent, String> {
        if self.failure.is_some() {
            return Err("Resolve the retained note edit before making another note change".into());
        }
        let (selection, edit) = match action {
            NoteAction::Save => {
                let draft = self.draft.as_ref().ok_or("Select a note first")?;
                if !draft.dirty() {
                    return Err("The note has no unsaved change".into());
                }
                let values = draft.values()?;
                let edit = if draft.is_new {
                    EditOperation::AddNote {
                        values,
                        timeline: draft.timeline(),
                    }
                } else {
                    EditOperation::UpdateNote {
                        values,
                        timeline: draft.timeline(),
                    }
                };
                (draft.selection.clone(), edit)
            }
            NoteAction::Delete(selection) => (selection.clone(), EditOperation::DeleteNote),
            NoteAction::Restore(selection, timeline) => (
                selection.clone(),
                EditOperation::RestoreNote {
                    timeline: *timeline,
                },
            ),
            _ => return Err("This note choice does not submit an edit".into()),
        };
        if selection.project_id != doc.id() {
            return Err("Reopen the target session before editing this note".into());
        }
        let selected_track =
            track(doc, &selection.address.track_id).ok_or("Track is no longer present")?;
        if selected_track["kind"] != "notes" || selected_track["locked"] != false {
            return Err("Choose an unlocked notes track".into());
        }
        Ok(ProjectEditIntent {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            project_id: ProjectId(doc.id().into()),
            expected_revision: doc.revision(),
            scope: EditScope::DocumentNote {
                address: selection.address,
            },
            edit,
            extensions: Default::default(),
        })
    }
    pub fn failed(&mut self, intent: ProjectEditIntent, message: String, known_rejected: bool) {
        self.failed_with_draft(intent, message, known_rejected, self.draft.clone());
    }
    pub fn not_admitted(&mut self, intent: ProjectEditIntent, message: String) {
        if let Some(failure) = &mut self.failure
            && failure.intent == intent
        {
            failure.message = format!(
                "This retry was not admitted: {message}. The earlier outcome classification is unchanged."
            );
        } else {
            self.failed(
                intent,
                format!("Not admitted to the session worker: {message}"),
                true,
            );
        }
    }
    pub fn failed_with_draft(
        &mut self,
        intent: ProjectEditIntent,
        message: String,
        known_rejected: bool,
        draft: Option<NoteDraft>,
    ) {
        // A retry failure keeps the original bounded draft even after navigation.
        if let Some(failure) = &mut self.failure
            && failure.intent == intent
        {
            failure.message = message;
            failure.known_rejected = known_rejected;
            return;
        }
        self.failure = Some(NoteFailure {
            intent,
            draft,
            message,
            known_rejected,
            suspended: false,
            reviewed_revision: None,
        });
    }
    pub fn suspend(&mut self) {
        if let Some(failure) = &mut self.failure {
            failure.suspended = true;
            self.draft = None;
            self.changed();
        }
    }
    pub fn new_choice(&mut self, doc: &ProjectDocument) -> Result<(), String> {
        let failure = self.failure.as_ref().ok_or("No retained edit")?;
        if !failure.known_rejected
            || !failure.suspended
            || failure.intent.project_id.0 != doc.id()
            || failure.reviewed_revision != Some(doc.revision())
        {
            return Err("Review the latest saved target session before making a new choice".into());
        }
        let old = failure.clone();
        let attempted = old.draft.clone();
        let mut new_draft = None;
        if let Some(attempted) = attempted {
            let selected_track = track(doc, &attempted.selection.address.track_id)
                .ok_or("The original track is absent; retained draft kept")?;
            if selected_track["kind"] != "notes" || selected_track["locked"] != false {
                return Err("The original track is not editable; retained draft kept".into());
            }
            let base = if attempted.is_new {
                NoteValues {
                    pitch: 60,
                    start_tick: 0,
                    duration_ticks: 960,
                    velocity16: 49152,
                }
            } else {
                values(
                    note(doc, &attempted.selection)
                        .ok_or("The original note is absent; retained draft kept")?,
                )
                .ok_or("The original note has no editable values")?
            };
            let mut replacement = NoteDraft::new(
                attempted.selection.clone(),
                doc.revision(),
                base,
                attempted.is_new,
            );
            replacement.pitch = attempted.pitch;
            replacement.start = attempted.start;
            replacement.duration = attempted.duration;
            replacement.velocity = attempted.velocity;
            replacement.extend_loop = attempted.extend_loop;
            replacement.allow_upgrade = attempted.allow_upgrade;
            new_draft = Some(replacement);
        }
        self.failure = None;
        self.released = Some(old);
        if let Some(draft) = &new_draft {
            self.selection = Some(draft.selection.clone());
        }
        self.draft = new_draft;
        self.notice=Some("Rejected edit released. Review these local values; only a new explicit Save submits a new operation.".into());
        self.changed();
        Ok(())
    }
    pub fn release_rejected(&mut self, doc: &ProjectDocument) -> Result<(), String> {
        let failure = self.failure.as_ref().ok_or("No retained note edit")?;
        if !failure.known_rejected
            || !failure.suspended
            || failure.intent.project_id.0 != doc.id()
            || failure.reviewed_revision != Some(doc.revision())
        {
            return Err("Review the latest saved target before releasing a rejected choice".into());
        }
        self.released = self.failure.take();
        self.draft = None;
        self.changed();
        self.notice=Some("Rejected note choice released; its attempted values remain below. Choose a target and make a new explicit edit when ready.".into());
        Ok(())
    }
    pub fn succeeded(&mut self) {
        self.failure = None;
        self.notice = None;
    }
    pub fn failure_ui(
        &self,
        ui: &mut egui::Ui,
        available: bool,
        current: Option<&ProjectDocument>,
    ) -> Option<NoteAction> {
        let mut action = None;
        if let Some(failure) = &self.failure {
            ui.group(|ui|{
            ui.heading(if failure.known_rejected{"Note edit was not published"}else{"Note edit outcome is uncertain"});
            ui.label(&failure.message);ui.small(format!("Retained operation {} · target {} · expected revision {}",failure.intent.operation_id.0,display_key(&failure.intent.project_id.0),failure.intent.expected_revision));
            if let Some(draft)=&failure.draft{ui.small(format!("Retained local values: pitch {} · tick {} · length {} · velocity {}",draft.pitch,draft.start,draft.duration,draft.velocity));}
            ui.small("Closing keeps no durable draft inbox and does not cancel a possibly completed edit. Reconnect never retries this edit automatically.");
            ui.horizontal_wrapped(|ui|{
                let target_open=current.is_some_and(|d|d.id()==failure.intent.project_id.0);
                if ui.add_enabled(available&&target_open,egui::Button::new("Retry exact note edit")).clicked(){action=Some(NoteAction::Retry);}
                if ui.add_enabled(available&&!failure.suspended,egui::Button::new("Keep draft locally and inspect saved session")).clicked(){action=Some(NoteAction::Suspend);}
                if failure.known_rejected {
                    if ui.add_enabled(available,egui::Button::new("Review latest saved note session")).clicked(){action=Some(NoteAction::ReviewLatest);}
                    if ui.add_enabled(available&&current.is_some_and(|d|target_open&&failure.reviewed_revision==Some(d.revision())),egui::Button::new("Make a new note choice")).clicked(){action=Some(NoteAction::NewChoice);}
                    if ui.add_enabled(available&&current.is_some_and(|d|target_open&&failure.reviewed_revision==Some(d.revision())),egui::Button::new("Release rejected note choice")).clicked(){action=Some(NoteAction::ReleaseRejected);}
                }
            });
            if failure.suspended{ui.small("Local evidence retained here; session navigation is available. No new note mutation is allowed until this intent is resolved.");}
        });
        }
        if let Some(released) = &self.released {
            ui.small(format!(
                "Earlier rejected choice retained: {} · no automatic replay",
                released.intent.operation_id.0
            ));
            if let Some(draft) = &released.draft {
                ui.small(format!(
                    "Rejected local values: pitch {} · tick {} · length {} · velocity {}",
                    draft.pitch, draft.start, draft.duration, draft.velocity
                ));
            }
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        action
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &ProjectDocument,
        track_id: &str,
        available: bool,
        metadata_dirty: bool,
    ) -> Option<NoteAction> {
        let mut action = None;
        let selected_track = track(doc, track_id)?;
        if selected_track["kind"] != "notes" {
            return None;
        }
        let unlocked = selected_track["locked"] == false;
        let editable = available && unlocked && !metadata_dirty && !self.unresolved();
        ui.heading("Note editor");
        ui.small("Exact saved notes · integer ticks and 16-bit velocity · no audio preview");
        if !unlocked {
            ui.label("This track is locked; note values are read-only.");
        }
        if metadata_dirty {
            ui.label("Session details have unsaved edits. Resolve them before editing a note.");
            if ui
                .add_enabled(
                    available && !self.unresolved(),
                    egui::Button::new("Edit selected note"),
                )
                .clicked()
            {
                action = Some(NoteAction::BeginNote);
            }
        }
        if ui
            .add_enabled(
                available && unlocked && !self.unresolved(),
                egui::Button::new("Add note"),
            )
            .clicked()
        {
            action = Some(NoteAction::Add(NoteSelection {
                project_id: doc.id().into(),
                address: DocumentNoteAddress {
                    track_id: track_id.into(),
                    note_id: uuid::Uuid::new_v4().to_string(),
                },
            }));
        }
        let matches = self
            .selection
            .as_ref()
            .is_some_and(|s| s.project_id == doc.id() && s.address.track_id == track_id);
        if !matches {
            ui.label("Select an individual saved note in the list or roll.");
            return action;
        }
        let selection = self.selection.as_ref().expect("matched").clone();
        ui.small(format!(
            "Track key: {} · note key: {}",
            display_key(track_id),
            display_key(&selection.address.note_id)
        ));
        let saved = note(doc, &selection);
        if let Some(saved) = saved {
            source_details(ui, saved);
        } else if !self.draft.as_ref().is_some_and(|d| d.is_new) {
            ui.label("The selected note is absent from this saved revision. Undo may restore its exact identity.");
            return action;
        }
        let Some(draft) = &mut self.draft else {
            return action;
        };
        let before = draft.clone();
        ui.small(format!(
            "Draft based on saved revision {}",
            draft.base_revision
        ));
        ui.add_enabled_ui(editable, |ui| {
            for (label, raw) in [
                ("Pitch (0–127)", &mut draft.pitch),
                ("Start (ticks)", &mut draft.start),
                ("Length (ticks)", &mut draft.duration),
                ("Velocity (0–65535)", &mut draft.velocity),
            ] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.add(
                        egui::TextEdit::singleline(raw)
                            .char_limit(20)
                            .desired_width(140.0),
                    )
                    .accessible_name(label);
                });
            }
            ui.radio_value(&mut draft.extend_loop, false, "Keep current playback loop");
            ui.radio_value(&mut draft.extend_loop, true, "Extend song to include note");
            if draft.extend_loop {
                ui.checkbox(
                    &mut draft.allow_upgrade,
                    "Allow native format upgrade beyond 128 bars",
                );
            }
        });
        let valid = draft.values();
        match &valid {
            Ok(values) => {
                ui.label(format!(
                    "{} · pitch {} · end {} ticks",
                    pitch_name(values.pitch),
                    values.pitch,
                    values.start_tick + values.duration_ticks
                ));
                if values.start_tick + values.duration_ticks > doc.bars() * 3840 {
                    ui.label("This note extends outside the current playback loop. Keep loop preserves those saved values without extending playback.");
                }
            }
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
        ui.small(if draft.is_new {
            "New local note · not saved or attributed to an input performance"
        } else {
            "The roll shows saved values until this edit is acknowledged."
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    editable && draft.dirty() && valid.is_ok(),
                    egui::Button::new("Save note"),
                )
                .clicked()
            {
                action = Some(NoteAction::Save);
            }
            if ui
                .add_enabled(
                    available && draft.dirty() && !self.failure.is_some(),
                    egui::Button::new("Discard note"),
                )
                .clicked()
            {
                action = Some(NoteAction::Discard);
            }
            if ui
                .add_enabled(
                    available && unlocked && saved.is_some() && !self.failure.is_some(),
                    egui::Button::new("Delete selected note"),
                )
                .clicked()
            {
                action = Some(NoteAction::Delete(selection.clone()));
            }
            if ui
                .add_enabled(
                    available
                        && unlocked
                        && saved.is_some_and(|n| n.get("editSource").is_some())
                        && !self.failure.is_some(),
                    egui::Button::new("Restore original"),
                )
                .clicked()
            {
                action = Some(NoteAction::Restore(selection.clone(), draft.timeline()));
            }
        });
        if *draft != before {
            self.changed();
        }
        action
    }
}

pub(super) fn display_key(raw: &str) -> String {
    let mut result = String::new();
    for ch in raw.chars().take(72) {
        for escaped in ch.escape_debug() {
            result.push(escaped);
        }
    }
    if raw.chars().nth(72).is_some() {
        result.push('…');
    }
    if result.is_empty() {
        "\"\"".into()
    } else {
        result
    }
}
fn pitch_name(pitch: u8) -> String {
    format!(
        "{}{}",
        [
            "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"
        ][usize::from(pitch % 12)],
        i16::from(pitch) / 12 - 1
    )
}
fn scalar(value: &Value) -> String {
    match value {
        Value::String(v) => display_key(v),
        Value::Number(n) => {
            let raw = n.as_str();
            if raw.len() > 80 {
                format!("{}…", &raw[..80])
            } else {
                raw.into()
            }
        }
        Value::Bool(v) => v.to_string(),
        Value::Null => "null".into(),
        _ => "structured metadata retained".into(),
    }
}
fn expression_summary(ui: &mut egui::Ui, value: Option<&Value>) {
    let Some(value) = value else {
        ui.small("Expression: absent in original representation");
        return;
    };
    let Some(lanes) = value.as_array() else {
        ui.small("Expression: explicit null retained");
        return;
    };
    ui.small(format!("Expression: {} lane(s) retained", lanes.len()));
    let mut shown = 0;
    let mut total = 0usize;
    for lane in lanes {
        total = total.saturating_add(lane["points"].as_array().map_or(0, Vec::len));
    }
    for lane in lanes.iter().take(8) {
        ui.small(format!(
            "Lane {} · range {} · estimated {}",
            scalar(&lane["type"]),
            scalar(&lane["rangeSemitones"]),
            scalar(&lane["estimated"])
        ));
        if let Some(points) = lane["points"].as_array() {
            for point in points.iter().take(64 - shown) {
                ui.small(format!(
                    "tick {} · value32 {}",
                    scalar(&point["tick"]),
                    scalar(&point["value32"])
                ));
                shown += 1;
            }
        }
    }
    if lanes.len() > 8 || total > shown {
        ui.small(format!("Display bounded to 8 lanes / 64 points: {shown} of {total} points shown. Omitted data remains saved; it is not compared or rewritten here."));
    }
}
pub(super) fn source_details(ui: &mut egui::Ui, note: &Value) {
    if note.get("editSource").is_some() {
        ui.small("Original retained");
    }
    ui.collapsing("Original and expression details",|ui|{
        if let Some(input)=note.get("input"){
            for field in ["sourceId","sourceName","protocol","group","channel","releaseVelocity16","keyDurationTicks"]{if let Some(value)=input.get(field){ui.small(format!("{field}: {}",scalar(value)));}}
        }
        expression_summary(ui,note.get("expression"));
        if let Some(source)=note.get("editSource"){
            ui.label("Original retained");
            for field in ["pitch","startTick","durationTicks","velocity16"]{ui.small(format!("Original {field}: {}",scalar(&source[field])));}
            ui.small("Snapshot presence does not mean the current values differ. Unfamiliar snapshot fields remain retained.");
            ui.small(match source.get("expression"){
                None=>"Original expression representation: absent".into(),
                Some(Value::Null)=>"Original expression representation: explicit null".into(),
                Some(Value::Array(lanes))=>format!("Original expression representation: {} lane(s), retained without a second expanded point display",lanes.len()),
                _=>"Original expression representation retained".into(),
            });
        }
        ui.small("Raw session input and model metadata remain retained. No packet-to-note linkage is inferred by pitch or tick.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (ProjectDocument, NoteSelection) {
        let mut value = ProjectDocument::new("Synthetic editor")
            .unwrap()
            .value()
            .clone();
        value["tracks"][0]["id"] = json!("鍵盤\0");
        value["tracks"][0]["notes"] = json!([
            {"id":"","pitch":60,"startTick":37,"durationTicks":960,"velocity16":65535,"expression":null},
            {"id":"same pitch","pitch":60,"startTick":37,"durationTicks":960,"velocity16":0,"expression":[]}
        ]);
        let doc = ProjectDocument::from_value(value).unwrap();
        let selection = NoteSelection {
            project_id: doc.id().into(),
            address: DocumentNoteAddress {
                track_id: "鍵盤\0".into(),
                note_id: "".into(),
            },
        };
        (doc, selection)
    }
    #[test]
    fn four_integer_fields_preserve_invalid_intermediates_and_velocity_zero() {
        let (doc, selection) = fixture();
        let mut editor = NoteEditor::default();
        editor.select(&doc, selection).unwrap();
        let draft = editor.draft.as_mut().unwrap();
        draft.pitch = "127".into();
        draft.start = "1000000000".into();
        draft.duration = "100000000".into();
        draft.velocity = "0".into();
        assert_eq!(
            draft.values().unwrap(),
            NoteValues {
                pitch: 127,
                start_tick: 1_000_000_000,
                duration_ticks: 100_000_000,
                velocity16: 0
            }
        );
        for invalid in ["", "-1", "1.5", " 1", "65536", "18446744073709551616"] {
            draft.velocity = invalid.into();
            assert!(draft.values().is_err());
            assert_eq!(draft.velocity, invalid);
        }
        draft.velocity = "65535".into();
        draft.pitch = "128".into();
        assert!(draft.values().is_err());
        draft.pitch = "60".into();
        draft.duration = "0".into();
        assert!(draft.values().is_err());
    }
    #[test]
    fn exact_legacy_identity_survives_reorder_delete_and_undo_without_neighbour_selection() {
        let (doc, selection) = fixture();
        let mut editor = NoteEditor::default();
        editor.select(&doc, selection.clone()).unwrap();
        let mut reordered = doc.value().clone();
        reordered["tracks"][0]["notes"]
            .as_array_mut()
            .unwrap()
            .reverse();
        let reordered = ProjectDocument::from_value(reordered).unwrap();
        editor.document_installed(&reordered);
        assert_eq!(editor.selection, Some(selection.clone()));
        assert_eq!(editor.draft.as_ref().unwrap().velocity, "65535");
        let deleted = reordered.delete_note(&selection.address).unwrap();
        editor.document_installed(&deleted);
        assert_eq!(editor.selection, Some(selection.clone()));
        assert!(editor.draft.is_none());
        editor.document_installed(&doc);
        assert_eq!(editor.draft.as_ref().unwrap().velocity, "65535");
        assert_eq!(display_key(""), "\"\"");
        assert!(display_key("鍵盤\0").contains("\\0"));
    }
    #[test]
    fn retry_enqueue_rejection_cannot_downgrade_uncertainty_or_replace_attempted_values() {
        let (doc, selection) = fixture();
        let mut editor = NoteEditor::default();
        editor.select(&doc, selection).unwrap();
        editor.draft.as_mut().unwrap().velocity = "0".into();
        let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
        let attempted = editor.draft.clone();
        editor.failed(intent.clone(), "lost reply".into(), false);
        editor.draft.as_mut().unwrap().velocity = "123".into();
        editor.not_admitted(intent.clone(), "full".into());
        let failure = editor.failure.as_ref().unwrap();
        assert!(!failure.known_rejected);
        assert_eq!(failure.draft, attempted);
        assert_eq!(failure.intent, intent);
        editor.suspend();
        assert!(!editor.dirty());
        assert_eq!(editor.failure.as_ref().unwrap().draft, attempted);
        assert!(
            editor
                .intent(&doc, &NoteAction::Delete(editor.selection.clone().unwrap()))
                .is_err()
        );
    }
    #[test]
    fn reviewing_deleted_target_keeps_rejected_intent_and_draft_until_explicit_resolution() {
        let (doc, selection) = fixture();
        let mut editor = NoteEditor::default();
        editor.select(&doc, selection.clone()).unwrap();
        editor.draft.as_mut().unwrap().pitch = "61".into();
        let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
        editor.failed(intent.clone(), "conflict".into(), true);
        let attempted = editor.draft.clone();
        editor.suspend();
        let deleted = doc.delete_note(&selection.address).unwrap();
        editor.document_installed(&deleted);
        assert!(editor.new_choice(&deleted).is_err());
        let failure = editor.failure.as_ref().unwrap();
        assert_eq!(failure.intent, intent);
        assert_eq!(failure.draft, attempted);
        editor.document_installed(&doc);
        editor.new_choice(&doc).unwrap();
        assert!(editor.failure.is_none());
        assert_eq!(editor.draft.as_ref().unwrap().pitch, "61");
        assert_ne!(
            editor.intent(&doc, &NoteAction::Save).unwrap().operation_id,
            intent.operation_id
        );
    }
    #[test]
    fn large_expression_display_is_bounded_and_never_serializes_opaque_source() {
        let points: Vec<_> = (0..1000)
            .map(|tick| json!({"tick":tick,"value32":tick,"opaque":"must not display"}))
            .collect();
        let lanes: Vec<_> = (0..20)
            .map(|_| json!({"type":"pitch","points":points}))
            .collect();
        let expression = json!(lanes);
        let before = expression.clone();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 4000.0),
                )),
                ..Default::default()
            },
            |ui| expression_summary(ui, Some(&expression)),
        );
        output.textures_delta.clear();
        let tree = output.platform_output.accesskit_update.unwrap();
        let all_text: Vec<_> = tree
            .nodes
            .iter()
            .filter_map(|(_, node)| node.label().or_else(|| node.value()))
            .collect();
        // TextRun children repeat their parent Label's text for selection.
        // Count semantic labels, while checking every node for opaque leakage.
        let labels: Vec<_> = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.role() == egui::accesskit::Role::Label)
            .filter_map(|(_, node)| node.label().or_else(|| node.value()))
            .collect();
        assert_eq!(
            labels
                .iter()
                .filter(|text| text.starts_with("tick "))
                .count(),
            64
        );
        assert_eq!(
            labels
                .iter()
                .filter(|text| text.starts_with("Lane "))
                .count(),
            8
        );
        assert!(
            labels
                .iter()
                .any(|text| text.contains("64 of 20000 points shown"))
        );
        assert!(
            !all_text
                .iter()
                .any(|text| text.contains("must not display"))
        );
        assert_eq!(expression, before);
    }
    #[test]
    fn authored_draft_discard_is_local_and_locked_non_note_tracks_cannot_submit() {
        let (doc, mut selection) = fixture();
        let mut editor = NoteEditor::default();
        selection.address.note_id = uuid::Uuid::new_v4().to_string();
        editor.add(&doc, selection.clone()).unwrap();
        assert!(editor.dirty());
        let intent = editor.intent(&doc, &NoteAction::Save).unwrap();
        assert!(matches!(
            intent.edit,
            EditOperation::AddNote {
                timeline: NoteTimelinePolicy::PreserveLoop,
                ..
            }
        ));
        editor.discard(&doc);
        assert!(editor.selection.is_none());
        assert!(!editor.dirty());
        let mut locked = doc.value().clone();
        locked["tracks"][0]["locked"] = json!(true);
        let locked = ProjectDocument::from_value(locked).unwrap();
        assert!(editor.add(&locked, selection.clone()).is_err());
        let mut audio = doc.value().clone();
        audio["tracks"][0]["kind"] = json!("audio");
        let audio = ProjectDocument::from_value(audio).unwrap();
        assert!(editor.add(&audio, selection).is_err());
    }
    #[test]
    fn loop_policy_and_native_upgrade_are_explicit_and_retained_in_the_request() {
        let (doc, selection) = fixture();
        let mut editor = NoteEditor::default();
        editor.select(&doc, selection).unwrap();
        let draft = editor.draft.as_mut().unwrap();
        draft.start = "500000".into();
        draft.duration = "960".into();
        let keep = editor.intent(&doc, &NoteAction::Save).unwrap();
        assert!(matches!(
            keep.edit,
            EditOperation::UpdateNote {
                timeline: NoteTimelinePolicy::PreserveLoop,
                ..
            }
        ));
        editor.draft.as_mut().unwrap().extend_loop = true;
        let extend = editor.intent(&doc, &NoteAction::Save).unwrap();
        assert!(matches!(
            extend.edit,
            EditOperation::UpdateNote {
                timeline: NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: false
                },
                ..
            }
        ));
        editor.draft.as_mut().unwrap().allow_upgrade = true;
        let upgrade = editor.intent(&doc, &NoteAction::Save).unwrap();
        assert!(matches!(
            upgrade.edit,
            EditOperation::UpdateNote {
                timeline: NoteTimelinePolicy::ExtendLoop {
                    allow_native_schema_upgrade: true
                },
                ..
            }
        ));
    }
}
