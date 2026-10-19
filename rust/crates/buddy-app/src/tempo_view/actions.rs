use super::*;
use buddy_core::tempo::{AccountCalendar, Draft, EditorIssue, Worklog, WorklogId};

pub(super) enum Update {
    Message(Tone, String),
    TemplateEditor(Box<(Worklog, EditorIssue, chrono::NaiveDate)>),
    CopyReady(Box<buddy_core::tempo::CopyPlan>),
    Editor(Box<(Worklog, EditorIssue)>),
    Lookup(Box<(uuid::Uuid, String, EditorIssue)>),
    Wrote {
        confirmed: bool,
        tone: Tone,
        message: String,
        problem: Option<String>,
    },
}
impl TempoView {
    pub(super) fn can_submit(&self) -> bool {
        !self.busy
            && self
                .connection
                .as_ref()
                .is_some_and(|c| c.session.can_submit())
    }
    pub(super) fn open_new_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_submit() || self.editor.is_some() || self.copy_plan.is_some() {
            return;
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let data = connection.session.snapshot();
        let date = match AccountCalendar::from_zone(data.time_zone.as_deref())
            .and_then(|c| c.today_at(connection.now()))
        {
            Ok(date) => date,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.open_dated_editor(date, None, window, cx);
    }
    pub(super) fn lookup_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let id = editor.id;
        let query = editor.issue.read(cx).value().to_string();
        if query.trim().is_empty() {
            self.error = Some("Enter an issue key or ID before looking it up.".into());
            cx.notify();
            return;
        }
        self.operate(Operation::Lookup(id, query), cx);
    }
    pub(super) fn load_edit(&mut self, id: WorklogId, cx: &mut Context<Self>) {
        if !self.can_submit() || self.editor.is_some() || self.copy_plan.is_some() {
            return;
        }
        self.delete_target = None;
        self.editor_binding = self.connection.as_ref().map(io::Connection::binding);
        self.operate(Operation::Editor(id), cx);
    }
    pub(super) fn submit_editor(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        let (Some(editor), Some(data)) = (&self.editor, &self.data) else {
            return;
        };
        match editor.draft(&data.account, cx) {
            Ok(draft) => self.operate(Operation::Submit(Box::new(draft)), cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }
    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit() {
            return;
        }
        let Some(row) = &self.delete_target else {
            return;
        };
        match Draft::delete(row) {
            Ok(draft) => self.operate(Operation::Submit(Box::new(draft)), cx),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    pub(super) fn operate(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if self.copy_plan.is_some() && !matches!(operation, Operation::ExecuteCopy(_)) {
            self.error = Some("Create or cancel the pending copies before another action.".into());
            cx.notify();
            return;
        }
        if self.editor.is_some()
            && matches!(
                operation,
                Operation::Month(_)
                    | Operation::Today
                    | Operation::Range(_)
                    | Operation::Template(_, _)
            )
        {
            self.error = Some("Save or cancel the open draft before changing the displayed range. Refresh keeps its inputs.".into());
            cx.notify();
            return;
        }
        if self.busy || !cx.has_global::<Runtime>() {
            return;
        }
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.set_notice(Tone::Info, "Working with Tempo…");
        self.discard_confirm = false;
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async move {
            let result = io::apply(&mut connection, operation).await;
            (connection, result)
        });
        cx.spawn(async move |this,cx| {
            let result=rx.await;
            this.update(cx,|this,cx| {
                if this.generation!=generation {return;}
                this.busy=false;this.notice=None;
                match result {
                    Ok((connection,outcome))=>{
                        this.data=Some(connection.session.snapshot().clone());this.metadata=connection.metadata.clone();this.templates=connection.templates.clone();this.connection=Some(connection);
                        match outcome {
                            Ok(Update::Message(tone,message))=>{this.set_notice(tone,message);this.populate_range=true;this.scroll_to_today=true;this.page=0;this.cell_filter=None;},
                            Ok(Update::CopyReady(plan))=>{this.copy_plan=Some(plan);this.delete_target=None;},
                            Ok(Update::TemplateEditor(value))=>this.pending_template=Some(*value),
                            Ok(Update::Editor(value))=>this.pending_editor=Some(*value),
                            Ok(Update::Lookup(value))=>{
                                let (id,query,info)=*value;
                                if let Some(editor)=&mut this.editor && editor.id==id
                                    && let Err(error)=editor.accept_lookup(query,info,cx) {this.error=Some(error);}
                            },
                            Ok(Update::Wrote{confirmed,tone,message,problem})=>{
                                if confirmed {this.editor=None;this.editor_binding=None;this.delete_target=None;}
                                this.set_notice(tone,message);this.error=problem;this.populate_range=true;this.scroll_to_today=true;this.cell_filter=None;this.page=0;
                            },
                            Err(error)=>this.error=Some(error),
                        }
                    },
                    Err(_)=>this.error=Some("The worker stopped. The displayed timesheet and draft were retained. Recheck the original connection and its recovery before continuing.".into()),
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
}
