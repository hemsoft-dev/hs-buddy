use std::collections::{HashMap, HashSet};

use jiff::{Span, ToSpan, civil::Date};

use super::{AccountId, DaySchedule, Error, Worklog};

#[derive(Clone, Copy)]
pub struct DateRange {
    pub start: Date,
    pub end: Date,
}
impl DateRange {
    pub fn new(start: Date, end: Date) -> Result<Self, Error> {
        if start > end
            || (end - start).get_days() >= 366
            || !(1..=9999).contains(&start.year())
            || !(1..=9999).contains(&end.year())
        {
            return Err(Error::InvalidRange);
        }
        Ok(Self { start, end })
    }
    pub fn month_containing(day: Date) -> Result<Self, Error> {
        Self::new(day.first_of_month(), day.last_of_month())
    }
    /// Whole months only. A custom range must not silently become another view.
    pub fn shift_month(&self, delta: i32) -> Result<Self, Error> {
        let original = Self::month_containing(self.start)?;
        if self.start != original.start || self.end != original.end {
            return Err(Error::InvalidRange);
        }
        let day = Span::new()
            .try_months(delta)
            .and_then(|months| self.start.checked_add(months))
            .map_err(|_| Error::InvalidRange)?;
        Self::month_containing(day)
    }
    pub fn week_containing(day: Date) -> Result<Self, Error> {
        let start = day
            .checked_sub(i64::from(day.weekday().to_monday_zero_offset()).days())
            .map_err(|_| Error::InvalidRange)?;
        Self::new(
            start,
            start
                .checked_add(6.days())
                .map_err(|_| Error::InvalidRange)?,
        )
    }
    pub fn contains(&self, day: Date) -> bool {
        self.start <= day && day <= self.end
    }
}
pub struct DayTotal {
    pub date: Date,
    pub seconds: u64,
    pub worklogs: Vec<Worklog>,
}

pub struct SummaryDay {
    pub logged: DayTotal,
    /// None means unavailable, not a scheduled zero-hour day.
    pub schedule: Option<DaySchedule>,
}
impl SummaryDay {
    pub fn balance_seconds(&self) -> Option<i128> {
        self.schedule
            .as_ref()
            .map(|schedule| i128::from(self.logged.seconds) - i128::from(schedule.required_seconds))
    }
}
pub struct TimesheetSummary {
    pub days: Vec<SummaryDay>,
    pub logged_seconds: u64,
    /// Available only when the service supplied every selected day's schedule.
    pub required_seconds: Option<u64>,
}

pub fn summarize(
    rows: &[Worklog],
    author: &AccountId,
    range: DateRange,
    schedules: &[DaySchedule],
) -> Result<TimesheetSummary, Error> {
    let days = day_totals(rows, author, range)?;
    let mut by_date = HashMap::new();
    let mut required = 0_u64;
    for schedule in schedules {
        if !range.contains(schedule.date) {
            return Err(Error::WrongScope);
        }
        if by_date.insert(schedule.date, schedule).is_some() {
            return Err(Error::InvalidResponse);
        }
        required = required
            .checked_add(schedule.required_seconds)
            .ok_or(Error::TotalOverflow)?;
    }
    let mut logged_seconds = 0_u64;
    let complete = by_date.len() == days.len();
    let mut summary = Vec::with_capacity(days.len());
    for logged in days {
        logged_seconds = logged_seconds
            .checked_add(logged.seconds)
            .ok_or(Error::TotalOverflow)?;
        let schedule = by_date.get(&logged.date).map(|value| (*value).clone());
        summary.push(SummaryDay { logged, schedule });
    }
    Ok(TimesheetSummary {
        days: summary,
        logged_seconds,
        required_seconds: complete.then_some(required),
    })
}

/// Do not shift a service worklog to another day based on its UTC timestamp.
/// Integer seconds are summed before any presentation rounding.
pub fn day_totals(
    rows: &[Worklog],
    author: &AccountId,
    range: DateRange,
) -> Result<Vec<DayTotal>, Error> {
    let range = DateRange::new(range.start, range.end)?;
    let mut days: Vec<_> = range
        .start
        .series(1.day())
        .take(((range.end - range.start).get_days() + 1) as usize)
        .map(|date| DayTotal {
            date,
            seconds: 0,
            worklogs: Vec::new(),
        })
        .collect();
    let positions: HashMap<_, _> = days
        .iter()
        .enumerate()
        .map(|(index, day)| (day.date, index))
        .collect();
    let mut identities = HashSet::new();
    for row in rows {
        if &row.author != author || !range.contains(row.date) {
            return Err(Error::WrongScope);
        }
        if !identities.insert(row.id) {
            return Err(Error::InvalidResponse);
        }
        let day = &mut days[positions[&row.date]];
        day.seconds = day
            .seconds
            .checked_add(row.seconds)
            .ok_or(Error::TotalOverflow)?;
        day.worklogs.push(row.clone());
    }
    for day in &mut days {
        day.worklogs
            .sort_by_key(|row| (row.start_time, row.id.get()));
    }
    Ok(days)
}
