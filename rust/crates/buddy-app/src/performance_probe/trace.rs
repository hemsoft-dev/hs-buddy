//! Optional timing spans for the isolated Windows performance fixture.
//!
//! Admitted only on Windows, behind the existing `BUDDY_PERF_DIR` probe, for the
//! driver's own temporary root and config. Every duration comes from one local
//! monotonic clock. Nothing here starts work, requests a frame, retries or waits,
//! and nothing holds text, paths, errors or handles. A failed diagnostic makes
//! the trace unavailable and leaves the measured behavior alone.
//!
//! Frame callbacks bound a combined span. Draw, present and frame delivery all
//! fall inside it and are not separated here. See
//! `docs/NATIVE-PERFORMANCE-DIAGNOSTICS.md`.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(crate) const SCHEMA: u64 = 1;
/// The largest id a JSON number holds exactly.
const MAX_REQUEST_ID: u64 = (1 << 53) - 1;
/// `try_update_at` spends three attempts per update, and a navigation runs few.
const MAX_STAGE_COUNT: u8 = 32;
/// One hour. The driver gives up on a receipt after one minute.
const MAX_SPAN_MICROS: u64 = 3_600_000_000;
const ROOT_PREFIX: &str = "buddy-paired-perf-";

/// Fixed reasons, the only trace text a receipt can carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    Admission,
    RequestId,
    Invalidated,
    Incomplete,
}

impl Unavailable {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::RequestId => "request-id",
            Self::Invalidated => "invalidated",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Lock,
    Load,
    Save,
    Install,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StageTotal {
    pub(crate) micros: u64,
    pub(crate) calls: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SettingsStages {
    pub(crate) lock: StageTotal,
    pub(crate) load: StageTotal,
    pub(crate) save: StageTotal,
    pub(crate) install: StageTotal,
    pub(crate) attempts: u8,
}

const ZERO_TOTAL: StageTotal = StageTotal {
    micros: 0,
    calls: 0,
};

#[derive(Clone, Copy)]
struct Collector {
    armed: bool,
    in_stage: bool,
    invalid: bool,
    stages: SettingsStages,
}

const IDLE: Collector = Collector {
    armed: false,
    in_stage: false,
    invalid: false,
    stages: SettingsStages {
        lock: ZERO_TOTAL,
        load: ZERO_TOTAL,
        save: ZERO_TOTAL,
        install: ZERO_TOTAL,
        attempts: 0,
    },
};

thread_local! {
    // A `Copy` cell: nothing stays borrowed while an original call runs.
    static COLLECTOR: Cell<Collector> = const { Cell::new(IDLE) };
}

/// Change the armed collector. An unarmed one is only read.
fn change_armed<R>(change: impl FnOnce(&mut Collector) -> R) -> Option<R> {
    COLLECTOR
        .try_with(|cell| {
            let mut collector = cell.get();
            if !collector.armed {
                return None;
            }
            let result = change(&mut collector);
            cell.set(collector);
            Some(result)
        })
        .ok()
        .flatten()
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros())
        .unwrap_or(MAX_SPAN_MICROS)
        .min(MAX_SPAN_MICROS)
}

/// Run one original call exactly once and return its result untouched. The
/// span only records into a collector armed on this thread.
pub(crate) fn measure<T>(stage: Stage, call: impl FnOnce() -> T) -> T {
    let _span = StageSpan::begin(stage);
    call()
}

/// Count one read-modify-write attempt. A no-op unless armed.
pub(crate) fn attempt() {
    change_armed(|collector| {
        if collector.stages.attempts >= MAX_STAGE_COUNT {
            collector.invalid = true;
        } else {
            collector.stages.attempts += 1;
        }
    });
}

struct StageSpan {
    running: Option<(Stage, Instant)>,
}

impl StageSpan {
    fn begin(stage: Stage) -> Self {
        let admitted = change_armed(|collector| {
            if collector.in_stage {
                collector.invalid = true;
                return false;
            }
            collector.in_stage = true;
            true
        });
        Self {
            running: (admitted == Some(true)).then(|| (stage, Instant::now())),
        }
    }
}

impl Drop for StageSpan {
    fn drop(&mut self) {
        let Some((stage, started)) = self.running.take() else {
            return;
        };
        let elapsed = micros(started.elapsed());
        let unwinding = std::thread::panicking();
        change_armed(|collector| {
            collector.in_stage = false;
            let total = match stage {
                Stage::Lock => &mut collector.stages.lock,
                Stage::Load => &mut collector.stages.load,
                Stage::Save => &mut collector.stages.save,
                Stage::Install => &mut collector.stages.install,
            };
            if unwinding || total.calls >= MAX_STAGE_COUNT {
                collector.invalid = true;
            } else {
                total.calls += 1;
                total.micros = total.micros.saturating_add(elapsed).min(MAX_SPAN_MICROS);
            }
        });
    }
}

/// Arms stage collection for the synchronous call it encloses. Dropping it,
/// including during an unwind, clears the collector.
#[must_use]
pub(crate) struct StageScope {
    owner: bool,
}

impl StageScope {
    pub(crate) fn arm() -> Self {
        let owner = COLLECTOR
            .try_with(|cell| {
                let mut collector = cell.get();
                if collector.armed {
                    collector.invalid = true;
                    cell.set(collector);
                    return false;
                }
                collector.armed = true;
                cell.set(collector);
                true
            })
            .unwrap_or(false);
        Self { owner }
    }

    pub(crate) fn finish(self) -> Result<SettingsStages, Unavailable> {
        if !self.owner {
            return Err(Unavailable::Invalidated);
        }
        match COLLECTOR.try_with(Cell::get) {
            Ok(collector) if collector.armed && !collector.invalid && !collector.in_stage => {
                Ok(collector.stages)
            }
            _ => Err(Unavailable::Invalidated),
        }
    }
}

impl Drop for StageScope {
    fn drop(&mut self) {
        if self.owner {
            let _ = COLLECTOR.try_with(|cell| cell.set(IDLE));
        }
    }
}

/// The driver's own temporary root and its config, by canonical path.
fn admit_paths(root: &Path, config: Option<&Path>, temp: &Path) -> Result<(), Unavailable> {
    let canonical = |path: &Path| path.canonicalize().map_err(|_| Unavailable::Admission);
    let root = canonical(root)?;
    let temp = canonical(temp)?;
    let config = canonical(config.ok_or(Unavailable::Admission)?)?;
    let owned_root = root.parent() == Some(temp.as_path())
        && root
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.starts_with(ROOT_PREFIX));
    let owned_config = config.parent() == Some(root.as_path())
        && config.file_name() == Some(OsStr::new("config.json"))
        && config.is_file();
    if owned_root && owned_config {
        Ok(())
    } else {
        Err(Unavailable::Admission)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreviousWrite {
    pub(crate) id: u64,
    pub(crate) micros: u64,
}

type LastWrite = Rc<Cell<Option<PreviousWrite>>>;

enum Mode {
    Off,
    Unavailable(Unavailable),
    On {
        last_poll_end: Instant,
        last_write: LastWrite,
    },
}

/// Owned by the existing poll loop.
pub(crate) struct Tracer {
    mode: Mode,
}

/// When a poll started, and the gap since the previous poll ended.
#[derive(Clone, Copy)]
pub(crate) struct PollMark(Option<(Instant, Duration)>);

impl Tracer {
    pub(crate) fn admit(root: &Path) -> Self {
        if !cfg!(windows) {
            return Self { mode: Mode::Off };
        }
        let config = std::env::var_os("BUDDY_CONFIG_PATH");
        let config = config.as_deref().map(Path::new);
        Self::from_admission(admit_paths(root, config, &std::env::temp_dir()))
    }

    /// A running tracer without the Windows and path admission.
    #[cfg(test)]
    pub(crate) fn admit_for_test() -> Self {
        Self::from_admission(Ok(()))
    }

    fn from_admission(admission: Result<(), Unavailable>) -> Self {
        let mode = match admission {
            Ok(()) => Mode::On {
                last_poll_end: Instant::now(),
                last_write: Rc::default(),
            },
            Err(reason) => Mode::Unavailable(reason),
        };
        Self { mode }
    }

    pub(crate) fn poll_started(&self) -> PollMark {
        match &self.mode {
            Mode::On { last_poll_end, .. } => {
                let now = Instant::now();
                PollMark(Some((now, now.saturating_duration_since(*last_poll_end))))
            }
            _ => PollMark(None),
        }
    }

    pub(crate) fn poll_ended(&mut self) {
        if let Mode::On { last_poll_end, .. } = &mut self.mode {
            *last_poll_end = Instant::now();
        }
    }

    /// Call right after a successful read: its return is this request's zero.
    pub(crate) fn request_read(&self, poll: PollMark) -> RequestSpans {
        match (&self.mode, poll.0) {
            (Mode::On { last_write, .. }, Some((started, gap))) => {
                let t0 = Instant::now();
                RequestSpans(Spans::On(Box::new(Running {
                    t0,
                    poll_gap: micros(gap),
                    read: micros(t0.saturating_duration_since(started)),
                    id: None,
                    remove_done: None,
                    update_entry: None,
                    set_section_done: None,
                    settings: None,
                    first_frame: None,
                    second_frame: None,
                    last_write: last_write.clone(),
                })))
            }
            (Mode::Unavailable(reason), _) => RequestSpans(Spans::Unavailable(*reason)),
            _ => RequestSpans(Spans::Off),
        }
    }
}

struct Running {
    t0: Instant,
    poll_gap: u64,
    read: u64,
    id: Option<u64>,
    remove_done: Option<u64>,
    update_entry: Option<u64>,
    set_section_done: Option<u64>,
    settings: Option<Result<SettingsStages, Unavailable>>,
    first_frame: Option<u64>,
    second_frame: Option<u64>,
    last_write: LastWrite,
}

enum Spans {
    Off,
    Unavailable(Unavailable),
    On(Box<Running>),
}

/// One request's spans, moved through the two existing frame callbacks.
pub(crate) struct RequestSpans(Spans);

/// Every span of a complete request, as local durations in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) id: u64,
    pub(crate) poll_gap: u64,
    pub(crate) read: u64,
    pub(crate) remove_done: u64,
    pub(crate) update_entry: u64,
    pub(crate) set_section_done: u64,
    pub(crate) first_frame: u64,
    pub(crate) second_frame: u64,
    pub(crate) settings: SettingsStages,
    pub(crate) previous: Option<PreviousWrite>,
}

impl RequestSpans {
    /// For receipts that are not a request, such as `ready.json`.
    pub(crate) fn off() -> Self {
        Self(Spans::Off)
    }

    fn mark(&mut self, slot: impl FnOnce(&mut Running) -> &mut Option<u64>) {
        if let Spans::On(running) = &mut self.0 {
            let offset = micros(running.t0.elapsed());
            *slot(running) = Some(offset);
        }
    }

    pub(crate) fn mark_removed(&mut self) {
        self.mark(|running| &mut running.remove_done);
    }

    pub(crate) fn admit_id(&mut self, id: Option<u64>) {
        if let Spans::On(running) = &mut self.0 {
            match id {
                Some(id) if (1..=MAX_REQUEST_ID).contains(&id) => running.id = Some(id),
                _ => self.0 = Spans::Unavailable(Unavailable::RequestId),
            }
        }
    }

    pub(crate) fn mark_update_entry(&mut self) {
        self.mark(|running| &mut running.update_entry);
    }

    /// Arm stage collection for one synchronous navigation.
    pub(crate) fn arm_settings(&self) -> Option<StageScope> {
        matches!(self.0, Spans::On(_)).then(StageScope::arm)
    }

    pub(crate) fn settings_finished(&mut self, scope: Option<StageScope>) {
        if let (Spans::On(running), Some(scope)) = (&mut self.0, scope) {
            running.settings = Some(scope.finish());
        }
    }

    pub(crate) fn mark_set_section_done(&mut self) {
        self.mark(|running| &mut running.set_section_done);
    }

    pub(crate) fn mark_first_frame(&mut self) {
        self.mark(|running| &mut running.first_frame);
    }

    pub(crate) fn mark_second_frame(&mut self) {
        self.mark(|running| &mut running.second_frame);
    }

    /// `None` when tracing is off, so the receipt keeps its original shape. The
    /// previous receipt's write is taken here, once.
    pub(crate) fn report(&self) -> Option<Result<Report, Unavailable>> {
        match &self.0 {
            Spans::Off => None,
            Spans::Unavailable(reason) => Some(Err(*reason)),
            Spans::On(running) => Some(Self::complete(running)),
        }
    }

    fn complete(running: &Running) -> Result<Report, Unavailable> {
        let settings = running.settings.ok_or(Unavailable::Incomplete)??;
        let span = |value: Option<u64>| value.ok_or(Unavailable::Incomplete);
        let report = Report {
            id: span(running.id)?,
            poll_gap: running.poll_gap,
            read: running.read,
            remove_done: span(running.remove_done)?,
            update_entry: span(running.update_entry)?,
            set_section_done: span(running.set_section_done)?,
            first_frame: span(running.first_frame)?,
            second_frame: span(running.second_frame)?,
            settings,
            previous: running.last_write.take(),
        };
        Ok(report)
    }

    pub(crate) fn write_started(&self) -> Option<Instant> {
        matches!(self.0, Spans::On(_)).then(Instant::now)
    }

    /// Only a completed write is carried. The next receipt reports it, so the
    /// last receipt's write stays unobserved.
    pub(crate) fn write_finished(&self, started: Option<Instant>, written: bool) {
        if let (Spans::On(running), Some(started)) = (&self.0, started) {
            let write = running.id.filter(|_| written).map(|id| PreviousWrite {
                id,
                micros: micros(started.elapsed()),
            });
            running.last_write.set(write);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    fn collector() -> Collector {
        COLLECTOR.with(Cell::get)
    }

    fn assert_idle() {
        let state = collector();
        assert!(!state.armed && !state.in_stage && !state.invalid);
        assert_eq!(state.stages, SettingsStages::default());
    }

    fn tracer() -> Tracer {
        Tracer::from_admission(Ok(()))
    }

    /// One whole request through the same calls the probe makes.
    fn complete_request(tracer: &Tracer, id: u64) -> RequestSpans {
        let poll = tracer.poll_started();
        let mut spans = tracer.request_read(poll);
        spans.mark_removed();
        spans.admit_id(Some(id));
        spans.mark_update_entry();
        let scope = spans.arm_settings();
        attempt();
        measure(Stage::Lock, || ());
        spans.settings_finished(scope);
        spans.mark_set_section_done();
        spans.mark_first_frame();
        spans.mark_second_frame();
        spans
    }

    #[test]
    fn unarmed_calls_run_once_and_leave_the_collector_untouched() {
        let mut calls = 0;
        let value = measure(Stage::Save, || {
            calls += 1;
            "original"
        });
        attempt();
        assert_eq!((value, calls), ("original", 1));
        assert_idle();
    }

    #[test]
    fn armed_results_and_errors_are_the_original_objects() {
        struct NotClone(&'static str);
        let scope = StageScope::arm();
        let failed: Result<(), NotClone> = measure(Stage::Load, || Err(NotClone("exact")));
        assert_eq!(failed.err().map(|error| error.0), Some("exact"));
        let stages = scope.finish();
        assert_eq!(stages.map(|stages| stages.load.calls), Ok(1));
        assert_idle();
    }

    #[test]
    fn each_stage_records_each_call_and_attempts_are_counted() {
        let scope = StageScope::arm();
        attempt();
        for stage in [Stage::Lock, Stage::Load, Stage::Save, Stage::Install] {
            measure(stage, || std::thread::sleep(Duration::from_millis(1)));
        }
        attempt();
        measure(Stage::Load, || ());
        let stages = scope.finish().unwrap();
        assert_eq!(stages.attempts, 2);
        assert_eq!(stages.lock.calls, 1);
        assert_eq!(stages.load.calls, 2);
        assert_eq!(stages.save.calls, 1);
        assert_eq!(stages.install.calls, 1);
        for total in [stages.lock, stages.save, stages.install] {
            assert!(total.micros >= 1_000, "{total:?}");
        }
        assert_idle();
    }

    #[test]
    fn a_reentrant_stage_invalidates_but_still_runs_the_call() {
        let scope = StageScope::arm();
        let inner = measure(Stage::Save, || measure(Stage::Load, || 7));
        assert_eq!(inner, 7);
        assert_eq!(scope.finish(), Err(Unavailable::Invalidated));
        assert_idle();
    }

    #[test]
    fn a_reentrant_scope_invalidates_both_and_clears_once() {
        let outer = StageScope::arm();
        let inner = StageScope::arm();
        assert!(collector().armed && collector().invalid);
        assert_eq!(inner.finish(), Err(Unavailable::Invalidated));
        assert!(
            collector().armed,
            "the inner scope must not clear the outer"
        );
        assert_eq!(outer.finish(), Err(Unavailable::Invalidated));
        assert_idle();
    }

    #[test]
    fn an_unwind_keeps_its_payload_and_clears_the_scope() {
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _scope = StageScope::arm();
            measure(Stage::Save, || std::panic::panic_any(41_u32));
        }));
        assert_eq!(unwound.unwrap_err().downcast_ref::<u32>(), Some(&41));
        assert_idle();
        let scope = StageScope::arm();
        measure(Stage::Lock, || ());
        assert_eq!(scope.finish().map(|stages| stages.lock.calls), Ok(1));
    }

    #[test]
    fn an_unwound_stage_invalidates_a_scope_that_survives_it() {
        let scope = StageScope::arm();
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            measure(Stage::Save, || std::panic::panic_any(41_u32));
        }));
        assert!(unwound.is_err());
        assert_eq!(scope.finish(), Err(Unavailable::Invalidated));
        assert_idle();
    }

    #[test]
    fn counts_are_bounded_and_overflow_invalidates() {
        let scope = StageScope::arm();
        for _ in 0..MAX_STAGE_COUNT {
            attempt();
            measure(Stage::Load, || ());
        }
        assert_eq!(collector().stages.attempts, MAX_STAGE_COUNT);
        assert!(!collector().invalid);
        attempt();
        assert_eq!(collector().stages.attempts, MAX_STAGE_COUNT);
        assert!(collector().invalid);
        drop(scope);
        let scope = StageScope::arm();
        for _ in 0..=MAX_STAGE_COUNT {
            measure(Stage::Load, || ());
        }
        assert_eq!(collector().stages.load.calls, MAX_STAGE_COUNT);
        assert_eq!(scope.finish(), Err(Unavailable::Invalidated));
    }

    #[test]
    fn durations_saturate_at_the_schema_bound() {
        assert_eq!(micros(Duration::MAX), MAX_SPAN_MICROS);
        assert_eq!(micros(Duration::from_millis(2)), 2_000);
    }

    #[test]
    fn stages_outside_a_scope_are_not_collected() {
        let scope = StageScope::arm();
        drop(scope);
        measure(Stage::Lock, || ());
        assert_idle();
    }

    #[test]
    fn only_the_owned_root_and_its_config_are_admitted() {
        let temp = std::env::temp_dir();
        let root = temp.join(format!("{ROOT_PREFIX}admit-{}", std::process::id()));
        let foreign = temp.join(format!("buddy-foreign-admit-{}", std::process::id()));
        for directory in [&root, &foreign] {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("config.json"), "{}").unwrap();
            std::fs::write(directory.join("other.json"), "{}").unwrap();
        }
        let config = root.join("config.json");
        assert_eq!(admit_paths(&root, Some(&config), &temp), Ok(()));
        // A root reached through another spelling is still the same canonical root.
        let spelled = root.join("..").join(root.file_name().unwrap());
        assert_eq!(admit_paths(&spelled, Some(&config), &temp), Ok(()));
        for (case_root, case_config, case_temp) in [
            (&root, None, &temp),
            (&root, Some(root.join("other.json")), &temp),
            (&root, Some(root.join("missing.json")), &temp),
            (&root, Some(foreign.join("config.json")), &temp),
            (&foreign, Some(foreign.join("config.json")), &temp),
            (&root, Some(config.clone()), &root),
            (&root.join("missing"), Some(config.clone()), &temp),
        ] {
            assert_eq!(
                admit_paths(case_root, case_config.as_deref(), case_temp),
                Err(Unavailable::Admission),
            );
        }
        // A directory cannot stand in for the config file.
        let directory_config = root.join("nested");
        std::fs::create_dir_all(directory_config.join("config.json")).unwrap();
        assert_eq!(
            admit_paths(
                &directory_config,
                Some(&directory_config.join("config.json")),
                &temp
            ),
            Err(Unavailable::Admission),
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&foreign).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn other_platforms_never_admit_a_trace() {
        let root = std::env::temp_dir();
        let tracer = Tracer::admit(&root);
        assert!(matches!(tracer.mode, Mode::Off));
        let spans = tracer.request_read(tracer.poll_started());
        assert!(spans.arm_settings().is_none());
        assert_eq!(spans.report(), None);
        assert_idle();
    }

    #[test]
    fn failed_admission_is_reported_and_arms_nothing() {
        let tracer = Tracer::from_admission(Err(Unavailable::Admission));
        let mut spans = tracer.request_read(tracer.poll_started());
        spans.mark_removed();
        spans.admit_id(Some(1));
        assert!(spans.arm_settings().is_none());
        assert_idle();
        assert_eq!(spans.report(), Some(Err(Unavailable::Admission)));
        assert_eq!(spans.write_started(), None);
    }

    #[test]
    fn off_spans_read_no_clock_and_report_nothing() {
        let mut spans = RequestSpans::off();
        spans.mark_first_frame();
        spans.mark_second_frame();
        assert_eq!(spans.report(), None);
        assert_eq!(spans.write_started(), None);
    }

    #[test]
    fn only_a_valid_numeric_id_is_admitted() {
        let tracer = tracer();
        for id in [None, Some(0), Some(MAX_REQUEST_ID + 1), Some(u64::MAX)] {
            let mut spans = tracer.request_read(tracer.poll_started());
            spans.admit_id(id);
            assert!(spans.arm_settings().is_none());
            assert_eq!(spans.report(), Some(Err(Unavailable::RequestId)));
        }
        let mut spans = tracer.request_read(tracer.poll_started());
        spans.admit_id(Some(MAX_REQUEST_ID));
        assert!(matches!(spans.0, Spans::On(_)));
    }

    #[test]
    fn offsets_are_ordered_and_a_missing_span_stays_incomplete() {
        let tracer = tracer();
        let spans = complete_request(&tracer, 1);
        let report = spans.report().unwrap().unwrap();
        assert_eq!(report.id, 1);
        assert_eq!(report.settings.lock.calls, 1);
        assert_eq!(report.settings.attempts, 1);
        let offsets = [
            report.remove_done,
            report.update_entry,
            report.set_section_done,
            report.first_frame,
            report.second_frame,
        ];
        assert!(
            offsets.windows(2).all(|pair| pair[0] <= pair[1]),
            "{offsets:?}"
        );

        let marks: [fn(&mut RequestSpans); 5] = [
            RequestSpans::mark_removed,
            RequestSpans::mark_update_entry,
            RequestSpans::mark_set_section_done,
            RequestSpans::mark_first_frame,
            RequestSpans::mark_second_frame,
        ];
        for omitted in 0..marks.len() {
            let mut partial = tracer.request_read(tracer.poll_started());
            partial.admit_id(Some(2));
            let scope = partial.arm_settings();
            partial.settings_finished(scope);
            for (index, mark) in marks.iter().enumerate() {
                if index != omitted {
                    mark(&mut partial);
                }
            }
            assert_eq!(partial.report(), Some(Err(Unavailable::Incomplete)));
        }
        let mut unsettled = tracer.request_read(tracer.poll_started());
        unsettled.admit_id(Some(2));
        marks.iter().for_each(|mark| mark(&mut unsettled));
        assert_eq!(unsettled.report(), Some(Err(Unavailable::Incomplete)));
    }

    #[test]
    fn an_invalidated_collector_makes_only_the_trace_unavailable() {
        let tracer = tracer();
        let mut spans = tracer.request_read(tracer.poll_started());
        spans.admit_id(Some(1));
        let scope = spans.arm_settings();
        measure(Stage::Save, || measure(Stage::Load, || ()));
        spans.settings_finished(scope);
        assert_idle();
        for mark in [
            RequestSpans::mark_removed,
            RequestSpans::mark_update_entry,
            RequestSpans::mark_set_section_done,
            RequestSpans::mark_first_frame,
            RequestSpans::mark_second_frame,
        ] {
            mark(&mut spans);
        }
        assert_eq!(spans.report(), Some(Err(Unavailable::Invalidated)));
    }

    #[test]
    fn a_write_is_reported_once_by_the_next_receipt_under_its_own_id() {
        let tracer = tracer();
        let first = complete_request(&tracer, 1);
        assert_eq!(first.report().unwrap().unwrap().previous, None);
        let started = first.write_started();
        assert!(started.is_some());
        first.write_finished(started, true);

        let second = complete_request(&tracer, 2);
        let previous = second.report().unwrap().unwrap().previous.unwrap();
        assert_eq!(previous.id, 1);
        assert!(previous.micros <= MAX_SPAN_MICROS);
        // Taken once: a report without a newer write has nothing to carry.
        assert_eq!(second.report().unwrap().unwrap().previous, None);

        // The last write has no later receipt, so nothing reports it.
        second.write_finished(second.write_started(), true);
        drop(second);
    }

    #[test]
    fn a_failed_write_is_never_reported_as_a_previous_write() {
        let tracer = tracer();
        let first = complete_request(&tracer, 1);
        assert!(first.report().is_some());
        first.write_finished(first.write_started(), false);
        let second = complete_request(&tracer, 2);
        assert_eq!(second.report().unwrap().unwrap().previous, None);
    }

    #[test]
    fn a_poll_gap_runs_from_the_last_poll_end_to_the_read_start() {
        let mut tracer = tracer();
        tracer.poll_ended();
        std::thread::sleep(Duration::from_millis(2));
        let poll = tracer.poll_started();
        let mut spans = tracer.request_read(poll);
        spans.admit_id(Some(1));
        let Spans::On(running) = &spans.0 else {
            panic!("expected running spans");
        };
        assert!(running.poll_gap >= 2_000, "{}", running.poll_gap);
        tracer.poll_ended();
        let later = tracer.poll_started();
        let Some((_, gap)) = later.0 else {
            panic!("expected a poll mark");
        };
        assert!(gap < Duration::from_secs(1));
    }

    #[test]
    fn unavailable_reasons_are_fixed_tokens() {
        for (reason, token) in [
            (Unavailable::Admission, "admission"),
            (Unavailable::RequestId, "request-id"),
            (Unavailable::Invalidated, "invalidated"),
            (Unavailable::Incomplete, "incomplete"),
        ] {
            assert_eq!(reason.reason(), token);
        }
    }
}
