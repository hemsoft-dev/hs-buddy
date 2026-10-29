//! Tempo inputs and UI state perform I/O only through an explicit worker.
mod actions;
mod copy;
mod editor;
mod grid;
mod io;
mod metadata;
mod preferences;
mod render;
use crate::runtime::Runtime;
use buddy_core::tempo::{
    ApiOrigin, DateRange, Snapshot, WritePhase,
    credentials::{self, Credentials},
};
use gpui_kit::component::input::InputState;
use gpui_kit::{AppContext as _, Context, Entity, Window};
use std::sync::Arc;

/// Severity of the single status block; the primary outcome leads it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Info,
    Success,
    Warning,
    /// A destructive confirmation; its frame matches the danger button it holds.
    Danger,
    Error,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Timesheet,
    Timeline,
}
enum Operation {
    Refresh,
    Templates,
    Template(buddy_core::tempo::WorklogId, chrono::NaiveDate),
    PrepareCopy(Vec<buddy_core::tempo::WorklogId>),
    ExecuteCopy(Box<buddy_core::tempo::CopyPlan>),
    Month(i32),
    Today,
    Range(DateRange),
    Cleanup,
    RetryPrepared,
    Discard,
    Editor(buddy_core::tempo::WorklogId),
    Lookup(uuid::Uuid, String),
    Submit(Box<buddy_core::tempo::Draft>),
}
pub struct TempoView {
    connection: Option<io::Connection>,
    data: Option<Arc<Snapshot>>,
    metadata: Arc<metadata::Metadata>,
    templates: Option<Arc<buddy_core::tempo::Templates>>,
    pending_template: Option<(
        buddy_core::tempo::Worklog,
        buddy_core::tempo::EditorIssue,
        chrono::NaiveDate,
    )>,
    editor: Option<editor::Editor>,
    editor_binding: Option<io::Binding>,
    pending_editor: Option<(buddy_core::tempo::Worklog, buddy_core::tempo::EditorIssue)>,
    delete_target: Option<buddy_core::tempo::Worklog>,
    copy_plan: Option<Box<buddy_core::tempo::CopyPlan>>,
    cell_filter: Option<(chrono::NaiveDate, Option<buddy_core::tempo::IssueId>)>,
    credentials: [Entity<InputState>; 4],
    tempo_origin: ApiOrigin,
    from: Entity<InputState>,
    to: Entity<InputState>,
    pane: Pane,
    page: usize,
    initialized: bool,
    busy: bool,
    generation: uuid::Uuid,
    connection_form: bool,
    clear_credentials: bool,
    populate_range: bool,
    discard_confirm: bool,
    notice: Option<String>,
    notice_tone: Tone,
    error: Option<String>,
    /// Whether this profile has a saved connection; `None` until checked.
    saved: Option<bool>,
    grid_scroll: gpui_kit::ScrollHandle,
    today: Option<chrono::NaiveDate>,
    scroll_to_today: bool,
}
impl TempoView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pane = if cx.has_global::<crate::settings::Settings>() {
            Pane::from_config(&crate::settings::Settings::global(cx).config)
        } else {
            Pane::Timesheet
        };
        let credentials = std::array::from_fn(|index| {
            cx.new(|cx| InputState::new(window, cx).masked(index == 0 || index == 3))
        });
        Self {
            connection: None,
            data: None,
            metadata: Arc::new(metadata::Metadata::default()),
            templates: None,
            pending_template: None,
            editor: None,
            editor_binding: None,
            pending_editor: None,
            delete_target: None,
            copy_plan: None,
            cell_filter: None,
            credentials,
            tempo_origin: ApiOrigin::Global,
            from: cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD")),
            to: cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD")),
            pane,
            page: 0,
            initialized: false,
            busy: false,
            generation: uuid::Uuid::new_v4(),
            connection_form: true,
            clear_credentials: false,
            populate_range: false,
            discard_confirm: false,
            notice: None,
            notice_tone: Tone::Info,
            error: None,
            saved: None,
            grid_scroll: gpui_kit::ScrollHandle::new(),
            today: None,
            scroll_to_today: false,
        }
    }
    fn set_notice(&mut self, tone: Tone, message: impl Into<String>) {
        self.notice = Some(message.into());
        self.notice_tone = tone;
    }
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.initialized && cx.has_global::<Runtime>() {
            self.connect(None, cx);
        }
    }
    fn entered_connection(&mut self, cx: &mut Context<Self>) {
        let values: Vec<_> = self
            .credentials
            .iter()
            .map(|field| field.read(cx).value().to_string())
            .collect();
        match Credentials::new_in_region(
            self.tempo_origin,
            &values[0],
            &values[1],
            &values[2],
            &values[3],
        ) {
            Ok(value) => self.connect(Some(value), cx),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn connect(&mut self, entered: Option<Credentials>, cx: &mut Context<Self>) {
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        if self.copy_plan.is_some() {
            self.error = Some(
                "Create or cancel the pending copies before checking another connection.".into(),
            );
            cx.notify();
            return;
        }
        let retaining_draft = self.editor.is_some();
        let previous = if retaining_draft {
            self.editor_binding
                .clone()
                .or_else(|| self.connection.as_ref().map(io::Connection::binding))
        } else {
            self.connection.as_ref().map(io::Connection::binding)
        };
        if retaining_draft && previous.is_none() {
            self.error = Some("The draft's original account connection is unavailable. Keep its inputs and reconnect the original account before continuing.".into());
            cx.notify();
            return;
        }
        self.initialized = true;
        self.busy = true;
        self.error = None;
        self.set_notice(
            Tone::Info,
            "Checking the Tempo connection and loading the account month…",
        );
        let retained = self.data.is_some();
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async move {
            let config = buddy_core::config::config_path()
                .map_err(|_| "The configuration profile is unavailable.".to_owned())?;
            io::connect(config, entered, previous, retaining_draft).await
        });
        cx.spawn(async move |this,cx| {
            let result=rx.await;
            this.update(cx,|this,cx| {
                if this.generation!=generation {return;}
                this.busy=false;this.notice=None;
                match result {
                    Ok(Ok(Some(connection)))=>{
                        this.tempo_origin=connection.session.tempo_origin();
                        this.delete_target=None;this.pending_editor=None;this.pending_template=None;this.templates=None;
                        this.data=Some(connection.session.snapshot().clone());
                        this.metadata=connection.metadata.clone();
                        this.error=connection.session.recovery_problem().map(|e|e.to_string());
                        if let Some(editor)=&mut this.editor {
                            editor.invalidate_connection();
                            this.editor_binding=Some(connection.binding());
                            this.set_notice(Tone::Warning,"Connection checked. Your draft was retained. Look up its issue and billing accounts again before saving.");
                        } else {this.editor_binding=None;}
                        this.connection=Some(connection);this.saved=Some(true);this.connection_form=false;this.clear_credentials=true;this.populate_range=true;this.scroll_to_today=true;this.page=0;
                    },
                    Ok(Ok(None))=>{this.connection_form=true;this.saved=Some(false);this.set_notice(Tone::Info,if retained {"No connection is saved. The displayed timesheet was retained. Reconnect to continue."} else {"Connect Tempo and Jira. No connection is saved for this profile."});},
                    Ok(Err(error))=>this.error=Some(error),
                    Err(_)=>this.error=Some("The connection check stopped. The displayed timesheet was retained. Recheck the saved connection.".into()),
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
    fn apply_range(&mut self, cx: &mut Context<Self>) {
        let parsed = (|| {
            let from = self
                .from
                .read(cx)
                .value()
                .parse()
                .map_err(|_| buddy_core::tempo::Error::InvalidRange)?;
            let to = self
                .to
                .read(cx)
                .value()
                .parse()
                .map_err(|_| buddy_core::tempo::Error::InvalidRange)?;
            DateRange::new(from, to)
        })();
        match parsed {
            Ok(range) => self.operate(Operation::Range(range), cx),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn forget_connection(&mut self, cx: &mut Context<Self>) {
        if self.busy
            || self.editor.is_some()
            || self.copy_plan.is_some()
            || !cx.has_global::<Runtime>()
        {
            return;
        }
        self.busy = true;
        self.error = None;
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async {
            let config = buddy_core::config::config_path()
                .map_err(|_| "The configuration profile is unavailable.".to_owned())?;
            credentials::clear_at(&config).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this,cx| {
            let result=rx.await;
            this.update(cx,|this,cx| {
                if this.generation!=generation {return;}
                this.busy=false;
                match result {
                    Ok(Ok(()))=>{this.connection=None;this.data=None;this.templates=None;this.pending_template=None;this.metadata=Arc::new(metadata::Metadata::default());this.editor_binding=None;this.connection_form=true;this.clear_credentials=true;this.saved=Some(false);this.set_notice(Tone::Success,"The saved Tempo and Jira credentials were removed. Connect again to load a timesheet.");},
                    Ok(Err(error))=>this.error=Some(error),
                    Err(_)=>this.error=Some("Credential removal is unconfirmed. Check secure storage before reconnecting.".into()),
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
}
