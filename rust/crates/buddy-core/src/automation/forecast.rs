//! "Upcoming Scheduled Jobs": `ScheduleOverviewPanel.tsx` without its view.
//!
//! Every enabled schedule's cron occurrences after `now` and within the
//! forecast window, grouped by the viewer's calendar day. A schedule without a
//! timezone reads in the viewer's zone, as Electron passes no zone to
//! `cron-parser` then; an invalid expression or zone contributes nothing.
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use super::cron::Cron;
use super::models::Schedule;

/// `DAYS_OPTIONS`: the forecast windows Electron's dropdown offers.
pub const DAY_OPTIONS: [u32; 7] = [1, 2, 3, 5, 7, 14, 30];
/// `automation.scheduleForecastDays` when it is missing or out of range.
pub const DEFAULT_DAYS: f64 = 3.0;
/// The limit each schedule's enumeration passes to `enumerateCronOccurrences`.
pub const PER_SCHEDULE_LIMIT: usize = 500;
const DAY_MS: f64 = 86_400_000.0;

/// `normalizeForecastDays`: a window of 1 to 30 days, otherwise 3.
pub fn normalize_days(days: f64) -> f64 {
    if (1.0..=30.0).contains(&days) {
        days
    } else {
        DEFAULT_DAYS
    }
}

/// `"{n} {noun}"`, with Electron's plural `s` for anything but one, and
/// the number as JavaScript prints it (`3`, `2.5`).
pub fn count(n: f64, noun: &str) -> String {
    let number = if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        n.to_string()
    };
    let plural = if n == 1.0 { "" } else { "s" };
    format!("{number} {noun}{plural}")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub schedule_id: String,
    pub schedule_name: String,
    pub job_id: String,
    pub cron: String,
    /// The occurrence in the viewer's zone.
    pub at: jiff::Zoned,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DayGroup {
    pub date: Date,
    /// `Today`, `Tomorrow`, or `Wednesday, Nov 25` (English, as in Electron).
    pub label: String,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    pub days: f64,
    pub schedules: usize,
    pub active: usize,
    pub paused: usize,
    pub groups: Vec<DayGroup>,
}

impl Forecast {
    pub fn runs(&self) -> usize {
        self.groups
            .iter()
            .map(|group| group.occurrences.len())
            .sum()
    }

    /// `ScheduleOverviewSummary`: active, paused (only when any) and runs.
    pub fn summary(&self) -> Vec<String> {
        let mut parts = vec![count(self.active as f64, "active schedule")];
        if self.paused > 0 {
            parts.push(format!("{} paused", self.paused));
        }
        parts.push(format!(
            "{} in next {}",
            count(self.runs() as f64, "run"),
            count(self.days, "day")
        ));
        parts
    }

    /// `ScheduleEmptyState`: the message and, when one applies, its hint.
    pub fn empty(&self) -> (String, Option<&'static str>) {
        let message = format!("No scheduled runs in the next {}.", count(self.days, "day"));
        let hint = if self.schedules == 0 {
            Some("Create a schedule to get started.")
        } else if self.active == 0 {
            Some("All schedules are currently paused.")
        } else {
            None
        };
        (message, hint)
    }
}

/// `getDayLabel`: relative to the viewer's today.
fn day_label(date: Date, today: Date) -> String {
    if date == today {
        "Today".into()
    } else if today.tomorrow().ok() == Some(date) {
        "Tomorrow".into()
    } else {
        date.strftime("%A, %b %-d").to_string()
    }
}

/// `computeDayGroups` for `schedules` at `now`, seen from `viewer`.
pub fn forecast(schedules: &[Schedule], now: Timestamp, viewer: &TimeZone, days: f64) -> Forecast {
    let days = normalize_days(days);
    let active = schedules.iter().filter(|schedule| schedule.enabled).count();
    let mut forecast = Forecast {
        days,
        schedules: schedules.len(),
        active,
        paused: schedules.len() - active,
        groups: Vec::new(),
    };
    let Ok(end) = Timestamp::from_millisecond(
        now.as_millisecond()
            .saturating_add((days * DAY_MS).round() as i64),
    ) else {
        return forecast;
    };
    let mut all: Vec<(Timestamp, &Schedule)> = Vec::new();
    for schedule in schedules.iter().filter(|schedule| schedule.enabled) {
        let zone = match schedule.timezone.as_deref().filter(|zone| !zone.is_empty()) {
            None => viewer.clone(),
            Some(name) => match TimeZone::get(name) {
                Ok(zone) => zone,
                Err(_) => continue,
            },
        };
        let Ok(cron) = Cron::parse(&schedule.cron) else {
            continue;
        };
        all.extend(
            cron.occurrences(&zone, now, end, PER_SCHEDULE_LIMIT)
                .into_iter()
                .map(|time| (time, schedule)),
        );
    }
    // Stable, so schedules at the same minute keep their list order.
    all.sort_by_key(|(time, _)| *time);
    let today = now.to_zoned(viewer.clone()).date();
    for (time, schedule) in all {
        let at = time.to_zoned(viewer.clone());
        let date = at.date();
        if forecast.groups.last().map(|group| group.date) != Some(date) {
            forecast.groups.push(DayGroup {
                date,
                label: day_label(date, today),
                occurrences: Vec::new(),
            });
        }
        if let Some(group) = forecast.groups.last_mut() {
            group.occurrences.push(Occurrence {
                schedule_id: schedule.id.clone(),
                schedule_name: schedule.name.clone(),
                job_id: schedule.job_id.clone(),
                cron: schedule.cron.clone(),
                at,
            });
        }
    }
    forecast
}

#[cfg(test)]
#[path = "forecast_tests.rs"]
mod tests;
