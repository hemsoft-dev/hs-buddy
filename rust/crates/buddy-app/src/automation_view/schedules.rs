//! The Schedules route: Electron's "Upcoming Scheduled Jobs" forecast
//! (`ScheduleOverviewPanel.tsx`) and a schedule's detail with its recent runs
//! (`ScheduleDetailPanel.tsx`).
use buddy_core::automation::forecast::{self, DAY_OPTIONS, Forecast};
use buddy_core::automation::models::{Run, Schedule, WorkerType};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{AnyElement, App, ClickEvent, SharedString};

use super::render::local_time;
use super::*;

/// Forecast occurrences drawn at first and added by each "Show more". Electron
/// draws every occurrence; a busy window can hold tens of thousands.
pub(super) const FORECAST_SHOWN: usize = 100;
/// What a cached forecast was computed from: schedules, window, minute, zone.
pub(super) type ForecastKey = (Vec<Schedule>, f64, i64, String);

/// `automation.scheduleForecastDays` as Electron's panel reads it.
pub(super) fn forecast_days(cx: &gpui_kit::App) -> f64 {
    if cx.has_global::<Settings>() {
        Settings::global(cx).config.automation.forecast_days()
    } else {
        forecast::DEFAULT_DAYS
    }
}

/// The forecast row's worker badge, `occ.workerType` as Electron prints it.
fn worker_badge(worker: WorkerType) -> &'static str {
    match worker {
        WorkerType::Exec => "exec",
        WorkerType::Ai => "ai",
        WorkerType::Skill => "skill",
    }
}

/// `RunStatus` as Electron's recent-run rows print it (`run.status`).
fn status_word(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Pending => "pending",
        RunStatus::Running => "running",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

/// `run.triggeredBy` (`manual`, `schedule` or `api`); an older run without it
/// reads from its schedule link, as History does.
fn trigger_word(run: &Run) -> String {
    match run.extra.get("triggeredBy").and_then(Value::as_str) {
        Some(trigger) => trigger.to_owned(),
        None if run.schedule_id.is_some() => "schedule".into(),
        None => "manual".into(),
    }
}

/// One recent run: status, trigger, how long ago it started and, when the
/// server recorded one, its duration.
pub(super) fn recent_run_parts(run: &Run, now_ms: f64) -> Vec<String> {
    let ago = (now_ms - run.started_at).max(0.0) as u64;
    let mut parts = vec![
        status_word(run.status).to_owned(),
        trigger_word(run),
        buddy_core::format::distance_to_now(ago),
    ];
    if let Some(duration) = run
        .extra
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
    {
        parts.push(buddy_core::format::duration(duration));
    }
    parts
}

impl AutomationView {
    fn set_forecast_days(&mut self, days: u32, cx: &mut Context<Self>) {
        if !cx.has_global::<Settings>() {
            return;
        }
        use buddy_core::application_settings::LocalEdit;
        let edit = LocalEdit::ScheduleForecastDays(days);
        match Settings::try_update(cx, |config| edit.apply(config)) {
            Ok(()) => self.error = None,
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }

    /// The forecast for the current minute, rebuilt only when what it
    /// depends on changed: typing in the filter redraws without recomputing.
    pub(super) fn forecast(&self, cx: &App) -> std::rc::Rc<Forecast> {
        let now = jiff::Timestamp::now();
        let zone = jiff::tz::TimeZone::system();
        let days = forecast_days(cx);
        let minute = now.as_second().div_euclid(60);
        let zone_name = zone.iana_name().unwrap_or_default().to_owned();
        let mut cache = self.forecast_cache.borrow_mut();
        if let Some(((schedules, cached_days, cached_minute, cached_zone), forecast)) = &*cache
            && *schedules == self.snapshot.schedules
            && *cached_days == days
            && *cached_minute == minute
            && *cached_zone == zone_name
        {
            return forecast.clone();
        }
        let forecast = std::rc::Rc::new(forecast::forecast(
            &self.snapshot.schedules,
            now,
            &zone,
            days,
        ));
        *cache = Some((
            (self.snapshot.schedules.clone(), days, minute, zone_name),
            forecast.clone(),
        ));
        forecast
    }

    pub(super) fn render_forecast(&self, cx: &mut Context<Self>) -> AnyElement {
        let forecast = self.forecast(cx);
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let mono = cx.theme().mono_font_family.clone();
        let view = cx.entity().downgrade();
        let days = forecast.days;
        let dropdown = Button::new("automation-forecast-days")
            .label(forecast::count(days, "day"))
            .dropdown_menu(move |mut menu, _, _| {
                for option in DAY_OPTIONS {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(forecast::count(f64::from(option), "day"))
                            .checked(f64::from(option) == days)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    view.update(cx, |this, cx| this.set_forecast_days(option, cx));
                            }),
                    );
                }
                menu
            });
        let header = h_flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(div().text_lg().child("Upcoming Scheduled Jobs"))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_color(muted).child("Forecast:"))
                    .child(dropdown),
            );
        let summary = h_flex()
            .flex_wrap()
            .gap_3()
            .text_color(muted)
            .debug_selector(|| "automation-forecast-summary".into())
            .children(forecast.summary().into_iter().map(|part| div().child(part)));
        let mut timeline = v_flex().gap_3();
        if forecast.groups.is_empty() {
            let (message, hint) = forecast.empty();
            timeline = timeline
                .child(div().whitespace_normal().child(message))
                .children(hint.map(|hint| div().text_color(muted).child(hint)));
        }
        let jobs: std::collections::HashMap<&str, _> = self
            .snapshot
            .jobs
            .iter()
            .map(|job| (job.id.as_str(), job))
            .collect();
        let total: usize = forecast
            .groups
            .iter()
            .map(|group| group.occurrences.len())
            .sum();
        let mut index = 0usize;
        for group in &forecast.groups {
            if index >= self.forecast_shown {
                break;
            }
            let runs = group.occurrences.len();
            let mut items = v_flex().gap_1();
            for occurrence in group.occurrences.iter().take(self.forecast_shown - index) {
                let job = jobs.get(occurrence.job_id.as_str()).copied();
                let job_name = job.map_or("(unknown job)", |job| job.name.as_str());
                let worker = worker_badge(job.map_or(WorkerType::Exec, |job| job.worker_type));
                let time = buddy_core::format::hour_minute(occurrence.at.time())
                    .unwrap_or_else(|| occurrence.at.strftime("%H:%M").to_string());
                let tooltip: SharedString = format!(
                    "Schedule: {}\nJob: {job_name}\nCron: {}",
                    occurrence.schedule_name, occurrence.cron
                )
                .into();
                let id = occurrence.schedule_id.clone();
                // A ghost button, as Electron's occurrence is a button: Tab
                // reaches it and Enter or Space opens its schedule.
                items =
                    items.child(
                        Button::new(("automation-forecast-row", index))
                            .ghost()
                            .w_full()
                            .h_auto()
                            .px_2()
                            .py_1()
                            .debug_selector(move || format!("automation-forecast-row-{index}"))
                            .tooltip(tooltip)
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_color(muted)
                                            .font_family(mono.clone())
                                            .child(time),
                                    )
                                    .child(div().flex_1().whitespace_normal().child(format!(
                                        "{} › {job_name}",
                                        occurrence.schedule_name
                                    )))
                                    .child(div().text_color(muted).child(worker)),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.set_schedule_detail(Some(id.clone()), cx)
                            })),
                    );
                index += 1;
            }
            timeline =
                timeline.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().child(group.label.clone()))
                                .child(div().text_color(muted).child(format!(
                                    "{runs} run{}",
                                    if runs == 1 { "" } else { "s" }
                                ))),
                        )
                        .child(items),
                );
        }
        let hidden = total.saturating_sub(self.forecast_shown);
        if hidden > 0 {
            timeline = timeline.child(
                Button::new("automation-forecast-more")
                    .label(format!(
                        "Show {} more of {hidden} upcoming runs",
                        hidden.min(FORECAST_SHOWN)
                    ))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.forecast_shown = this.forecast_shown.saturating_add(FORECAST_SHOWN);
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(border)
            .debug_selector(|| "automation-forecast".into())
            .child(header)
            .child(summary)
            .child(timeline)
            .into_any_element()
    }

    pub(super) fn render_schedule_detail(
        &self,
        schedule: &Schedule,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut runs_section = v_flex().gap_2().child(div().text_lg().child("Recent Runs"));
        let recent = self
            .snapshot
            .schedule_runs
            .as_ref()
            .filter(|runs| runs.schedule_id == schedule.id);
        match recent.and_then(|recent| recent.runs.as_ref()) {
            None => {
                runs_section = runs_section.child(div().text_color(muted).child(
                    if recent.and_then(|recent| recent.error.as_ref()).is_some() {
                        "Recent runs are unavailable. Buddy retries with the next refresh."
                    } else {
                        "Loading runs…"
                    },
                ))
            }
            Some(runs) if runs.is_empty() => {
                runs_section = runs_section.child(div().child("No runs yet for this schedule."))
            }
            Some(runs) => {
                let now = jiff::Timestamp::now().as_millisecond() as f64;
                for (index, run) in runs.iter().enumerate() {
                    let color = match run.status {
                        RunStatus::Failed => cx.theme().danger,
                        RunStatus::Completed => cx.theme().success,
                        RunStatus::Running | RunStatus::Pending => cx.theme().info,
                        RunStatus::Cancelled => cx.theme().warning,
                    };
                    let mut parts = recent_run_parts(run, now).into_iter();
                    let status = parts.next().unwrap_or_default();
                    let started: SharedString = local_time(run.started_at).into();
                    runs_section = runs_section.child(
                        h_flex()
                            .id(("automation-schedule-run", index))
                            .debug_selector(move || format!("automation-schedule-run-{index}"))
                            .flex_wrap()
                            .gap_3()
                            .child(div().text_color(color).child(status))
                            .children(parts.map(|part| div().text_color(muted).child(part)))
                            .tooltip(move |window, cx| {
                                Tooltip::new(started.clone()).build(window, cx)
                            }),
                    );
                }
            }
        }
        if let Some(error) = recent.and_then(|recent| recent.error.as_ref())
            && recent.is_some_and(|recent| recent.runs.is_some())
        {
            runs_section = runs_section.child(
                div()
                    .whitespace_normal()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        v_flex()
            .gap_3()
            .debug_selector(|| "automation-schedule-detail".into())
            .child(
                h_flex().child(
                    Button::new("automation-schedule-back")
                        .label("All schedules")
                        .on_click(cx.listener(|this, _, _, cx| this.set_schedule_detail(None, cx))),
                ),
            )
            .child(self.schedule_row(0, schedule, cx))
            .child(runs_section)
            .into_any_element()
    }

    /// The open detail's schedule, while it still exists.
    pub(super) fn detail_schedule(&self) -> Option<&Schedule> {
        let id = self.schedule_detail.as_ref()?;
        self.snapshot
            .schedules
            .iter()
            .find(|schedule| &schedule.id == id)
    }
}
