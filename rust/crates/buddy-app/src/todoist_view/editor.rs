use super::*;
use buddy_core::todoist::{CreateDraft, EditDraft, Priority, Task, WriteKind};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::{AnyElement, InteractiveElement, StatefulInteractiveElement};
#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;

pub(super) struct Editor {
    target: Option<Id>,
    project: Option<Id>,
    content: Entity<InputState>,
    description: Entity<TextareaState>,
    due: Entity<InputState>,
    original_due: Option<String>,
    replace_due: bool,
    priority: Priority,
}

impl Editor {
    pub fn new(
        task: Option<&Task>,
        project: Option<Id>,
        window: &mut Window,
        cx: &mut Context<TodoistView>,
    ) -> Self {
        let content = cx.new(|cx| InputState::new(window, cx).placeholder("Enter a task name"));
        let description = cx.new(|cx| TextareaState::new(window, cx).placeholder("Add details"));
        let due = cx.new(|cx| InputState::new(window, cx).placeholder("YYYY-MM-DD"));
        if let Some(task) = task {
            content.update(cx, |state, cx| {
                state.set_value(task.content.clone(), window, cx)
            });
            description.update(cx, |state, cx| {
                state.set_value(task.description.clone(), window, cx)
            });
        }
        for input in [&content, &due] {
            cx.subscribe(input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
        }
        cx.subscribe(&description, |_, _, _: &InputEvent, cx| cx.notify())
            .detach();
        Self {
            target: task.map(|task| task.id.clone()),
            project,
            content,
            description,
            due,
            original_due: task.and_then(|task| task.due.as_ref()).map(|due| {
                format!(
                    "{}{}",
                    due.source_datetime.as_deref().unwrap_or(&due.source_date),
                    if due.recurring { " · recurring" } else { "" }
                )
            }),
            replace_due: task.is_none(),
            priority: task
                .map(|task| task.priority)
                .unwrap_or_else(|| Priority::from_api(1).expect("P4")),
        }
    }

    fn change(&self, cx: &Context<TodoistView>) -> Result<TaskChange, String> {
        let content = self.content.read(cx).value().to_string();
        if content.trim().is_empty() {
            return Err("Enter a task name before saving.".into());
        }
        let description = self.description.read(cx).value().to_string();
        let due_text = self.due.read(cx).value().to_string();
        let due = if self.replace_due && !due_text.is_empty() {
            if due_text.len() != 10 {
                return Err("Enter a due date as YYYY-MM-DD.".into());
            }
            Some(
                chrono::NaiveDate::parse_from_str(&due_text, "%Y-%m-%d")
                    .map_err(|_| "Enter a valid due date as YYYY-MM-DD.".to_owned())?,
            )
        } else {
            None
        };
        if self.target.is_some() && self.replace_due && due.is_none() {
            return Err("Enter the replacement date, or choose Keep due date to preserve the existing schedule.".into());
        }
        Ok(match &self.target {
            Some(id) => TaskChange::Edit(
                id.clone(),
                EditDraft {
                    content: Some(content),
                    description: Some(description),
                    priority: Some(self.priority),
                    due,
                },
            ),
            None => TaskChange::Create(CreateDraft {
                content,
                description,
                project: self.project.clone(),
                priority: self.priority,
                due,
            }),
        })
    }
}

impl TodoistView {
    pub(super) fn open_editor(
        &mut self,
        task: Option<Task>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !match &task {
            Some(task) => self.can_task(&task.id, WriteKind::Edit),
            None => self.can_create(),
        } {
            return;
        }
        self.editor = Some(Editor::new(task.as_ref(), self.project.clone(), window, cx));
        self.delete_target = None;
        cx.notify();
    }

    pub(super) fn render_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = &self.editor else {
            return div().into_any_element();
        };
        let selected_project = editor.project.as_ref().and_then(|id| {
            self.data
                .as_ref()?
                .projects
                .iter()
                .find(|project| &project.id == id && !project.archived && !project.deleted)
        });
        let unavailable_project =
            editor.target.is_none() && editor.project.is_some() && selected_project.is_none();
        let permitted = match &editor.target {
            Some(id) => self.can_task(id, WriteKind::Edit),
            None => {
                !self.busy
                    && self
                        .planner
                        .as_ref()
                        .is_some_and(|planner| planner.can_create_in(editor.project.as_ref()))
            }
        };
        let mut priorities = h_flex().flex_wrap().gap_2();
        for value in (1..=4).rev() {
            let priority = Priority::from_api(value).expect("bounded priority");
            priorities = priorities.child(
                Button::new(("todoist-priority", usize::from(value)))
                    .label(priority.label())
                    .selected(editor.priority == priority)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(editor) = &mut this.editor {
                            editor.priority = priority;
                        }
                        cx.notify();
                    })),
            );
        }
        let mut form = v_flex()
            .gap_3()
            .child(div().text_lg().child(if editor.target.is_some() {
                "Edit task"
            } else {
                "New task"
            }))
            .child(div().child("Task name"))
            .child(Input::new(&editor.content).disabled(self.busy))
            .child(div().child("Description"))
            .child(
                div()
                    .h_24()
                    .child(Textarea::new(&editor.description).disabled(self.busy)),
            )
            .child(div().child("Priority"))
            .child(priorities);
        if editor.target.is_some() {
            form = form
                .child(div().whitespace_normal().child(format!(
                    "Current due date: {}. Keeping it preserves its time and recurrence.",
                    editor.original_due.as_deref().unwrap_or("No due date")
                )))
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("todoist-preserve-due")
                                .label("Keep due date")
                                .selected(!editor.replace_due)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor {
                                        editor.replace_due = false;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("todoist-replace-due")
                                .label("Replace with date")
                                .selected(editor.replace_due)
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(editor) = &mut this.editor {
                                        editor.replace_due = true;
                                    }
                                    cx.notify();
                                })),
                        ),
                )
                .child(div().whitespace_normal().child(
                    "Replacing uses a date only and replaces any existing time or recurrence.",
                ));
        } else {
            let project = selected_project
                .map(|project| project.name.as_str())
                .unwrap_or(if unavailable_project {
                    "Unavailable project"
                } else {
                    "Todoist Inbox"
                });
            form = form.child(div().whitespace_normal().child(format!(
                "Project: {project}. To choose another project, cancel and select it in Projects."
            )));
        }
        if editor.replace_due {
            form = form
                .child(div().child(if editor.target.is_some() {
                    "Replacement due date"
                } else {
                    "Due date, optional"
                }))
                .child(Input::new(&editor.due).disabled(self.busy));
        }
        form.child(
            h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("todoist-save-task")
                        .label("Save task")
                        .disabled(!permitted || unavailable_project)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let change = this.editor.as_ref().expect("visible editor").change(cx);
                            match change {
                                Ok(change) => this.operate(Operation::Submit(change), cx),
                                Err(error) => {
                                    this.error = Some(error);
                                    cx.notify();
                                }
                            }
                        })),
                )
                .child(
                    Button::new("todoist-cancel-editor")
                        .label("Cancel")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.editor = None;
                            cx.notify();
                        })),
                ),
        )
        .id("todoist-editor-scroll")
        .overflow_y_scroll()
        .flex_1()
        .min_h_0()
        .w_full()
        .into_any_element()
    }
}
