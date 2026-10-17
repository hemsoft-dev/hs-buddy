//! Calendar grouping uses one captured clock and its timezone for the whole view.
use chrono::{DateTime, Days, NaiveDate, TimeZone};

use super::{DueDate, Error, Id, Task};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DayKind {
    Overdue,
    Date(NaiveDate),
}
pub struct DayGroup {
    pub kind: DayKind,
    pub tasks: Vec<Task>,
}

fn active(task: &Task) -> bool {
    !task.completed && !task.deleted
}
fn sort(tasks: &mut [Task]) {
    tasks.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
}

/// Match the planner's calendar sections: earlier days are Overdue, and timed
/// tasks on the captured current day remain in Today even after their due time.
/// Undated tasks remain accessible through their project rather than acquiring
/// an invented due date. The range is exclusive of its end date.
pub fn schedule<Zone: TimeZone>(
    tasks: &[Task],
    now: &DateTime<Zone>,
    days: u8,
) -> Result<Vec<DayGroup>, Error> {
    let zone = now.timezone();
    schedule_by_day(tasks, now.date_naive(), days, |due| Ok(due.day_in(&zone)))
}

pub(super) fn schedule_by_day(
    tasks: &[Task],
    start: NaiveDate,
    days: u8,
    due_day: impl Fn(&DueDate) -> Result<NaiveDate, Error>,
) -> Result<Vec<DayGroup>, Error> {
    if !(1..=31).contains(&days) {
        return Err(Error::InvalidDraft);
    }
    let end = start
        .checked_add_days(Days::new(u64::from(days)))
        .ok_or(Error::InvalidDate)?;
    let mut overdue = Vec::new();
    let mut dated: Vec<_> = (0..days)
        .map(|offset| DayGroup {
            kind: DayKind::Date(
                start
                    .checked_add_days(Days::new(u64::from(offset)))
                    .expect("bounded date range"),
            ),
            tasks: Vec::new(),
        })
        .collect();
    for task in tasks.iter().filter(|task| active(task)) {
        let Some(due) = &task.due else { continue };
        let day = due_day(due)?;
        if day < start {
            overdue.push(task.clone());
        } else if day < end {
            let offset = (day - start).num_days() as usize;
            dated[offset].tasks.push(task.clone());
        }
    }
    for group in &mut dated {
        sort(&mut group.tasks);
    }
    if !overdue.is_empty() {
        sort(&mut overdue);
        dated.insert(
            0,
            DayGroup {
                kind: DayKind::Overdue,
                tasks: overdue,
            },
        );
    }
    Ok(dated)
}

pub fn project_tasks(tasks: &[Task], project: &Id) -> Vec<Task> {
    let mut rows: Vec<_> = tasks
        .iter()
        .filter(|task| active(task) && &task.project_id == project)
        .cloned()
        .collect();
    sort(&mut rows);
    rows
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
