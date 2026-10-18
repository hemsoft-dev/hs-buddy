use super::*;
use buddy_core::todoist::{DayKind, Task, WriteKind, project_tasks};
use gpui_kit::{AnyElement, InteractiveElement, StatefulInteractiveElement};

const PAGE: usize = 30;
const PROJECT_PAGE: usize = 8;

impl TodoistView {
    pub(super) fn can_write(&self) -> bool {
        !self.busy && self.data.is_some() && self.planner.as_ref().is_some_and(Planner::can_submit)
    }

    pub(super) fn can_create(&self) -> bool {
        self.can_write()
            && self
                .planner
                .as_ref()
                .is_some_and(|planner| planner.can_create_in(self.project.as_ref()))
    }

    pub(super) fn can_task(&self, id: &Id, action: WriteKind) -> bool {
        self.can_write()
            && self
                .planner
                .as_ref()
                .is_some_and(|planner| planner.can_change_task(id, action))
    }

    pub(super) fn select_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        if pane != Pane::Projects
            && self
                .data
                .as_ref()
                .is_some_and(|data| matches!(data.coverage, Coverage::Project(_)))
        {
            self.operate(Operation::Load(Coverage::Calendar, pane, None), cx);
        } else {
            self.pane = pane;
            if pane == Pane::Projects
                && self
                    .data
                    .as_ref()
                    .is_none_or(|data| data.coverage == Coverage::Calendar)
            {
                self.project = None;
            }
            self.task_page = 0;
            self.delete_target = None;
            cx.notify();
        }
    }

    fn select_project(&mut self, project: Id, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.data.as_ref().is_some_and(|data| {
            data.coverage == Coverage::All || data.coverage == Coverage::Project(project.clone())
        }) {
            self.project = Some(project);
            self.task_page = 0;
            self.delete_target = None;
            cx.notify();
        } else {
            self.operate(
                Operation::Load(
                    Coverage::Project(project.clone()),
                    Pane::Projects,
                    Some(project),
                ),
                cx,
            );
        }
    }

    pub(super) fn render_task_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = v_flex().gap_3();
        if self.catalog.is_some() {
            let coverage = self.data.as_ref().map(|data| &data.coverage);
            body = body.child(div().whitespace_normal().child(match coverage {
                Some(Coverage::All) => "Loaded scope: all active tasks.",
                Some(Coverage::Calendar) => "Loaded scope: overdue tasks and the next seven days. Open a project to include its undated tasks.",
                Some(Coverage::Project(_)) => "Loaded scope: the selected project, including undated tasks.",
                None => "Choose calendar tasks or a project to load a smaller scope. No partial task list was loaded.",
            })).child(h_flex().flex_wrap().gap_2()
                .child(Button::new("todoist-load-calendar").label("Load calendar tasks").disabled(self.busy)
                    .on_click(cx.listener(|this,_,_,cx| this.operate(Operation::Load(Coverage::Calendar,Pane::Upcoming,None),cx))))
                .child(Button::new("todoist-load-all").label("Load all tasks").disabled(self.busy)
                    .on_click(cx.listener(|this,_,_,cx| this.operate(Operation::Load(Coverage::All,Pane::Today,None),cx)))));
        }
        let available_projects = self
            .catalog
            .as_ref()
            .map(|catalog| catalog.projects.as_slice())
            .or_else(|| self.data.as_ref().map(|data| data.projects.as_slice()))
            .unwrap_or(&[]);
        if self.pane == Pane::Projects {
            let projects: Vec<_> = available_projects
                .iter()
                .filter(|project| !project.archived && !project.deleted)
                .collect();
            let mut choices = h_flex().flex_wrap().gap_2();
            for project in projects
                .iter()
                .skip(self.project_page * PROJECT_PAGE)
                .take(PROJECT_PAGE)
            {
                let id = project.id.clone();
                choices = choices.child(
                    Button::new(format!("todoist-project-{}", id.as_str()))
                        .label(project.name.clone())
                        .selected(self.project.as_ref() == Some(&id))
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_project(id.clone(), cx);
                        })),
                );
            }
            body = body.child(choices).child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("todoist-project-prev")
                            .label("Previous projects")
                            .disabled(self.project_page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project_page = this.project_page.saturating_sub(1);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("todoist-project-next")
                            .label("Next projects")
                            .disabled((self.project_page + 1) * PROJECT_PAGE >= projects.len())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project_page += 1;
                                cx.notify();
                            })),
                    ),
            );
        }
        body.into_any_element()
    }

    pub(super) fn render_tasks(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = v_flex().gap_3().flex_1().min_h_0();
        if self.pane == Pane::Projects && self.project.is_none() {
            let projects = self
                .catalog
                .as_ref()
                .map(|catalog| catalog.projects.as_slice())
                .or_else(|| self.data.as_ref().map(|data| data.projects.as_slice()))
                .unwrap_or(&[]);
            return body
                .child(
                    div().child(
                        if !projects
                            .iter()
                            .any(|project| !project.archived && !project.deleted)
                        {
                            "No active projects."
                        } else {
                            "Choose a project to see its tasks, including tasks without a due date."
                        },
                    ),
                )
                .into_any_element();
        }
        let Some(data) = &self.data else {
            return body
                .child(div().child(if self.busy {
                    "Loading tasks…"
                } else {
                    "Connect or choose a scope to load your tasks."
                }))
                .into_any_element();
        };
        let project_by_id: std::collections::HashMap<_, _> = data
            .projects
            .iter()
            .map(|project| (&project.id, project))
            .collect();
        if self.pane == Pane::Projects
            && let Some(project) = self.project.as_ref().and_then(|id| project_by_id.get(id))
            && let Some(notice) = project.access_notice()
        {
            body = body.child(div().whitespace_normal().child(notice));
        }
        let mut rows: Vec<(String, Task)> = Vec::new();
        if self.pane == Pane::Projects {
            if let Some(project) = &self.project {
                rows = project_tasks(&data.tasks, project)
                    .into_iter()
                    .map(|task| ("Project tasks".to_owned(), task))
                    .collect();
            }
        } else {
            match data.calendar.schedule(
                &data.tasks,
                data.now(),
                if self.pane == Pane::Today { 1 } else { 7 },
            ) {
                Ok(groups) => {
                    for group in groups {
                        let label = match group.kind {
                            DayKind::Overdue => "Overdue".to_owned(),
                            DayKind::Date(day) => day.format("%a, %b %-d, %Y").to_string(),
                        };
                        rows.extend(group.tasks.into_iter().map(|task| (label.clone(), task)));
                    }
                }
                Err(error) => {
                    return body
                        .child(div().child(error.to_string()))
                        .into_any_element();
                }
            }
        }
        let needle = self.filter.read(cx).value().to_lowercase();
        rows.retain(|(_, task)| {
            task.content.to_lowercase().contains(&needle)
                || task.description.to_lowercase().contains(&needle)
                || task
                    .labels
                    .iter()
                    .any(|label| label.to_lowercase().contains(&needle))
                || project_by_id
                    .get(&task.project_id)
                    .is_some_and(|project| project.name.to_lowercase().contains(&needle))
        });
        let page = self.task_page.min(rows.len().saturating_sub(1) / PAGE);
        let mut list = v_flex()
            .gap_3()
            // Each task scope starts at its own first row. A stable shared ID
            // carries Upcoming's offset into an already selected project.
            .id(format!(
                "todoist-task-list-{}-{}-{page}",
                match self.pane {
                    Pane::Today => "today",
                    Pane::Upcoming => "upcoming",
                    Pane::Projects => "projects",
                },
                self.project.as_ref().map(Id::as_str).unwrap_or("")
            ))
            .debug_selector(|| "todoist-task-list-viewport".into())
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full();
        let mut previous_group = String::new();
        for (index, (group, task)) in rows.iter().skip(page * PAGE).take(PAGE).enumerate() {
            if group != &previous_group {
                list = list.child(div().text_lg().child(group.clone()));
                previous_group = group.clone();
            }
            let project = project_by_id
                .get(&task.project_id)
                .map(|project| project.name.as_str())
                .unwrap_or("Unknown project");
            let due = task
                .due
                .as_ref()
                .map(|due| {
                    format!(
                        "{}{}",
                        due.source_datetime.as_deref().unwrap_or(&due.source_date),
                        if due.recurring { " · recurring" } else { "" }
                    )
                })
                .unwrap_or_else(|| "No due date".into());
            let edit = task.clone();
            let complete = task.id.clone();
            let delete = task.id.clone();
            let mut row = v_flex()
                .debug_selector(move || format!("todoist-task-row-{index}"))
                .gap_2()
                .pb_3()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(div().whitespace_normal().child(task.content.clone()))
                .child(
                    div()
                        .whitespace_normal()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} · {project} · {due}", task.priority.label())),
                );
            if !task.labels.is_empty() {
                row = row.child(
                    div()
                        .whitespace_normal()
                        .child(format!("Labels: {}", task.labels.join(", "))),
                );
            }
            row = row.child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new(format!("todoist-complete-{}", task.id.as_str()))
                            .label("Complete")
                            .disabled(!self.can_task(&task.id, WriteKind::Complete))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.operate(
                                    Operation::Submit(TaskChange::Complete(complete.clone())),
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new(format!("todoist-edit-{}", task.id.as_str()))
                            .label("Edit")
                            .disabled(!self.can_task(&task.id, WriteKind::Edit))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_editor(Some(edit.clone()), window, cx)
                            })),
                    )
                    .child(
                        Button::new(format!("todoist-delete-{}", task.id.as_str()))
                            .label("Delete")
                            .disabled(!self.can_task(&task.id, WriteKind::Delete))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_target = Some(delete.clone());
                                cx.notify();
                            })),
                    ),
            );
            if self.delete_target.as_ref() == Some(&task.id) {
                let target = task.id.clone();
                row = row
                    .child(
                        div()
                            .whitespace_normal()
                            .child("Delete this task from Todoist? This cannot be undone."),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("todoist-confirm-delete")
                                    .label("Confirm deletion")
                                    .disabled(!self.can_task(&task.id, WriteKind::Delete))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_target = None;
                                        this.operate(
                                            Operation::Submit(TaskChange::Delete(target.clone())),
                                            cx,
                                        );
                                    })),
                            )
                            .child(
                                Button::new("todoist-cancel-delete")
                                    .label("Keep task")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.delete_target = None;
                                        cx.notify();
                                    })),
                            ),
                    );
            }
            list = list.child(row);
        }
        if rows.is_empty() {
            list = list.child(div().child(if needle.is_empty() {
                "No tasks in this view."
            } else {
                "No tasks match this filter."
            }));
        }
        body.child(list)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("todoist-task-prev")
                            .label("Previous tasks")
                            .disabled(page == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.task_page = this.task_page.saturating_sub(1);
                                this.delete_target = None;
                                cx.notify();
                            })),
                    )
                    .child(div().child(format!("{} tasks · page {}", rows.len(), page + 1)))
                    .child(
                        Button::new("todoist-task-next")
                            .label("Next tasks")
                            .disabled((page + 1) * PAGE >= rows.len())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.task_page += 1;
                                this.delete_target = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}
