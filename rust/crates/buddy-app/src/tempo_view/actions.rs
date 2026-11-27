use super::*;
use buddy_core::tempo::{AccountCalendar, Draft, EditorIssue, Worklog, WorklogId};

pub(super) enum Update {
    Message(Tone, String),
    TemplateEditor(Box<(Worklog, EditorIssue, jiff::civil::Date)>),
    CopyReady(Box<buddy_core::tempo::CopyPlan>),
    Editor(Box<(Worklog, EditorIssue)>),
    Lookup(Box<(uuid::Uuid, String, EditorIssue)>),
    Wrote {
        confirmed: bool,
        /// Sent without a reply: Tempo may have applied it.
        unconfirmed: bool,
        tone: Tone,
        message: String,
        problem: Option<String>,
    },
}
impl TempoView {
    pub(super) fn can_submit(&self) -> bool {
        #[cfg(test)]
        if self.test_can_submit {
            return !self.busy;
        }
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
    /// List every Tempo account in a new draft before its issue is looked up.
    /// The read shares the session's client but not the session, so the
    /// draft stays editable while it loads; the result applies only to the
    /// same draft on the same connection.
    pub(super) fn load_billing_catalog(&mut self, cx: &mut Context<Self>) {
        let (Some(editor), Some(connection)) = (&self.editor, &self.connection) else {
            return;
        };
        if !cx.has_global::<Runtime>() {
            return;
        }
        let editor_id = editor.id;
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(connection.session.billing_catalog());
        cx.spawn(async move |this, cx| {
            let result = match rx.await {
                Ok(result) => result.map_err(|e| e.to_string()),
                Err(_) => Err("The worker stopped before the accounts loaded.".into()),
            };
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some(editor) = &mut this.editor {
                    editor.accept_catalog(editor_id, result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
    /// A write's outcome. A confirmed one closes its editor or delete
    /// prompt; an unanswered delete of the prompt's worklog leaves the
    /// prompt saying so, whichever control sent it.
    pub(super) fn wrote(
        &mut self,
        confirmed: bool,
        unconfirmed: bool,
        tone: Tone,
        message: String,
        problem: Option<String>,
    ) {
        if confirmed {
            self.editor = None;
            self.editor_binding = None;
            self.delete_target = None;
        }
        if unconfirmed && self.delete_in_flight() {
            self.delete_unconfirmed = true;
        }
        self.set_notice(tone, message);
        self.error = problem;
        self.populate_range = true;
        self.scroll_to_today = true;
        self.cell_filter = None;
        self.page = 0;
    }
    pub(super) fn operate(&mut self, operation: Operation, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.test_operation = Some(std::mem::discriminant(&operation));
        }
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
        self.begin(&operation, connection.session.pending_draft());
        let generation = self.generation;
        let rx = Runtime::global(cx).spawn(async move {
            let result = io::apply(&mut connection, operation).await;
            (connection, result)
        });
        cx.spawn(async move |this,cx| {
            let result=rx.await;
            this.update(cx,|this,cx| {
                if this.generation!=generation {return;}
                let outcome=match result {
                    Ok((connection,outcome))=>{
                        this.data=Some(connection.session.snapshot().clone());this.metadata=connection.metadata.clone();this.templates=connection.templates.clone();this.connection=Some(connection);
                        outcome
                    },
                    Err(_)=>Err("The worker stopped. The displayed timesheet and draft were retained. Recheck the original connection and its recovery before continuing.".into()),
                };
                this.settle(outcome,cx);
            }).ok();
        }).detach();
        cx.notify();
    }
    /// Every operation starts here, whichever control sent it, so the view's
    /// in-flight state is the operation's own. `pending` is the captured
    /// change that `Operation::RetryPrepared` sends.
    pub(super) fn begin(&mut self, operation: &Operation, pending: Option<&Draft>) {
        // A lookup moves its draft to a new generation; only the response
        // `settle` pairs with it can resolve the draft.
        self.lookup_in_flight = match operation {
            Operation::Lookup(id, _) => self
                .editor
                .as_mut()
                .filter(|editor| editor.id == *id)
                .map(|editor| (*id, editor.begin_lookup())),
            _ => None,
        };
        self.busy = true;
        self.deleting = deletes(operation, pending);
        self.error = None;
        self.set_notice(Tone::Info, "Working with Tempo…");
        self.discard_confirm = false;
    }
    /// The end of the operation `begin` started.
    pub(super) fn settle(&mut self, outcome: Result<Update, String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.notice = None;
        let lookup = self.lookup_in_flight.take();
        match outcome {
            Ok(Update::Message(tone, message)) => {
                self.set_notice(tone, message);
                self.populate_range = true;
                self.scroll_to_today = true;
                self.page = 0;
                self.cell_filter = None;
            }
            Ok(Update::CopyReady(plan)) => {
                self.copy_plan = Some(plan);
                self.delete_target = None;
            }
            Ok(Update::TemplateEditor(value)) => self.pending_template = Some(*value),
            Ok(Update::Editor(value)) => self.pending_editor = Some(*value),
            Ok(Update::Lookup(value)) => {
                let (id, query, info) = *value;
                if let Some((sent_for, generation)) = lookup
                    && sent_for == id
                    && let Some(editor) = &mut self.editor
                    && editor.id == id
                    && let Err(error) = editor.settle_lookup(generation, query, Ok(info), cx)
                {
                    self.error = Some(error);
                }
            }
            Ok(Update::Wrote {
                confirmed,
                unconfirmed,
                tone,
                message,
                problem,
            }) => self.wrote(confirmed, unconfirmed, tone, message, problem),
            Err(error) => {
                // A failed lookup takes away the draft's earlier resolution
                // (I1): nothing it listed can be saved now.
                if let Some((id, generation)) = lookup
                    && let Some(editor) = &mut self.editor
                    && editor.id == id
                {
                    let _ = editor.settle_lookup(generation, String::new(), Err(error.clone()), cx);
                }
                self.error = Some(error);
            }
        }
        self.deleting = None;
        cx.notify();
    }
    /// The open prompt's worklog is being deleted, by Confirm deletion or by
    /// Retry captured change.
    pub(super) fn delete_in_flight(&self) -> bool {
        self.deleting
            .is_some_and(|id| self.delete_target.as_ref().is_some_and(|row| row.id == id))
    }
    /// The phase of a captured change that deletes the open prompt's
    /// worklog, for example one whose preflight failed. Its recovery
    /// controls then belong in the prompt as well as on the page.
    pub(super) fn target_recovery(&self) -> Option<WritePhase> {
        let target = self.delete_target.as_ref()?.id;
        #[cfg(test)]
        if let Some((phase, id)) = self.test_pending {
            return (id == target).then_some(phase);
        }
        let session = &self.connection.as_ref()?.session;
        (deletes(&Operation::RetryPrepared, session.pending_draft()) == Some(target))
            .then(|| session.pending_phase())
            .flatten()
    }
}

/// The worklog `operation` deletes. Retry captured change sends `pending`.
fn deletes(operation: &Operation, pending: Option<&Draft>) -> Option<WorklogId> {
    let draft = match operation {
        Operation::Submit(draft) => draft,
        Operation::RetryPrepared => pending?,
        _ => return None,
    };
    (draft.kind() == buddy_core::tempo::DraftKind::Delete)
        .then(|| draft.worklog_id())
        .flatten()
}
