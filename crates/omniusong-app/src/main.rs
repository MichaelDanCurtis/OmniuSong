#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_ui;
mod recovery_ui;

use audio_ui::AudioUi;
use eframe::egui;
use omniusong_core::ProjectDocument;
use omniusong_core::contracts::{
    EditOperation, EditScope, OperationId, ProjectEditIntent, ProjectId,
};
use omniusong_core::native::{RecordingPayload, TakeRange};
use omniusong_session::{
    ArchiveInspection, Command, Completion, DocumentView, FailureKind, LegacyProject, Library,
    Reply, RequestId, RestoreCopyIntent, SessionService,
};
use recovery_ui::{RecoveryAction, RecoveryUi};
use serde_json::Value;
use std::{
    error::Error,
    path::{Path, PathBuf},
    time::Duration,
};

type AppResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone)]
struct Paths {
    native: PathBuf,
    legacy: PathBuf,
}

impl Paths {
    fn parse(args: impl IntoIterator<Item = String>) -> AppResult<Option<Self>> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let mut paths = Self {
            native: workspace.join(".runtime/native-studio"),
            legacy: workspace.join("data"),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    paths.native = args.next().ok_or("--data-dir requires a path")?.into()
                }
                "--legacy-dir" => {
                    paths.legacy = args.next().ok_or("--legacy-dir requires a path")?.into()
                }
                "--help" | "-h" => {
                    println!(
                        "OmniuSong native studio\n  --data-dir PATH    Native session store\n  --legacy-dir PATH  Read-only legacy data directory\nNo server is required. Audio playback and explicit solo capture use a separate native process."
                    );
                    return Ok(None);
                }
                _ => return Err(format!("Unknown argument: {arg}").into()),
            }
        }
        Ok(Some(paths))
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Draft {
    name: String,
    tempo: String,
}

impl Draft {
    fn from_document(document: &ProjectDocument) -> Self {
        Self {
            name: document.name().into(),
            tempo: document.tempo().to_string(),
        }
    }
    fn is_dirty(&self, document: &ProjectDocument) -> bool {
        self.name != document.name() || self.tempo != document.tempo().to_string()
    }
    fn values(&self) -> AppResult<(String, f64)> {
        if self.name.trim().is_empty() || self.name.chars().count() > 160 {
            return Err("Session name must contain 1–160 characters".into());
        }
        let tempo = self
            .tempo
            .parse::<f64>()
            .map_err(|_| "Tempo must be a number from 30 to 300")?;
        if !tempo.is_finite() || !(30.0..=300.0).contains(&tempo) {
            return Err("Tempo must be a number from 30 to 300".into());
        }
        Ok((self.name.clone(), tempo))
    }
}

#[derive(Clone)]
enum Action {
    Create,
    Open(String),
    Import(PathBuf),
    Save,
    Undo,
    Redo,
    Refresh,
    Export(PathBuf),
    Inspect(PathBuf),
    Restore {
        path: PathBuf,
        intent: RestoreCopyIntent,
    },
    AttachCapture {
        journal_path: PathBuf,
        publication: Box<omniusong_store::TakePublication>,
    },
    Recovery(RecoveryAction),
    Close,
}
#[derive(Clone)]
struct Active {
    id: RequestId,
    command: Command,
    action: Action,
    context: Option<(String, u64)>,
    draft: Draft,
    after: Option<Action>,
}
struct InspectedBackup {
    path: PathBuf,
    inspection: ArchiveInspection,
}
struct StudioApp {
    paths: Paths,
    service: Option<SessionService>,
    ready: bool,
    reconnect_existing: bool,
    active: Option<Active>,
    retry: Option<Active>,
    sessions: Vec<omniusong_session::ProjectSummary>,
    legacy: Vec<LegacyProject>,
    legacy_notice: Option<String>,
    legacy_selection: usize,
    document: Option<ProjectDocument>,
    recordings: Vec<Option<RecordingPayload>>,
    provenance: Vec<Option<String>>,
    draft: Draft,
    new_name: String,
    selected_track: usize,
    selected_note: Option<usize>,
    pending: Option<Action>,
    status: String,
    error: Option<String>,
    closing: bool,
    roll_needs_center: bool,
    export_path: String,
    restore_path: String,
    inspected: Option<InspectedBackup>,
    audio: AudioUi,
    audio_executable: Option<PathBuf>,
    audio_close_requested: bool,
    recovery: RecoveryUi,
    recovery_close_requested: bool,
}
impl StudioApp {
    // Construction is pure. The first rendered loading frame starts the worker.
    fn new(paths: Paths) -> Self {
        Self {
            paths,
            service: None,
            ready: false,
            reconnect_existing: false,
            active: None,
            retry: None,
            sessions: vec![],
            legacy: vec![],
            legacy_notice: None,
            legacy_selection: 0,
            document: None,
            recordings: vec![],
            provenance: vec![],
            draft: Draft::default(),
            new_name: "My native session".into(),
            selected_track: 0,
            selected_note: None,
            pending: None,
            status: "Opening your local studio.".into(),
            error: None,
            closing: false,
            roll_needs_center: true,
            export_path: String::new(),
            restore_path: String::new(),
            inspected: None,
            audio: AudioUi::new(),
            audio_executable: None,
            audio_close_requested: false,
            recovery: RecoveryUi::new(),
            recovery_close_requested: false,
        }
    }
    fn with_audio_executable(mut self, executable: PathBuf) -> Self {
        self.audio_executable = Some(executable);
        self
    }
    fn context(&self) -> Option<(String, u64)> {
        self.document
            .as_ref()
            .map(|doc| (doc.id().into(), doc.revision()))
    }
    fn busy(&self) -> bool {
        self.service.as_ref().is_some_and(SessionService::is_busy)
    }
    fn disconnected(&self) -> bool {
        self.service
            .as_ref()
            .is_some_and(SessionService::is_disconnected)
    }
    fn dirty(&self) -> bool {
        self.document
            .as_ref()
            .is_some_and(|doc| self.draft.is_dirty(doc))
    }
    fn install_document(&mut self, view: DocumentView) {
        self.draft = Draft::from_document(&view.document);
        self.document = Some(view.document);
        self.recordings = view.recordings;
        self.provenance = view.provenance;
        self.selected_track = 0;
        self.selected_note = None;
        self.roll_needs_center = true;
    }
    fn library(&mut self, library: Library) {
        self.sessions = library.sessions;
        self.legacy = library.legacy.projects;
        self.legacy_notice = library.legacy_notice;
    }
    fn start(&mut self) {
        self.start_service(false);
    }
    fn start_service(&mut self, require_existing: bool) {
        self.ready = false;
        let paths = omniusong_session::Paths {
            native: self.paths.native.clone(),
            legacy: self.paths.legacy.clone(),
        };
        let spawned = if require_existing {
            SessionService::reconnect(paths)
        } else {
            SessionService::spawn(paths)
        };
        match spawned {
            Ok(service) => {
                self.service = Some(service);
                if let Err(error) = self.submit(Command::Initialize, Action::Refresh, None) {
                    self.error = Some(error.to_string());
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
    fn submit(&mut self, command: Command, action: Action, after: Option<Action>) -> AppResult<()> {
        let context = self.context();
        let draft = self.draft.clone();
        let service = self
            .service
            .as_mut()
            .ok_or("Session worker has not started")?;
        match service.try_submit(command) {
            Ok(id) => {
                let command = service.pending().expect("accepted request").command.clone();
                if let Action::Recovery(recovery) = &action {
                    self.recovery.started(recovery);
                }
                self.active = Some(Active {
                    id,
                    command,
                    action,
                    context,
                    draft,
                    after,
                });
                self.error = None;
                self.status =
                    "Work is running in the background. If the app closes, it may already have saved; reopening lets you inspect the result."
                        .into();
                Ok(())
            }
            Err(error) => {
                if let Action::Recovery(recovery) = &action {
                    self.recovery.failed(recovery);
                }
                if error.command.retry_safe() {
                    self.retry = Some(Active {
                        id: RequestId {
                            generation: service.generation(),
                            sequence: 0,
                        },
                        command: error.command,
                        action,
                        context,
                        draft,
                        after,
                    });
                }
                Err(error.message.into())
            }
        }
    }
    fn poll(&mut self) {
        let Some(service) = self.service.as_mut() else {
            return;
        };
        if service.is_disconnected() {
            return;
        }
        match service.try_poll() {
            Ok(Some(completion)) => self.complete(completion),
            Ok(None) => {}
            Err(error) => {
                if error.kind == FailureKind::Disconnected
                    && let Some(active) = self.active.take()
                    && active.command.retry_safe()
                    && !(matches!(active.command, Command::Initialize) && self.retry.is_some())
                {
                    if let Action::Recovery(recovery) = &active.action {
                        self.recovery.failed(recovery);
                    }
                    if matches!(active.action, Action::AttachCapture { .. }) {
                        self.audio.publication_failed();
                    }
                    self.retry = Some(active);
                }
                if error.kind == FailureKind::Disconnected {
                    self.status="Storage worker stopped. Your draft was kept; reconnect explicitly before another action.".into();
                }
                self.error = Some(error.to_string());
            }
        }
    }
    fn complete(&mut self, completion: Completion) {
        let Some(active) = self.active.take() else {
            self.error = Some("Ignored a completion without an active action".into());
            return;
        };
        if active.id != completion.id {
            self.active = Some(active);
            self.error = Some("Ignored a stale session completion".into());
            return;
        }
        match completion.result {
            Err(error) => {
                if let Action::Recovery(recovery) = &active.action {
                    self.recovery.failed(recovery);
                    if error.kind == FailureKind::RevisionConflict && !error.outcome_unknown {
                        self.recovery.revision_conflict(recovery);
                    }
                }
                if matches!(active.action, Action::AttachCapture { .. }) {
                    self.audio.publication_failed();
                }
                if error.kind == FailureKind::StorageUnavailable {
                    self.reconnect_existing = true;
                    self.status="Storage worker stopped. Your draft was kept; reconnect explicitly before another action.".into();
                } else {
                    self.status = if error.outcome_unknown {
                        "Action ended without a confirmed result. Inspect or explicitly retry the retained action."
                    } else {
                        "Action finished with an error. Your saved session and local draft were kept."
                    }.into();
                }
                if active.command.retry_safe()
                    && !(matches!(active.command, Command::Initialize) && self.retry.is_some())
                {
                    self.retry = Some(active);
                } else if !matches!(active.command, Command::Initialize) {
                    self.retry = None;
                }
                self.error = Some(format!(
                    "{}{}",
                    error.message,
                    if error.outcome_unknown {
                        " The action may have completed. Retry the same saved action when available, or refresh sessions / inspect the backup before starting another."
                    } else {
                        ""
                    }
                ));
            }
            Ok(reply) => {
                let mut after = None;
                match reply {
                    Reply::Library(library) => {
                        self.library(library);
                        self.ready = true;
                        self.reconnect_existing = true;
                        self.status = "Choose a session or create one.".into();
                    }
                    Reply::Document {
                        view,
                        library,
                        replayed,
                    } => {
                        self.library(library);
                        if self.context() != active.context || self.draft != active.draft {
                            if let Action::Recovery(recovery)=&active.action {self.recovery.failed(recovery);}
                            if matches!(active.action, Action::AttachCapture { .. }) {
                                self.audio.publication_failed();
                            }
                            self.status="Action completed in storage; the newer local session/draft was kept.".into();
                            self.error=Some("The session or draft changed while this action ran. Its completion was retained in storage; your current draft was kept.".into());
                            return;
                        }
                        self.status = if replayed {
                            "Action already completed; refreshed the latest saved session.".into()
                        } else {
                            format!(
                                "Opened {} · native revision {}",
                                view.document.name(),
                                view.document.revision()
                            )
                        };
                        self.install_document(view);
                        if matches!(active.action, Action::AttachCapture { .. }) {
                            self.audio.publication_succeeded();
                        }
                        if matches!(active.action,Action::Recovery(RecoveryAction::Attach {..})) {self.recovery.attached();}
                        after = active.after;
                    }
                    Reply::Exported(receipt) => {
                        self.status = format!(
                            "Backup created: {}. Originals and source session were preserved.",
                            receipt.path.display()
                        )
                    }
                    Reply::Inspected(inspection) => {
                        if let Command::InspectArchive { path } = active.command {
                            if self.restore_path == path.to_string_lossy() {
                                self.inspected = Some(InspectedBackup { path, inspection });
                                self.status="Backup fingerprint and summary checked. Full validation happens when restoring a new session.".into();
                            } else {
                                self.error = Some(
                                    "Backup path changed; inspect the selected path again.".into(),
                                );
                            }
                        }
                    }
                    Reply::CaptureStages(scan)=>{self.recovery.scanned(scan);self.status="Retained stages scanned. Inspect a source before choosing recovery or attachment.".into();}
                    Reply::CaptureInspected(inspection)=>{
                        match self.recovery.inspected(inspection) {Ok(())=>self.status="Retained original inspected without changing its bytes. Review the verified prefix and unknown tail.".into(),Err(error)=>{self.status="Inspection finished; the retained selection was kept.".into();self.error=Some(error);}}
                    }
                    Reply::CaptureRecovered(receipt)=>{
                        match self.recovery.recovered(receipt) {Ok(())=>self.status="Verified prefix sealed and original rows retained. Choose an audio stream and attach explicitly; recovery did not attach audio to the session.".into(),Err(error)=>{self.status="Recovery finished without a matching displayed receipt; no attachment was started.".into();self.error=Some(error);}}
                    }
                }
                if !matches!(active.action, Action::Refresh) {
                    self.retry = None;
                }
                if let Some(action) = after {
                    self.execute(action);
                }
            }
        }
    }
    fn save(&mut self) -> AppResult<()> {
        self.dispatch(Action::Save, None)
    }
    fn request(&mut self, action: Action) {
        if self.audio.blocks_session() {
            if matches!(action, Action::Close) {
                self.audio_close_requested = true;
            } else {
                self.error = Some("Finish or stop audio, and resolve any retained sealed take, before changing this session.".into());
            }
            return;
        }
        if self.recovery.blocks_session()
            && !matches!(&action,Action::Recovery(recovery)if recovery.read_only()||matches!(recovery,RecoveryAction::Attach {..}))
        {
            if matches!(action, Action::Close) {
                self.recovery_close_requested = true;
            } else {
                self.error = Some(
                    "Resolve the retained capture attachment before changing this session.".into(),
                );
            }
            return;
        }
        if self.busy() {
            if matches!(action, Action::Close) {
                self.closing = true;
            }
            return;
        }
        if self.dirty()
            && !matches!(action, Action::Save | Action::Refresh)
            && !matches!(&action,Action::Recovery(recovery)if recovery.read_only())
        {
            self.pending = Some(action);
        } else {
            self.execute(action);
        }
    }
    fn execute(&mut self, action: Action) {
        if let Err(error) = self.dispatch(action, None) {
            self.error = Some(error.to_string());
        }
    }
    fn dispatch(&mut self, action: Action, after: Option<Action>) -> AppResult<()> {
        if self.audio.blocks_session() && !matches!(action, Action::AttachCapture { .. }) {
            return Err(
                "Finish or stop audio and resolve the retained original before this action".into(),
            );
        }
        if self.busy() {
            return Err("Another action is still running".into());
        }
        if self.recovery.blocks_session()
            && !matches!(&action,Action::Recovery(recovery)if recovery.read_only()||matches!(recovery,RecoveryAction::Attach {..}))
        {
            return Err("Resolve the retained capture attachment before this action".into());
        }
        let action = if let Action::Recovery(RecoveryAction::ChooseAttachment(choice)) = action {
            let document = self
                .document
                .as_ref()
                .ok_or("Open a session before attaching a retained original")?;
            Action::Recovery(self.recovery.prepare_attachment(
                &choice,
                document.id(),
                document.revision(),
            )?)
        } else {
            action
        };
        if let Action::Recovery(RecoveryAction::Attach { publication, .. }) = &action {
            if self.document.as_ref().map(ProjectDocument::id)
                != Some(publication.project_id.0.as_str())
            {
                return Err(
                    "Reopen the original target session before retrying this exact attachment"
                        .into(),
                );
            }
            if self.dirty() {
                return Err("Save or discard local edits before retrying this exact attachment; its expected revision will not change".into());
            }
        }
        let command = match &action {
            Action::Create => Command::Create {
                name: self.new_name.clone(),
            },
            Action::Open(id) => Command::Open {
                project_id: ProjectId(id.clone()),
            },
            Action::Import(path) => Command::ImportLegacy { path: path.clone() },
            Action::Refresh => Command::Refresh,
            Action::Save | Action::Undo | Action::Redo => {
                let document = self.document.as_ref().ok_or("No open session")?;
                let edit = match &action {
                    Action::Save => {
                        let (name, tempo) = self.draft.values()?;
                        EditOperation::UpdateSession { name, tempo }
                    }
                    Action::Undo => EditOperation::Undo,
                    _ => EditOperation::Redo,
                };
                Command::Edit {
                    intent: ProjectEditIntent {
                        operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
                        project_id: ProjectId(document.id().into()),
                        expected_revision: document.revision(),
                        scope: EditScope::Project,
                        edit,
                        extensions: Default::default(),
                    },
                }
            }
            Action::Export(path) => Command::ExportArchive {
                project_id: ProjectId(
                    self.document
                        .as_ref()
                        .ok_or("Open a session before creating a backup")?
                        .id()
                        .into(),
                ),
                destination: path.clone(),
            },
            Action::Inspect(path) => Command::InspectArchive { path: path.clone() },
            Action::Restore { path, intent } => Command::RestoreCopy {
                path: path.clone(),
                intent: intent.clone(),
            },
            Action::AttachCapture {
                journal_path,
                publication,
            } => Command::AttachCapture {
                journal_path: journal_path.clone(),
                publication: publication.as_ref().clone(),
            },
            Action::Recovery(recovery) => recovery.command()?,
            Action::Close => {
                self.closing = true;
                return Ok(());
            }
        };
        self.submit(command, action, after)
    }
    fn attach_capture(
        &mut self,
        journal_path: PathBuf,
        publication: omniusong_store::TakePublication,
    ) {
        match self.dispatch(
            Action::AttachCapture {
                journal_path,
                publication: Box::new(publication),
            },
            None,
        ) {
            Ok(()) => self.audio.publication_started(),
            Err(error) => {
                self.audio.publication_failed();
                self.error = Some(error.to_string());
            }
        }
    }
    fn poll_audio(&mut self) {
        self.audio.poll();
        if !self.busy()
            && let Some((path, publication)) = self.audio.take_auto_publication()
        {
            if !self.ready || self.disconnected() {
                self.audio.publication_failed();
                self.error=Some("The sealed original is retained. Reconnect storage, then explicitly retry attaching it.".into());
            } else {
                self.attach_capture(path, publication);
            }
        }
    }
    fn retry_last(&mut self) {
        let Some(active) = self.retry.clone() else {
            return;
        };
        if self.context() != active.context || self.draft != active.draft {
            self.error = Some(
                "Your draft changed. Keep these edits and choose a new save or reopen action."
                    .into(),
            );
            return;
        }
        if let Err(error) = self.submit(active.command, active.action, active.after) {
            self.error = Some(error.to_string());
        }
    }
    fn loading_ui(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show(root,|ui|{
            ui.add_space(45.0);ui.heading(if self.disconnected(){"Session storage is unavailable"}else{"Opening your local studio"});
            ui.label("Session storage and source summaries are handled in the background.");
            ui.label("macOS may ask OmniuSong Native for access to the external drive. A system prompt, if shown, controls access.");
            if let Some(error)=&self.error{ui.colored_label(egui::Color32::LIGHT_RED,error);}
            if !self.busy()&&self.error.is_some()&&ui.button("Try opening storage again").clicked(){
                if self.disconnected()||self.service.is_none(){self.service=None;self.start_service(self.reconnect_existing);}else{self.execute_initialize();}
            }
            ui.label("Work is running in the background. If the app closes, it may already have saved; reopening lets you inspect the result.");
            if ui.button("Close").clicked(){self.request(Action::Close);}
        });
    }
    fn execute_initialize(&mut self) {
        if let Err(error) = self.submit(Command::Initialize, Action::Refresh, None) {
            self.error = Some(error.to_string());
        }
    }
    fn show(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        self.poll();
        self.poll_audio();
        if !self.ready {
            self.loading_ui(root);
            self.audio_close_guard_ui(&ctx);
            self.recovery_close_guard_ui(&ctx);
            if self.service.is_none() && self.error.is_none() && !self.closing {
                self.start();
            }
            if self.busy() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return;
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(executable) = self.audio_executable.take() {
            self.audio.start(executable, self.paths.native.clone());
        }
        if self.audio.needs_poll() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let can_start_audio = !self.busy()
            && !self.disconnected()
            && !self.dirty()
            && self.pending.is_none()
            && !self.recovery.blocks_session();
        let audio_context = self.context();
        let mut retry_attach = false;
        let mut requested = None;
        egui::Panel::top("transport").show(root, |ui| {
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.heading(
                    egui::RichText::new("omniuSong").color(egui::Color32::from_rgb(100, 225, 187)),
                );
                ui.label("NATIVE STUDIO · FOUNDATION");
                ui.separator();
            });
            retry_attach = self.audio.ui(ui, can_start_audio, audio_context);
            if self.dirty() {
                ui.small("Save or discard draft edits before playing or recording.");
            }
            ui.add_space(5.0);
        });
        if retry_attach && let Some((path, publication)) = self.audio.retry_publication() {
            self.attach_capture(path, publication);
        }
        let available = !self.busy()
            && !self.disconnected()
            && !self.audio.blocks_session()
            && !self.recovery.blocks_session();
        egui::Panel::bottom("status").show(root, |ui| {
            if let Some(error) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            ui.label(&self.status);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(available, egui::Button::new("Refresh sessions"))
                    .clicked()
                {
                    requested = Some(Action::Refresh);
                }
                if ui
                    .add_enabled(
                        available && self.retry.is_some(),
                        egui::Button::new("Retry last action"),
                    )
                    .clicked()
                {
                    self.retry_last();
                }
                if !self.busy() && self.disconnected() && ui.button("Reconnect storage").clicked() {
                    self.service = None;
                    self.start_service(self.reconnect_existing);
                }
                if ui.button("Close window").clicked() {
                    requested = Some(Action::Close);
                }
            });
            ui.small(
                "Native audio runs in a separate process. MIDI, plugins and model execution remain pending.",
            );
        });
        let recovery_available =
            !self.busy() && !self.disconnected() && !self.audio.blocks_session();
        let mut recovery_requested = None;
        let recovery_draft_dirty = self.dirty();
        egui::Panel::right("retained-captures")
            .default_size(340.0)
            .min_size(260.0)
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("retained-capture-review")
                    .show(ui, |ui| {
                        recovery_requested = self.recovery.ui(
                            ui,
                            recovery_available,
                            self.document.as_ref().map(ProjectDocument::id),
                            recovery_draft_dirty,
                        );
                    });
            });
        if let Some(recovery) = recovery_requested {
            self.request(Action::Recovery(recovery));
        }
        let available = !self.busy()
            && !self.disconnected()
            && !self.audio.blocks_session()
            && !self.recovery.blocks_session();
        egui::Panel::left("sessions")
            .default_size(270.0)
            .min_size(210.0)
            .show(root, |ui| {
                if !available {ui.disable();}
                egui::ScrollArea::vertical().id_salt("library-sidebar").show(ui,|ui|{
                ui.heading("Your sessions");
                ui.label("New session name");
                ui.add(egui::TextEdit::singleline(&mut self.new_name).char_limit(160).desired_width(f32::INFINITY))
                    .accessible_name("New session name");
                if ui.button("Create session").clicked() {
                    requested = Some(Action::Create);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("native-sessions")
                    .max_height(240.0)
                    .show(ui, |ui| {
                        if self.sessions.is_empty() {
                            ui.label("No native sessions yet.");
                        }
                        for session in &self.sessions {
                            let selected =
                                self.document.as_ref().is_some_and(|d| d.id() == session.id);
                            if ui.selectable_label(selected, &session.name).clicked() {
                                requested = Some(Action::Open(session.id.clone()));
                            }
                            ui.small(format!(
                                "{} tracks · {} notes · r{}",
                                session.track_count, session.note_count, session.revision
                            ));
                            if let Some(problem) = &session.problem {
                                ui.colored_label(
                                    egui::Color32::GOLD,
                                    format!("Needs recovery: {problem}"),
                                );
                            }
                        }
                    });
                ui.separator();
                ui.heading("Bring your work");
                ui.label("Legacy projects are read-only sources.");
                if let Some(notice) = &self.legacy_notice {
                    ui.colored_label(egui::Color32::GOLD, notice);
                }
                if self.legacy.is_empty() {
                    ui.small("No supported projects found in the legacy directory.");
                } else {
                    self.legacy_selection = self.legacy_selection.min(self.legacy.len() - 1);
                    ui.label("Legacy source project");
                    egui::ComboBox::from_id_salt("legacy-project")
                        .selected_text(&self.legacy[self.legacy_selection].name)
                        .show_ui(ui, |ui| {
                            for (index, project) in self.legacy.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.legacy_selection,
                                    index,
                                    &project.name,
                                );
                            }
                        })
                        .response
                        .accessible_name("Legacy source project");
                    ui.small(format!(
                        "{} tracks · separate native copy",
                        self.legacy[self.legacy_selection].tracks
                    ));
                    if ui.button("Import selected project").clicked() {
                        requested = Some(Action::Import(
                            self.legacy[self.legacy_selection].path.clone(),
                        ));
                    }
                }
                ui.add_space(8.0);
                ui.collapsing("Back up and restore",|ui|{
                    ui.label("New backup path");
                    ui.add(egui::TextEdit::singleline(&mut self.export_path).char_limit(4096)).accessible_name("New backup path");
                    if ui.add_enabled(self.document.is_some(),egui::Button::new("Create backup")).clicked(){requested=Some(Action::Export(PathBuf::from(&self.export_path)));}
                    ui.small("Choose a new file. Existing files are never overwritten.");
                    ui.separator();ui.label("Backup to inspect");
                    if ui.add(egui::TextEdit::singleline(&mut self.restore_path).char_limit(4096)).accessible_name("Backup to inspect").changed(){self.inspected=None;}
                    if ui.button("Inspect backup").clicked(){requested=Some(Action::Inspect(PathBuf::from(&self.restore_path)));}
                    if let Some(backup)=&self.inspected {
                        ui.label(format!("{} · {} tracks · {} original take(s)",backup.inspection.name,backup.inspection.track_count,backup.inspection.take_count));
                        ui.small("Preliminary summary and fingerprint only. Restoring checks the complete backup.");
                        ui.small(format!("Fingerprint: {}",backup.inspection.sha256));
                        if ui.button("Restore as new session").clicked(){requested=Some(Action::Restore{path:backup.path.clone(),intent:RestoreCopyIntent{operation_id:OperationId(uuid::Uuid::new_v4().to_string()),expected_archive_sha256:backup.inspection.sha256.clone()}});}
                    }
                    ui.small("Restoring creates a separate session and preserves the backup and existing sessions.");
                });
                ui.collapsing("Storage locations", |ui| {
                    ui.small(format!("Native: {}", self.paths.native.display()));
                    ui.small(format!("Legacy: {}", self.paths.legacy.display()));
                });
                });
            });
        egui::CentralPanel::default().show(root, |ui| {
            if !available {ui.disable();}
            if self.document.is_none() {
                ui.add_space(60.0); ui.heading("Your music stays yours.");
                ui.label("Create a native session, or bring in an existing OmniuSong project.");
                ui.label("Import preserves source notes, expression, provenance and referenced audio.");
                ui.label("Open a saved session to play supported sources or record a new solo original using the audio controls.");
                return;
            }
            ui.horizontal(|ui| {
                ui.label("Session");
                ui.add(egui::TextEdit::singleline(&mut self.draft.name).char_limit(160).desired_width(280.0)).accessible_name("Session name");
                ui.label("BPM");
                ui.add(egui::TextEdit::singleline(&mut self.draft.tempo).char_limit(32).desired_width(65.0)).accessible_name("Tempo in BPM");
                if ui.add_enabled(self.dirty(), egui::Button::new("Save changes")).clicked()
                    && let Err(error) = self.save() { self.error = Some(error.to_string()); }
                if ui.add_enabled(self.dirty(), egui::Button::new("Discard edits")).clicked()
                    && let Some(document) = &self.document { self.draft = Draft::from_document(document); }
            });
            ui.horizontal(|ui| {
                if ui.button("Undo saved change").clicked() { requested = Some(Action::Undo); }
                if ui.button("Redo saved change").clicked() { requested = Some(Action::Redo); }
                if self.dirty() { ui.colored_label(egui::Color32::GOLD, "Unsaved edits"); }
                else { ui.label("Saved"); }
            });
            if let Some(document) = &self.document {
                ui.small(format!("Schema {} · revision {} · {} bars · 960 ticks/beat", document.schema_version(), document.revision(), document.bars()));
                let recovery=&document.value()["omniusongOperatorRecovery"];
                if recovery["format"]=="omniusong.operator-recovery-evidence" && recovery["version"].as_u64()==Some(1) && recovery["historicalLedgerComplete"].as_bool()==Some(false) {
                    ui.colored_label(recovery_warning_color(ui.visuals().dark_mode),"Recovered snapshot · earlier edit history is incomplete");
                    ui.small("This session starts new edit history. The source checkpoint and recovery evidence are retained in the project.");
                }
                if let Some(import) = document.value().get("nativeImport") {
                    ui.collapsing("Imported original preserved", |ui| {
                        ui.label("Source identity and provenance retained. Referenced audio remains separately stored.");
                        ui.small(format!("Source session: {} · revision {}", text(&import["sourceId"]), import["sourceRevision"]));
                        ui.small(format!("Source SHA-256: {}", text(&import["sourceSha256"])));
                    });
                }
                ui.separator();
                let tracks = document.tracks();
                if tracks.is_empty() { ui.label("This session has no tracks."); }
                else {
                    self.selected_track = self.selected_track.min(tracks.len() - 1);
                    ui.horizontal_wrapped(|ui| {
                        for (index, track) in tracks.iter().enumerate() {
                            let label = format!("{}{}", text(&track["name"]), if track["locked"] == true { " · locked" } else { "" });
                            if ui.selectable_label(self.selected_track == index, label).clicked() {
                                self.selected_track = index; self.selected_note = None; self.roll_needs_center = true;
                            }
                        }
                    });
                    let track = &tracks[self.selected_track];
                    if document.schema_version()==2 && track["kind"] == "recording" {
                        ui.label(format!("Recording · {}", if track["muted"] == true { "muted" } else { "unmuted" }));
                        if let Some(payload)=self.recordings.get(self.selected_track).and_then(Option::as_ref){recording_details(ui,payload);}
                    } else {
                        ui.label(format!("{} · {} · {}", text(&track["kind"]), text(&track["instrument"]), if track["muted"] == true { "muted" } else { "unmuted" }));
                        if let Some(Some(provenance)) = self.provenance.get(self.selected_track) {
                            ui.collapsing("Track provenance", |ui| { ui.monospace(provenance); });
                        }
                        if track["kind"] == "audio" { audio_details(ui, track); }
                        else { piano_overview(ui, track, document.bars(), &mut self.selected_note, &mut self.roll_needs_center); }
                    }
                }
                ui.separator();
                ui.collapsing("Model and performance integration", |ui| {
                    ui.label("Existing model research and legacy outputs are preserved. Native inference adapters are pending qualification.");
                    ui.add_enabled(false, egui::Button::new("Generate candidate · pending"));
                    ui.add_enabled(false, egui::Button::new("Connect MIDI · pending"));
                    ui.add_enabled(false, egui::Button::new("Load audio plugin · pending"));
                });
            }
        });
        if let Some(action) = requested {
            self.request(action);
        }
        if self.pending.is_some() {
            let mut resolution = 0;
            let response = egui::Modal::new(egui::Id::unique("unsaved-edits")).show(&ctx, |ui| {
                ui.heading("Save your edits?");
                ui.label(
                    "This session has unsaved changes. Save or discard them before continuing.",
                );
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save and continue").clicked() {
                        resolution = 1;
                    }
                    if ui.button("Discard and continue").clicked() {
                        resolution = 2;
                    }
                    if ui.button("Cancel").clicked() {
                        resolution = 3;
                    }
                });
            });
            if response.should_close() {
                resolution = 3;
            }
            match resolution {
                1 => match self.dispatch(Action::Save, self.pending.clone()) {
                    Ok(()) => {
                        self.pending = None;
                    }
                    Err(error) => self.error = Some(error.to_string()),
                },
                2 => {
                    if let Some(document) = &self.document {
                        self.draft = Draft::from_document(document);
                    }
                    if let Some(action) = self.pending.take() {
                        self.execute(action);
                    }
                }
                3 => {
                    self.pending = None;
                    self.error = None;
                }
                _ => {}
            }
        }
        self.audio_close_guard_ui(&ctx);
        self.recovery_close_guard_ui(&ctx);
    }
    fn recovery_close_guard_ui(&mut self, ctx: &egui::Context) {
        if !self.recovery_close_requested {
            return;
        }
        let mut resolution = 0;
        egui::Modal::new(egui::Id::unique("retained-recovery-close")).show(ctx,|ui|{
            ui.heading("Retained capture work is unresolved");
            ui.label("The original journal is retained. Recovery or attachment may already have completed; closing does not prove cancellation or a complete final tail.");
            if ui.button("Keep recovery window open").clicked(){resolution=1;}
            if ui.button("Close recovery without waiting").clicked(){resolution=2;}
        });
        if resolution == 1 {
            self.recovery_close_requested = false;
        }
        if resolution == 2 {
            self.recovery_close_requested = false;
            self.closing = true;
        }
    }
    fn audio_close_guard_ui(&mut self, ctx: &egui::Context) {
        if self.audio_close_requested {
            let mut close = 0;
            egui::Modal::new(egui::Id::unique("audio-close-guard")).show(ctx,|ui|{
                ui.heading("Keep the original take safe");
                ui.label(if self.audio.capture_guard(){"Audio or take attachment is still active. Finish the take and wait for attachment, or stop audio before closing."}else{"Saved-session playback is still active. Stop audio before closing, or explicitly close without waiting."});
                ui.label("Closing without waiting can interrupt an unfinished original. A sealed journal is retained; attachment may already have completed. Closing does not prove cancellation or a complete final tail.");
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(self.audio.can_finish(),egui::Button::new("Finish take and keep window open")).clicked(){self.audio.finish();}
                    if ui.button("Stop audio and keep window open").clicked(){self.audio.stop();}
                    if ui.button("Keep window open").clicked(){close=1;}
                    if ui.button("Close without waiting").clicked(){close=2;}
                });
            });
            if close == 1 {
                self.audio_close_requested = false;
            }
            if close == 2 {
                self.audio_close_requested = false;
                self.closing = true;
            }
        }
    }
}

fn recovery_warning_color(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_rgb(255, 210, 90)
    } else {
        egui::Color32::from_rgb(112, 64, 0)
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("Unknown")
}

fn recording_details(ui: &mut egui::Ui, payload: &RecordingPayload) {
    ui.heading("Sealed original recordings");
    ui.label(format!(
        "{} declared clip(s) · read-only arrangement",
        payload.clips.len()
    ));
    ui.label("Source details are read-only. Use the audio controls for saved-session playback or a new solo take; waveform editing remains unavailable.");
    egui::ScrollArea::vertical().id_salt("recording-details").max_height(400.0).show(ui, |ui| {
        for (index, clip) in payload.clips.iter().enumerate() {
            ui.group(|ui| {
                ui.label(format!("Clip {} · {}", index + 1, clip.id));
                ui.label(format!("Source take {} · stream {}", clip.source.take_id.0, clip.source.stream_id));
                let end = clip.range.end().expect("validated recording window");
                match clip.range {
                    TakeRange::AudioFrames {start, count} => {
                        ui.label(format!("Source window: audio frames {start}–{end} · {count} frames"));
                    }
                    TakeRange::Events {start, count} => {
                        ui.label(format!("Source window: events {start}–{end} · {count} original events"));
                        ui.small("Event counts are not audio duration. Original clock observations remain in the take.");
                    }
                }
                ui.label(format!("Placement: frame {} at {} Hz ({:.3} s)", clip.position.frame, clip.position.sample_rate,
                    clip.position.frame as f64 / f64::from(clip.position.sample_rate)));
                if let Some(fades) = &clip.fades {
                    ui.small(format!("Audio fades: {} frames in · {} frames out", fades.in_frames, fades.out_frames));
                }
                ui.small(format!("Sealed manifest: {}", clip.source.manifest_sha256));
            });
        }
    });
}

fn audio_details(ui: &mut egui::Ui, track: &Value) {
    ui.heading("Original audio");
    let audio = &track["audio"];
    ui.label(format!(
        "{} · {} seconds · {} bytes",
        text(&audio["origin"]),
        audio["durationSeconds"],
        audio["byteLength"]
    ));
    ui.small(format!("Asset: {}", text(&track["assetId"])));
    for field in [
        "startTick",
        "trimStartSeconds",
        "trimEndSeconds",
        "fadeInSeconds",
        "fadeOutSeconds",
    ] {
        if let Some(value) = audio.get(field) {
            ui.label(format!("{field}: {value}"));
        }
    }
    ui.label(
        "Source and clip settings are retained. Playback reports unsupported sources explicitly; native waveform editing remains pending.",
    );
}

fn piano_overview(
    ui: &mut egui::Ui,
    track: &Value,
    bars: u64,
    selected: &mut Option<usize>,
    needs_center: &mut bool,
) {
    let notes = track["notes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    ui.heading("Piano roll · inspect original notes");
    ui.label(format!("{} notes · select a note below or in the roll · editing follows in a later native milestone", notes.len()));
    let max_end = notes
        .iter()
        .map(|n| n["startTick"].as_u64().unwrap_or(0) + n["durationTicks"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let total_ticks = max_end.max(bars * 3840).max(1);
    let content_width = (total_ticks as f32 / 3840.0 * 160.0).clamp(700.0, 24000.0);
    let mut scroll = egui::ScrollArea::both()
        .id_salt("piano-overview")
        .max_height(340.0);
    if *needs_center {
        let pitch = notes
            .first()
            .and_then(|note| note["pitch"].as_u64())
            .unwrap_or(60)
            .min(127);
        scroll = scroll.vertical_scroll_offset(((127 - pitch) as f32 * 8.0 - 160.0).max(0.0));
        *needs_center = false;
    }
    scroll.show(ui, |ui| {
        let (rect, response) = ui.allocate_exact_size(egui::vec2(content_width, 1024.0), egui::Sense::hover());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, "Piano roll: full MIDI pitch range, horizontal and vertical scrolling. Exact values are available in the note list."));
        let clip = ui.clip_rect().intersect(rect);
        let painter = ui.painter().with_clip_rect(clip);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(19, 24, 31));
        for pitch in 0..=127 {
            let y = rect.top() + (127-pitch) as f32 * 8.0;
            if y < clip.top()-8.0 || y > clip.bottom() { continue; }
            if [1,3,6,8,10].contains(&(pitch%12)) {
                painter.rect_filled(egui::Rect::from_min_size(egui::pos2(rect.left(),y),egui::vec2(content_width,8.0)),0.0,egui::Color32::from_rgb(27,31,39));
            }
            if pitch%12==0 { painter.text(egui::pos2(clip.left()+3.0,y),egui::Align2::LEFT_TOP,format!("C{}",pitch/12-1),egui::FontId::monospace(9.0),egui::Color32::GRAY); }
        }
        let visible_first = (((clip.left()-rect.left()) / content_width * total_ticks as f32 / 3840.0).floor() as u64).min(1_000_000);
        let visible_last = (((clip.right()-rect.left()) / content_width * total_ticks as f32 / 3840.0).ceil() as u64).min(visible_first+256);
        for bar in visible_first..=visible_last {
            let x = rect.left()+bar as f32*3840.0/total_ticks as f32*content_width;
            painter.line_segment([egui::pos2(x,clip.top()),egui::pos2(x,clip.bottom())],egui::Stroke::new(1.0,egui::Color32::from_gray(65)));
        }
        for (index,note) in notes.iter().enumerate() {
            let start = note["startTick"].as_u64().unwrap_or(0);
            let duration = note["durationTicks"].as_u64().unwrap_or(1);
            let pitch = note["pitch"].as_u64().unwrap_or(0).min(127);
            let note_rect = egui::Rect::from_min_size(egui::pos2(rect.left()+start as f32/total_ticks as f32*content_width,rect.top()+(127-pitch) as f32*8.0),egui::vec2((duration as f32/total_ticks as f32*content_width).max(4.0),7.0));
            if !note_rect.intersects(clip) { continue; }
            let response = ui.interact(note_rect,egui::Id::unique(("native-note",text(&track["id"]),text(&note["id"]))),egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button,true,*selected==Some(index),format!("Note {} pitch {} tick {} duration {} velocity {}",index+1,pitch,start,duration,note["velocity16"])));
            if response.clicked() { *selected=Some(index); }
            painter.rect_filled(note_rect,1.5,if *selected==Some(index) { egui::Color32::GOLD } else { egui::Color32::from_rgb(99,215,182) });
        }
    });
    if let Some(index) = *selected
        && let Some(note) = notes.get(index)
    {
        ui.label(format!(
            "Selected note {} · pitch {} · start {} ticks · length {} ticks · velocity {}/65535",
            index + 1,
            note["pitch"],
            note["startTick"],
            note["durationTicks"],
            note["velocity16"]
        ));
        if let Some(expression) = note.get("expression") {
            ui.small(format!("Expression retained: {}", expression));
        }
    }
    egui::ScrollArea::vertical()
        .id_salt("note-list")
        .max_height(150.0)
        .show_rows(ui, 20.0, notes.len(), |ui, range| {
            for index in range {
                let note = &notes[index];
                if ui
                    .selectable_label(
                        *selected == Some(index),
                        format!(
                            "Note {} · pitch {} · tick {} · length {} · velocity {}",
                            index + 1,
                            note["pitch"],
                            note["startTick"],
                            note["durationTicks"],
                            note["velocity16"]
                        ),
                    )
                    .clicked()
                {
                    *selected = Some(index);
                }
            }
        });
}

impl StudioApp {
    fn native_ui(&mut self, ui: &mut egui::Ui) {
        if ui.ctx().input(|i| i.viewport().close_requested()) && self.audio.blocks_session() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.audio_close_requested = true;
        } else if ui.ctx().input(|i| i.viewport().close_requested())
            && self.recovery.blocks_session()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.recovery_close_requested = true;
        } else if ui.ctx().input(|i| i.viewport().close_requested()) && self.dirty() && !self.busy()
        {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.pending.is_none() {
                self.pending = Some(Action::Close);
            }
        }
        self.show(ui);
        if self.closing {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.native_ui(ui);
    }
}

fn main() -> AppResult<()> {
    let mut args = std::env::args();
    let executable = PathBuf::from(args.next().ok_or("Missing executable path")?);
    let args: Vec<_> = args.collect();
    if args.first().is_some_and(|arg| arg == "--audio-worker") {
        return omniusong_engine::run_worker().map_err(Into::into);
    }
    let Some(paths) = Paths::parse(args)? else {
        return Ok(());
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 1000.0])
            .with_min_inner_size([1000.0, 720.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "OmniuSong — Native Studio",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(
                StudioApp::new(paths).with_audio_executable(executable),
            ))
        }),
    )
    .map_err(|e| format!("Native application failed: {e}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniusong_session::{Backend, CommandKind, ServiceError};
    use std::{fs, sync::mpsc, time::Instant};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.runtime/rust-migration/session-app-tests");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn paths(&self) -> Paths {
            Paths {
                native: self.0.join("native"),
                legacy: self.0.join("legacy"),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn frame(
        ctx: &egui::Context,
        app: &mut StudioApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.native_ui(ui),
        );
        output.textures_delta.clear();
        output
    }
    fn settle(ctx: &egui::Context, app: &mut StudioApp) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let _ = frame(ctx, app, vec![]);
            if !app.busy() && (app.ready || app.error.is_some()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Session worker did not complete; {:?}",
                app.error
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn click(ctx: &egui::Context, app: &mut StudioApp, label: &str) {
        // egui Area deliberately hides a newly opened modal's first sizing
        // frame and requests repaint. Render that frame before finding its AX
        // control; an invisible sizing pass cannot receive an accessible click.
        let _ = frame(ctx, app, vec![]);
        let tree = frame(ctx, app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap_or_else(|| {
                panic!(
                    "Missing action {label}; ready={}, busy={}, pending={}, text={:?}",
                    app.ready,
                    app.busy(),
                    app.pending.is_some(),
                    tree.nodes
                        .iter()
                        .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
                        .collect::<Vec<_>>()
                )
            })
            .0;
        let _ = frame(
            ctx,
            app,
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
    fn ready(fixture: &Fixture) -> (egui::Context, StudioApp) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = StudioApp::new(fixture.paths());
        settle(&ctx, &mut app);
        assert!(app.ready, "{:?}", app.error);
        (ctx, app)
    }
    fn manual(fixture: &Fixture) -> (egui::Context, StudioApp, omniusong_session::TestPeer) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (service, peer) = SessionService::test_pair();
        let mut app = StudioApp::new(fixture.paths());
        app.service = Some(service);
        app.ready = true;
        app.install_document(
            DocumentView::prepare(ProjectDocument::new("Manual session").unwrap()).unwrap(),
        );
        (ctx, app, peer)
    }

    #[test]
    fn creator_is_io_free_and_held_startup_keeps_paint_close_and_drop_responsive() {
        struct Held {
            entered: mpsc::SyncSender<std::thread::ThreadId>,
            gate: mpsc::Receiver<()>,
        }
        impl Backend for Held {
            fn handle(&mut self, _: &Command) -> Result<Reply, ServiceError> {
                self.entered.send(std::thread::current().id()).unwrap();
                self.gate.recv().unwrap();
                Ok(Reply::Library(Library::default()))
            }
        }
        let fixture = Fixture::new();
        let paths = fixture.paths();
        let mut app = StudioApp::new(paths.clone());
        assert!(!paths.native.exists());
        assert!(!paths.legacy.exists());
        let (entered, started) = mpsc::sync_channel(1);
        let (release, gate) = mpsc::channel();
        app.service = Some(SessionService::spawn_with_backend(Held { entered, gate }).unwrap());
        app.execute_initialize();
        assert_ne!(
            started.recv_timeout(Duration::from_secs(1)).unwrap(),
            std::thread::current().id()
        );
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        for _ in 0..12 {
            assert!(!frame(&ctx, &mut app, vec![]).shapes.is_empty());
            assert!(app.busy());
        }
        assert!(!paths.native.exists());
        assert!(!paths.legacy.exists());
        click(&ctx, &mut app, "Close");
        assert!(app.closing);
        assert!(app.busy());
        let before = Instant::now();
        drop(app);
        assert!(before.elapsed() < Duration::from_millis(200));
        release.send(()).unwrap();
    }

    #[test]
    fn accessible_create_atomic_save_undo_redo_and_reopen_use_persistent_worker() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.new_name = "Native worker creation".into();
        click(&ctx, &mut app, "Create session");
        assert!(app.busy());
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        let id = app.document.as_ref().unwrap().id().to_owned();
        app.draft.name = "One atomic draft save".into();
        app.draft.tempo = "125.5".into();
        click(&ctx, &mut app, "Save changes");
        settle(&ctx, &mut app);
        let saved = app.document.as_ref().unwrap();
        assert_eq!(saved.revision(), 1);
        assert_eq!(saved.name(), "One atomic draft save");
        assert_eq!(saved.tempo(), 125.5);
        click(&ctx, &mut app, "Undo saved change");
        settle(&ctx, &mut app);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "Native worker creation"
        );
        assert_eq!(app.document.as_ref().unwrap().tempo(), 104.0);
        click(&ctx, &mut app, "Redo saved change");
        settle(&ctx, &mut app);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "One atomic draft save"
        );
        drop(app);
        let (_, mut reopened) = ready(&fixture);
        reopened.request(Action::Open(id));
        settle(&ctx, &mut reopened);
        assert_eq!(reopened.document.as_ref().unwrap().tempo(), 125.5);
    }

    #[test]
    fn invalid_draft_and_save_before_navigation_keep_the_same_pending_action() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let id = app.document.as_ref().unwrap().id().to_owned();
        app.draft.name = "Pending saved name".into();
        app.draft.tempo = "invalid".into();
        app.request(Action::Open(id.clone()));
        assert!(app.pending.is_some());
        assert!(app.save().is_err());
        assert!(!app.busy());
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        app.draft.tempo = "128".into();
        click(&ctx, &mut app, "Save and continue");
        assert!(app.pending.is_none());
        assert!(app.busy());
        settle(&ctx, &mut app);
        assert_eq!(app.document.as_ref().unwrap().id(), id);
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(app.document.as_ref().unwrap().name(), "Pending saved name");
    }

    #[test]
    fn lost_ack_retry_keeps_operation_id_uses_new_transport_id_and_does_not_duplicate_save() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.draft.name = "Stable intent".into();
        app.save().unwrap();
        let first = peer.requests.try_recv().unwrap();
        let operation = match &first.command {
            Command::Edit { intent } => intent.operation_id.clone(),
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::backend("Acknowledgement was lost", true)),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.busy());
        assert_eq!(app.draft.name, "Stable intent");
        click(&ctx, &mut app, "Retry last action");
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(first.id, retry.id);
        assert!(matches!(retry.command,Command::Edit{ref intent}if intent.operation_id==operation));
        assert!(app.busy());
        app.request(Action::Create);
        assert!(peer.requests.try_recv().is_err());
    }

    #[test]
    fn stale_context_completion_never_replaces_newer_draft() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.draft.name = "Accepted save".into();
        app.save().unwrap();
        let request = peer.requests.try_recv().unwrap();
        let mut result = ProjectDocument::new("Accepted save").unwrap();
        result.rename("Backend result").unwrap();
        app.draft.name = "A newer local draft".into();
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(result).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.draft.name, "A newer local draft");
        assert_eq!(app.document.as_ref().unwrap().name(), "Manual session");
        assert!(app.error.as_ref().unwrap().contains("draft changed"));
    }

    #[test]
    fn export_inspect_and_restore_copy_are_worker_actions_with_dirty_resolution() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let source_id = app.document.as_ref().unwrap().id().to_owned();
        let backup = fixture.0.join("new-backup.omniusong");
        app.draft.name = "Saved before backup".into();
        app.request(Action::Export(backup.clone()));
        assert!(app.pending.is_some());
        assert!(!backup.exists());
        click(&ctx, &mut app, "Save and continue");
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert!(backup.exists());
        let bytes = fs::read(&backup).unwrap();
        app.restore_path = backup.to_string_lossy().into_owned();
        app.request(Action::Inspect(backup.clone()));
        settle(&ctx, &mut app);
        let inspected = app.inspected.as_ref().unwrap();
        let intent = RestoreCopyIntent {
            operation_id: OperationId(uuid::Uuid::new_v4().to_string()),
            expected_archive_sha256: inspected.inspection.sha256.clone(),
        };
        app.draft.name = "Explicitly discarded draft".into();
        app.request(Action::Restore {
            path: backup.clone(),
            intent: intent.clone(),
        });
        assert!(app.pending.is_some());
        click(&ctx, &mut app, "Discard and continue");
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_ne!(app.document.as_ref().unwrap().id(), source_id);
        assert_eq!(app.document.as_ref().unwrap().name(), "Saved before backup");
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        assert_eq!(app.sessions.len(), 2);
    }

    #[test]
    fn recording_view_ignores_opaque_legacy_arrays_and_keeps_exact_metadata() {
        use omniusong_core::{
            contracts::TakeId,
            native::{TakeClip, TakeReference, TimelinePosition},
        };
        let fixture = Fixture::new();
        let (ctx, mut app, _) = manual(&fixture);
        let mut document = ProjectDocument::new("Declared original")
            .unwrap()
            .upgrade_native()
            .unwrap();
        document
            .append_take_clip(
                "recording-track",
                "Original recording",
                TakeClip {
                    id: "declared-clip".into(),
                    source: TakeReference {
                        take_id: TakeId("original-take".into()),
                        manifest_sha256: "a".repeat(64),
                        stream_id: "microphone".into(),
                    },
                    range: TakeRange::AudioFrames {
                        start: 100,
                        count: 4,
                    },
                    position: TimelinePosition {
                        frame: -48_000,
                        sample_rate: 48_000,
                    },
                    fades: None,
                },
            )
            .unwrap();
        let mut value = document.value().clone();
        let track = value["tracks"].as_array_mut().unwrap().last_mut().unwrap();
        track["notes"] =
            serde_json::json!([null,{"startTick":u64::MAX,"durationTicks":u64::MAX},["opaque"]]);
        track["audio"] = serde_json::json!(["opaque"]);
        track["instrument"] = serde_json::json!(["opaque"]);
        track["provenance"] = serde_json::json!(["opaque"]);
        let document = ProjectDocument::from_value(value).unwrap();
        let retained = document.to_json().unwrap();
        app.install_document(DocumentView::prepare(document).unwrap());
        app.selected_track = app.document.as_ref().unwrap().tracks().len() - 1;
        let tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        for expected in [
            "Sealed original recordings",
            "1 declared clip(s) · read-only arrangement",
            "Source take original-take · stream microphone",
            "Source window: audio frames 100–104 · 4 frames",
            "Placement: frame -48000 at 48000 Hz (-1.000 s)",
        ] {
            assert!(text.contains(&expected), "Missing {expected}: {text:?}");
        }
        assert!(!text.iter().any(|value| value.contains("Piano roll")
            || value.starts_with("Note ")
            || value.contains("Track provenance")));
        assert!(app.roll_needs_center);
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), retained);
    }

    #[test]
    fn path_overrides_remain_explicit_and_need_no_server() {
        let paths = Paths::parse([
            "--data-dir".into(),
            "/tmp/native-song".into(),
            "--legacy-dir".into(),
            "/tmp/legacy-song".into(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(paths.native, PathBuf::from("/tmp/native-song"));
        assert_eq!(paths.legacy, PathBuf::from("/tmp/legacy-song"));
        assert!(Paths::parse(["--data-dir".into()]).is_err());
        assert!(Paths::parse(["--unknown".into()]).is_err());
    }

    #[test]
    fn fixture_cleanup_never_removes_another_exclusively_owned_fixture() {
        let first = Fixture::new();
        let second = Fixture::new();
        assert_ne!(first.0, second.0);
        let marker = first.0.join("original-marker");
        fs::write(&marker, b"preserve").unwrap();
        drop(second);
        assert_eq!(fs::read(marker).unwrap(), b"preserve");
    }

    #[test]
    fn failed_initialization_stays_visible_and_only_explicit_action_retries() {
        let fixture = Fixture::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (mut service, peer) = SessionService::test_pair();
        let first = service.try_submit(Command::Initialize).unwrap();
        let request = peer.requests.try_recv().unwrap();
        let mut app = StudioApp::new(fixture.paths());
        app.service = Some(service);
        app.active = Some(Active {
            id: first,
            command: Command::Initialize,
            action: Action::Refresh,
            context: None,
            draft: Draft::default(),
            after: None,
        });
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Initialize,
                result: Err(ServiceError::backend("Storage unavailable fixture", false)),
            })
            .unwrap();
        for _ in 0..8 {
            let output = frame(&ctx, &mut app, vec![]);
            assert!(!output.shapes.is_empty());
            assert!(!app.ready);
            assert!(!app.busy());
        }
        assert!(peer.requests.try_recv().is_err());
        assert!(app.error.as_ref().unwrap().contains("Storage unavailable"));
        click(&ctx, &mut app, "Try opening storage again");
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(retry.id, first);
        assert!(matches!(retry.command, Command::Initialize));
        assert!(app.busy());
        assert!(!fixture.paths().native.exists());
    }

    #[test]
    fn fatal_completion_keeps_error_draft_and_intent_until_explicit_fresh_reconnect_and_retry() {
        let fixture = Fixture::new();
        let (ctx, mut app) = ready(&fixture);
        app.execute(Action::Create);
        settle(&ctx, &mut app);
        let (service, peer) = SessionService::test_pair();
        let old_generation = service.generation();
        app.service = Some(service);
        app.draft.name = "Draft retained through storage quarantine".into();
        app.save().unwrap();
        let request = peer.requests.try_recv().unwrap();
        let operation = match &request.command {
            Command::Edit { intent } => intent.operation_id.clone(),
            _ => panic!(),
        };
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::Edit,
                result: Err(ServiceError::storage_unavailable(
                    "Synthetic fatal storage evidence",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(!app.busy());
        assert_eq!(app.draft.name, "Draft retained through storage quarantine");
        let known = app.error.clone().unwrap();
        assert!(known.contains("Synthetic fatal storage evidence"));
        assert!(known.contains("may have completed"));
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![]);
            assert_eq!(app.error.as_deref(), Some(known.as_str()));
        }
        assert!(
            matches!(app.retry.as_ref().map(|active|&active.command),Some(Command::Edit{intent})if intent.operation_id==operation)
        );
        click(&ctx, &mut app, "Reconnect storage");
        settle(&ctx, &mut app);
        assert!(!app.disconnected());
        assert_ne!(app.service.as_ref().unwrap().generation(), old_generation);
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        assert_eq!(app.draft.name, "Draft retained through storage quarantine");
        assert!(
            app.retry.is_some(),
            "Reconnection must not automatically resubmit the durable intent"
        );
        click(&ctx, &mut app, "Retry last action");
        assert!(
            matches!(app.active.as_ref().map(|active|&active.command),Some(Command::Edit{intent})if intent.operation_id==operation)
        );
        settle(&ctx, &mut app);
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(
            app.document.as_ref().unwrap().name(),
            "Draft retained through storage quarantine"
        );
    }

    #[test]
    fn disconnected_before_any_initialization_can_explicitly_retry_deliberate_first_run() {
        let fixture = Fixture::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (service, peer) = SessionService::test_pair();
        let mut app = StudioApp::new(fixture.paths());
        let generation = service.generation();
        app.service = Some(service);
        app.execute_initialize();
        let _ = peer.requests.try_recv().unwrap();
        drop(peer);
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(!app.reconnect_existing);
        assert!(!fixture.paths().native.exists());
        click(&ctx, &mut app, "Try opening storage again");
        settle(&ctx, &mut app);
        assert!(app.ready);
        assert!(app.reconnect_existing);
        assert_ne!(app.service.as_ref().unwrap().generation(), generation);
        assert!(fixture.paths().native.join("studio.sqlite3").is_file());
    }

    #[test]
    fn operator_recovery_notice_requires_exact_root_evidence_and_retains_opaque_values() {
        let fixture = Fixture::new();
        let (ctx, mut app, _peer) = manual(&fixture);
        for (format, version, complete, expected) in [
            ("omniusong.operator-recovery-evidence", 1, false, true),
            ("future.operator-recovery", 1, false, false),
            ("omniusong.operator-recovery-evidence", 2, false, false),
            ("omniusong.operator-recovery-evidence", 1, true, false),
        ] {
            let mut value = ProjectDocument::new("Recovery disclosure fixture")
                .unwrap()
                .value()
                .clone();
            value["omniusongOperatorRecovery"] = serde_json::json!({"format":format,"version":version,"historicalLedgerComplete":complete,"opaqueEvidence":[null,{"takeId":"foreign"}]});
            value["vendor"] = serde_json::json!({"omniusongOperatorRecovery":{"format":"omniusong.operator-recovery-evidence","version":1,"historicalLedgerComplete":false}});
            let document = ProjectDocument::from_value(value).unwrap();
            let bytes = document.to_json().unwrap();
            app.install_document(DocumentView::prepare(document).unwrap());
            let tree = frame(&ctx, &mut app, vec![])
                .platform_output
                .accesskit_update
                .unwrap();
            let text: Vec<_> = tree
                .nodes
                .iter()
                .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
                .collect();
            assert_eq!(
                text.contains(&"Recovered snapshot · earlier edit history is incomplete"),
                expected
            );
            assert_eq!(text.contains(&"This session starts new edit history. The source checkpoint and recovery evidence are retained in the project."),expected);
            assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), bytes);
            assert!(
                app.document
                    .as_ref()
                    .unwrap()
                    .referenced_takes()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn completed_ordinary_inspection_error_clears_background_work_status() {
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.restore_path = "/synthetic/malformed-backup".into();
        app.execute(Action::Inspect(PathBuf::from(&app.restore_path)));
        let request = peer.requests.try_recv().unwrap();
        peer.responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::InspectArchive,
                result: Err(ServiceError::backend("Synthetic invalid archive", false)),
            })
            .unwrap();
        let tree = frame(&ctx, &mut app, vec![])
            .platform_output
            .accesskit_update
            .unwrap();
        assert!(!app.busy());
        assert!(!app.disconnected());
        let text: Vec<_> = tree
            .nodes
            .iter()
            .flat_map(|(_, node)| [node.label(), node.value()].into_iter().flatten())
            .collect();
        assert!(text.contains(&"Synthetic invalid archive"));
        assert!(text.contains(
            &"Action finished with an error. Your saved session and local draft were kept."
        ));
        assert!(
            !text
                .iter()
                .any(|text| text.starts_with("Work is running in the background"))
        );
    }

    #[test]
    fn recovery_warning_meets_text_contrast_on_both_default_panel_themes() {
        fn luminance(color: egui::Color32) -> f64 {
            let linear = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        }
        for (dark, visuals) in [
            (true, egui::Visuals::dark()),
            (false, egui::Visuals::light()),
        ] {
            let fg = luminance(recovery_warning_color(dark));
            let bg = luminance(visuals.panel_fill);
            let contrast = (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05);
            assert!(
                contrast >= 4.5,
                "Theme dark={dark} recovery warning contrast {contrast}"
            );
        }
    }

    #[test]
    fn accessible_capture_guards_edits_navigation_and_close_but_stop_stays_queued() {
        use audio_ui::test_support;
        use omniusong_engine::{EngineCommand, EngineResult};
        let fixture = Fixture::new();
        let (ctx, mut app, store_peer) = manual(&fixture);
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        click(&ctx, &mut app, "Record solo");
        assert!(matches!(
            peer.command(),
            Some(EngineCommand::StartCapture { input_device: None })
        ));
        assert!(app.audio.capture_guard());
        let original = app.document.as_ref().unwrap().to_json().unwrap();
        app.request(Action::Open("another-project".into()));
        assert!(store_peer.requests.try_recv().is_err());
        app.request(Action::Recovery(RecoveryAction::Scan));
        assert!(store_peer.requests.try_recv().is_err());
        assert!(app.save().is_err());
        app.request(Action::Export(PathBuf::from("/synthetic/new-backup")));
        assert!(store_peer.requests.try_recv().is_err());
        app.request(Action::Close);
        assert!(app.audio_close_requested);
        assert!(!app.closing);
        // Stop is admitted locally while the starting input still owns the one credit.
        app.audio.stop();
        assert!(peer.command().is_none());
        peer.reply(Ok(EngineResult::Capturing(test_support::status(
            "capturing",
        ))));
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(matches!(peer.command(), Some(EngineCommand::Stop)));
        peer.reply(Ok(EngineResult::Stopped));
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.audio.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(store_peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Keep window open");
        assert!(!app.closing);
    }

    #[test]
    fn sealed_capture_publication_failure_keeps_original_and_same_retry_intent() {
        use audio_ui::test_support;
        use omniusong_engine::{EngineCommand, EngineResult};
        let fixture = Fixture::new();
        let (ctx, mut app, store_peer) = manual(&fixture);
        let original_id = app.document.as_ref().unwrap().id().to_owned();
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        click(&ctx, &mut app, "Record solo");
        let _ = peer.command();
        peer.reply(Ok(EngineResult::Capturing(test_support::status(
            "capturing",
        ))));
        let _ = frame(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, "Finish take");
        assert!(matches!(peer.command(), Some(EngineCommand::FinishCapture)));
        peer.reply(Ok(EngineResult::Captured(test_support::take())));
        let _ = frame(&ctx, &mut app, vec![]);
        let request = store_peer.requests.try_recv().unwrap();
        let (journal_path, publication) = match request.command {
            Command::AttachCapture {
                journal_path,
                publication,
            } => (journal_path, publication),
            _ => panic!("Capture must attach through the storage worker"),
        };
        assert_eq!(publication.project_id.0, original_id);
        assert_eq!(publication.expected_revision, 0);
        store_peer
            .responses
            .try_send(Completion {
                id: request.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::backend(
                    "Synthetic lost attachment acknowledgement",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.busy());
        assert!(app.audio.capture_guard());
        assert!(app.audio.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 0);
        click(&ctx, &mut app, "Retry attach sealed take");
        let retry = store_peer.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture{journal_path:path,publication:intent}if path==&journal_path&&intent==&publication)
        );
        assert_ne!(request.id, retry.id);
        let mut document = app.document.as_ref().unwrap().upgrade_native().unwrap();
        document
            .append_take_clip(
                &publication.track_id.0,
                &publication.track_name,
                publication.clip.clone(),
            )
            .unwrap();
        let mut value = document.value().clone();
        value["revision"] = 1.into();
        let document = ProjectDocument::from_value(value).unwrap();
        store_peer
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(document).unwrap(),
                    library: Library::default(),
                    replayed: true,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.audio.blocks_session());
        assert!(!app.audio.capture_guard());
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        assert_eq!(
            app.document.as_ref().unwrap().take_clips().unwrap(),
            vec![publication.clip]
        );
        assert!(peer.command().is_none());
    }

    #[test]
    fn dirty_draft_never_starts_audio_and_close_with_held_capture_stays_explicit() {
        use audio_ui::test_support;
        use omniusong_engine::EngineCommand;
        let fixture = Fixture::new();
        let (ctx, mut app, _store_peer) = manual(&fixture);
        let (audio, peer) = test_support::pair();
        app.audio = audio;
        app.draft.name = "Unsaved local name".into();
        click(&ctx, &mut app, "Play saved session");
        click(&ctx, &mut app, "Record solo");
        assert!(peer.command().is_none());
        app.draft = Draft::from_document(app.document.as_ref().unwrap());
        click(&ctx, &mut app, "Record solo");
        assert!(matches!(
            peer.command(),
            Some(EngineCommand::StartCapture { .. })
        ));
        app.request(Action::Close);
        assert!(!app.closing);
        assert!(app.audio_close_requested);
        click(&ctx, &mut app, "Close without waiting");
        assert!(app.closing);
        let before = Instant::now();
        drop(app);
        assert!(before.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn retained_scan_inspection_and_explicit_recovery_keep_music_and_retry_identity() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        let original = app.document.as_ref().unwrap().to_json().unwrap();
        app.draft.name = "Unsaved draft survives read-only inspection".into();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            peer.requests.try_recv().is_err(),
            "Opening the panel must not automatically scan"
        );
        click(&ctx, &mut app, "Scan retained captures");
        let scan = peer.requests.try_recv().unwrap();
        assert!(matches!(scan.command, Command::ScanCaptures));
        assert!(app.pending.is_none());
        peer.responses
            .try_send(Completion {
                id: scan.id,
                kind: CommandKind::ScanCaptures,
                result: Ok(Reply::CaptureStages(retained::scan())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        click(&ctx, &mut app, "Inspect selected capture");
        let inspect = peer.requests.try_recv().unwrap();
        assert!(
            matches!(inspect.command,Command::InspectCapture {stage_id}if stage_id==retained::STAGE)
        );
        peer.responses
            .try_send(Completion {
                id: inspect.id,
                kind: CommandKind::InspectCapture,
                result: Ok(Reply::CaptureInspected(retained::inspection(false))),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(
            app.draft.name,
            "Unsaved draft survives read-only inspection"
        );
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(peer.requests.try_recv().is_err());
        let action = retained::recover(&app.recovery);
        let operation = retained::operation(&action);
        app.request(Action::Recovery(action.clone()));
        assert!(app.pending.is_some());
        assert!(peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Discard and continue");
        let first = peer.requests.try_recv().unwrap();
        assert!(
            matches!(&first.command,Command::RecoverCapture {operation_id,..}if operation_id.0==operation)
        );
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::RecoverCapture,
                result: Err(ServiceError::backend(
                    "Synthetic recovery acknowledgement lost",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.retry_last();
        let retry = peer.requests.try_recv().unwrap();
        assert_ne!(first.id, retry.id);
        assert!(
            matches!(&retry.command,Command::RecoverCapture {inspection,operation_id}if operation_id.0==operation&&inspection.inspection_sha256=="b".repeat(64))
        );
        peer.responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::RecoverCapture,
                result: Ok(Reply::CaptureRecovered(retained::receipt(&action))),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert_eq!(app.document.as_ref().unwrap().to_json().unwrap(), original);
        assert!(!app.recovery.blocks_session());
        assert!(
            peer.requests.try_recv().is_err(),
            "A recovered receipt must not automatically attach or switch projects"
        );
    }

    #[test]
    fn retained_attachment_saves_first_then_keeps_bound_intent_through_fatal_reconnect() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.recovery.scanned(retained::scan());
        app.recovery.inspected(retained::inspection(true)).unwrap();
        let action = retained::attach(&app.recovery, app.document.as_ref().unwrap().id());
        let operation = retained::operation(&action);
        app.draft.name = "Saved before retained attachment".into();
        app.request(Action::Recovery(action));
        assert!(app.pending.is_some());
        assert!(peer.requests.try_recv().is_err());
        click(&ctx, &mut app, "Save and continue");
        let save = peer.requests.try_recv().unwrap();
        assert!(matches!(save.command, Command::Edit { .. }));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Saved before retained attachment".into();
        value["revision"] = 1.into();
        let saved = ProjectDocument::from_value(value).unwrap();
        peer.responses
            .try_send(Completion {
                id: save.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(saved).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        let attach = peer.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = &attach.command else {
            panic!()
        };
        let publication = publication.clone();
        assert_eq!(publication.operation_id.0, operation);
        assert_eq!(publication.expected_revision, 1);
        assert_eq!(
            publication.clip.range,
            TakeRange::AudioFrames {
                start: 100,
                count: 4
            }
        );
        assert_eq!(publication.clip.position.frame, 0);
        peer.responses
            .try_send(Completion {
                id: attach.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::storage_unavailable(
                    "Synthetic fatal attachment acknowledgement",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.disconnected());
        assert!(app.recovery.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 1);
        app.request(Action::Close);
        assert!(app.recovery_close_requested);
        assert!(!app.closing);
        click(&ctx, &mut app, "Keep recovery window open");
        let (service, fresh) = SessionService::test_pair();
        app.service = Some(service);
        app.execute_initialize();
        let initialize = fresh.requests.try_recv().unwrap();
        assert!(matches!(initialize.command, Command::Initialize));
        fresh
            .responses
            .try_send(Completion {
                id: initialize.id,
                kind: CommandKind::Initialize,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            fresh.requests.try_recv().is_err(),
            "Initialization must not replay recovery or attachment"
        );
        app.request(Action::Recovery(retained::retry_attach(&app.recovery)));
        let retry = fresh.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture {publication:retried,..}if retried==&publication)
        );
        let mut document = app.document.as_ref().unwrap().upgrade_native().unwrap();
        document
            .append_take_clip(
                &publication.track_id.0,
                &publication.track_name,
                publication.clip.clone(),
            )
            .unwrap();
        let mut value = document.value().clone();
        value["revision"] = 2.into();
        let document = ProjectDocument::from_value(value).unwrap();
        fresh
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(document).unwrap(),
                    library: Library::default(),
                    replayed: true,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(!app.recovery.blocks_session());
        assert_eq!(app.document.as_ref().unwrap().revision(), 2);
        assert_eq!(
            app.document.as_ref().unwrap().take_clips().unwrap(),
            vec![publication.clip]
        );
    }

    #[test]
    fn keep_local_releases_workflow_and_only_typed_conflict_allows_a_new_attachment_choice() {
        use recovery_ui::test_support as retained;
        let fixture = Fixture::new();
        let (ctx, mut app, peer) = manual(&fixture);
        app.recovery.scanned(retained::scan());
        app.recovery.inspected(retained::inspection(true)).unwrap();
        let target = app.document.as_ref().unwrap().id().to_owned();
        app.request(Action::Recovery(retained::attach(&app.recovery, &target)));
        let first = peer.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = &first.command else {
            panic!()
        };
        let original = publication.clone();
        peer.responses
            .try_send(Completion {
                id: first.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError::backend(
                    "Revision conflict words alone are not a known outcome",
                    true,
                )),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(app.recovery.blocks_session());
        assert!(!retained::release_rejected(&mut app.recovery));
        assert!(retained::keep_local(&mut app.recovery));
        assert!(!app.recovery.blocks_session());
        let (service, fresh) = SessionService::test_pair();
        app.service = Some(service);
        app.execute_initialize();
        let initialize = fresh.requests.try_recv().unwrap();
        assert_ne!(first.id.generation, initialize.id.generation);
        fresh
            .responses
            .try_send(Completion {
                id: initialize.id,
                kind: CommandKind::Initialize,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(
            fresh.requests.try_recv().is_err(),
            "Reconnect must not replay the suspended request"
        );
        app.request(Action::Refresh);
        let refresh = fresh.requests.try_recv().unwrap();
        assert!(matches!(refresh.command, Command::Refresh));
        fresh
            .responses
            .try_send(Completion {
                id: refresh.id,
                kind: CommandKind::Refresh,
                result: Ok(Reply::Library(Library::default())),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Open(target.clone()));
        let open = fresh.requests.try_recv().unwrap();
        assert!(matches!(open.command, Command::Open { .. }));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Latest external revision".into();
        value["revision"] = 1.into();
        let current = ProjectDocument::from_value(value).unwrap();
        fresh
            .responses
            .try_send(Completion {
                id: open.id,
                kind: CommandKind::Open,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(current).unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.draft.name = "Work continues after keep-local".into();
        app.save().unwrap();
        let save = fresh.requests.try_recv().unwrap();
        assert!(matches!(&save.command,Command::Edit {intent}if intent.expected_revision==1));
        let mut value = app.document.as_ref().unwrap().value().clone();
        value["name"] = "Work continues after keep-local".into();
        value["revision"] = 2.into();
        fresh
            .responses
            .try_send(Completion {
                id: save.id,
                kind: CommandKind::Edit,
                result: Ok(Reply::Document {
                    view: DocumentView::prepare(ProjectDocument::from_value(value).unwrap())
                        .unwrap(),
                    library: Library::default(),
                    replayed: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        app.request(Action::Recovery(retained::retry_attach(&app.recovery)));
        let retry = fresh.requests.try_recv().unwrap();
        assert!(
            matches!(&retry.command,Command::AttachCapture {publication,..}if publication==&original)
        );
        assert_eq!(
            original.expected_revision, 0,
            "Continuing to work must not rebase the suspended publication"
        );
        fresh
            .responses
            .try_send(Completion {
                id: retry.id,
                kind: CommandKind::AttachCapture,
                result: Err(ServiceError {
                    kind: FailureKind::RevisionConflict,
                    message: "Synthetic proven project conflict".into(),
                    outcome_unknown: false,
                }),
            })
            .unwrap();
        let _ = frame(&ctx, &mut app, vec![]);
        assert!(retained::release_rejected(&mut app.recovery));
        assert!(!app.recovery.blocks_session());
        assert!(
            fresh.requests.try_recv().is_err(),
            "Release is a local decision, never a new publication"
        );
        app.request(Action::Recovery(retained::attach(&app.recovery, &target)));
        let next = fresh.requests.try_recv().unwrap();
        let Command::AttachCapture { publication, .. } = next.command else {
            panic!()
        };
        assert_eq!(publication.expected_revision, 2);
        assert_ne!(publication.operation_id, original.operation_id);
        assert_ne!(publication.track_id, original.track_id);
    }
}
