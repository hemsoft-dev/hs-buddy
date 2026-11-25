//! The shell split saved in `ui.paneSizes`: `[sidebar, content, assistant]`,
//! as Electron's `useAppLayout` keeps them (`src/appUtils.ts`,
//! `src/hooks/useAppLayout.ts`).
//!
//! # Weights and widths
//!
//! Electron hands the saved sizes to Allotment as `defaultSizes`, which
//! shares the row among the panes on screen in proportion to them (the
//! sidebar and the content, and the assistant while it is open), so a file
//! from another row width, or edited by hand, holds weights. Both apps
//! write the pixel widths a drag left on screen, as Allotment's `onChange`
//! reports them, so a saved file is laid out at exactly those widths in a
//! row of the same width. While the assistant is closed its entry is the
//! width it opens at (Allotment's `preferredSize`).
//!
//! # Invariants
//!
//! - **I1** Clamping and minimums apply only to what is displayed
//!   ([`layout`]). A save writes them only as the displayed result: the
//!   widths the clamped panes showed.
//! - **I2** Releasing a divider saves the displayed widths of the panes on
//!   screen, in pixels at 100% (the unit [`layout`] reads):
//!   the sidebar, the main pane beside the open assistant, and the
//!   assistant ([`after_drag`]). When the file Electron may have changed
//!   since says the assistant is closed, the sidebar and the workspace
//!   beside it are written and the assistant entry is kept, and a drag of
//!   the assistant's own divider saves nothing. A row without a sidebar
//!   keeps the sidebar entry and shares the content and assistant entries'
//!   sum as they were displayed, so the hidden sidebar keeps its share.
//!   A value that is not a positive finite number is never written.
//! - **I3** Opening or closing the assistant keeps the sidebar's saved
//!   share: opening splits the content's weight between the content and
//!   the assistant, closing gives both back to the content
//!   ([`after_assistant_toggle`]). This differs from Electron on opening on
//!   purpose: Allotment inserts the pane with `Sizing.Distribute`, which
//!   shares the row equally among the three panes, then applies the
//!   assistant's preferred width, so Electron's sidebar moves to about a
//!   third of the row and its `onChange` saves that. Closing matches:
//!   Allotment gives the assistant's width to the content.
//! - **I4** After any save attempt, successful or not, the layout is rebuilt
//!   from what is on disk: the file the save read or wrote. When the file
//!   could not be read at all, the split lays out from the widths the drag
//!   showed, as I2 would have saved them, until a later read (`buddy-app`'s
//!   `app.rs`); it is never rebuilt from in-memory sizes the file may no
//!   longer hold.
//! - **I5** A save never moves a divider: for every row, zoom and
//!   assistant state in which the app saves a drag (the minimums fit the
//!   row), [`layout`] of the saved widths in the same row shows exactly the
//!   widths that were displayed. [`layout`] is the identity for entries
//!   that already hold the minimums and sum to the row.

/// `DEFAULT_PANE_SIZES`: the sidebar and content defaults.
pub const DEFAULT_PANE_SIZES: [f64; 2] = [300.0, 900.0];
/// `DEFAULT_ASSISTANT_PANE_SIZE`: the assistant default.
pub const DEFAULT_ASSISTANT_PANE_SIZE: f64 = 350.0;
/// The sidebar pane's `minSize` in `App.tsx`.
pub const SIDEBAR_MIN_WIDTH: f64 = 200.0;
/// The content pane's `minSize` in `App.tsx`.
pub const CONTENT_MIN_WIDTH: f64 = 400.0;
/// The main pane's minimum beside the open assistant in native's nested
/// split (the workspace holds both).
pub const MAIN_BESIDE_ASSISTANT_MIN_WIDTH: f64 = 250.0;
/// The assistant pane's `minSize` and `maxSize` in `App.tsx`.
pub const ASSISTANT_MIN_WIDTH: f64 = 280.0;
pub const ASSISTANT_MAX_WIDTH: f64 = 600.0;

/// A width that is not a number lays out as no width.
fn finite_width(width: f64) -> f64 {
    if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    }
}

/// The share of the minimums `wanted` that a row `available` pixels wide
/// can hold: 1 when it holds them all, less in proportion when it cannot.
fn fit(available: f64, wanted: f64) -> f64 {
    let available = finite_width(available);
    if wanted > available {
        available / wanted
    } else {
        1.0
    }
}

/// The workspace's minimum: the content's 400, or, beside the open
/// assistant (its width, `None` while it is closed), room for it and the
/// main pane.
fn workspace_min(assistant: Option<f64>) -> f64 {
    assistant
        .filter(|width| width.is_finite() && *width > 0.0)
        .map_or(CONTENT_MIN_WIDTH, |width| {
            (width + MAIN_BESIDE_ASSISTANT_MIN_WIDTH).max(CONTENT_MIN_WIDTH)
        })
}

/// `normalizePaneSizes`: fewer than two entries, or any entry that is not a
/// positive number, gives the defaults; two entries gain the assistant
/// default; anything else is kept as saved.
pub fn normalize(sizes: &[f64]) -> Vec<f64> {
    if sizes.len() < 2 || !sizes.iter().all(|size| size.is_finite() && *size > 0.0) {
        return vec![
            DEFAULT_PANE_SIZES[0],
            DEFAULT_PANE_SIZES[1],
            DEFAULT_ASSISTANT_PANE_SIZE,
        ];
    }
    let mut sizes = sizes.to_vec();
    if sizes.len() == 2 {
        sizes.push(DEFAULT_ASSISTANT_PANE_SIZE);
    }
    sizes
}

/// The three weights the layout reads from saved `sizes`: the sidebar and
/// content from [`normalize`] (its defaults when they are unreadable), and
/// the assistant entry when it is a positive number, its default
/// otherwise. None is clamped (I1).
pub fn entries(sizes: &[f64]) -> [f64; 3] {
    let normalized = normalize(sizes);
    let assistant = sizes
        .get(2)
        .copied()
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or(DEFAULT_ASSISTANT_PANE_SIZE);
    [normalized[0], normalized[1], assistant]
}

/// The sidebar split's limits for a row `available` pixels wide, with the
/// open assistant's width (`None` while it is closed).
///
/// The workspace beside the sidebar keeps Electron's 400 for the content,
/// or, while the assistant is open, room for it and the main pane's 250.
/// A row too narrow for both minimums shrinks them in proportion, so the
/// split never lays out wider than the row; `scale` is that proportion (1
/// when the row holds them), which the assistant split inside the
/// workspace shrinks by too (see [`AssistantLimits`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarLimits {
    pub sidebar_min: f64,
    pub sidebar_max: f64,
    pub workspace_min: f64,
    pub scale: f64,
}

impl SidebarLimits {
    pub fn new(available: f64, assistant: Option<f64>) -> Self {
        let available = finite_width(available);
        let workspace_min = workspace_min(assistant);
        let scale = fit(available, SIDEBAR_MIN_WIDTH + workspace_min);
        let sidebar_min = SIDEBAR_MIN_WIDTH * scale;
        let workspace_min = workspace_min * scale;
        Self {
            sidebar_min,
            // Never below the minimum, which a range needs.
            sidebar_max: (available - workspace_min).max(sidebar_min),
            workspace_min,
            scale,
        }
    }

    /// Whether the row was too narrow for the minimums, which then pin the
    /// sidebar at its shrunk minimum instead of its saved share.
    pub fn shrunk(&self) -> bool {
        self.scale < 1.0
    }
}

/// The scale for a workspace that fills a row `available` pixels wide on
/// its own (no sidebar beside it), with the open assistant's width.
pub fn workspace_scale(available: f64, assistant: Option<f64>) -> f64 {
    fit(available, workspace_min(assistant))
}

/// The split between the main pane and the open assistant, in a workspace
/// whose minimums shrank by `scale` (from [`SidebarLimits::scale`] or
/// [`workspace_scale`]).
///
/// The main pane keeps 250 and the assistant Electron's 280-600. When the
/// row shrank the workspace's minimum, which was the assistant and the
/// main pane's 250, both shrink by the same scale, and the assistant is
/// held at its shrunk width, so the two fill no more than that minimum
/// and never extend past the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistantLimits {
    pub main_min: f64,
    pub assistant_min: f64,
    pub assistant_max: f64,
    /// The assistant's width as it paints.
    pub assistant: f64,
    /// Its width in a row that holds every minimum: the share of the row
    /// its weight gives it, within 280-600.
    pub preferred: f64,
}

impl AssistantLimits {
    pub fn new(scale: f64, preferred: f64) -> Self {
        let scale = if scale.is_finite() {
            scale.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let preferred = if preferred.is_finite() {
            preferred.clamp(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH)
        } else {
            DEFAULT_ASSISTANT_PANE_SIZE
        };
        let (assistant_min, assistant_max) = if scale < 1.0 {
            (preferred * scale, preferred * scale)
        } else {
            (ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH)
        };
        Self {
            main_min: MAIN_BESIDE_ASSISTANT_MIN_WIDTH * scale,
            assistant_min,
            assistant_max,
            assistant: preferred * scale,
            preferred,
        }
    }

    /// Whether the minimums shrank, which holds the assistant at its shrunk
    /// width.
    pub fn shrunk(&self) -> bool {
        self.main_min < MAIN_BESIDE_ASSISTANT_MIN_WIDTH
    }

    /// The width the assistant paints for `width` in a workspace
    /// `workspace` wide: within its limits, leaving the main pane its
    /// minimum. GPUI Kit's resize state can hold more than the panel paints
    /// (a drag past 600 px), so a drag reads its width through this.
    pub fn painted(&self, width: f64, workspace: f64) -> f64 {
        let most = self
            .assistant_max
            .min(workspace - self.main_min)
            .max(self.assistant_min);
        if width.is_finite() {
            width.clamp(self.assistant_min, most)
        } else {
            self.assistant
        }
    }
}

/// The row the shell splits, in pixels at 100%: beside the activity bar,
/// and whether the sidebar shows in it (a section without navigation
/// gives the workspace the whole row).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub available: f64,
    pub sidebar: bool,
}

/// The sidebar as it paints, and the limits a drag of its divider keeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPane {
    pub width: f64,
    pub limits: SidebarLimits,
}

/// What the shell shows for saved sizes in a row: everything a split lays
/// out from. Nothing else goes in, so the same file, row, zoom and
/// assistant state always lay out the same.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// `None` without a sidebar.
    pub sidebar: Option<SidebarPane>,
    /// The workspace beside the sidebar (the whole row without one).
    pub workspace: f64,
    /// How far the row shrank the workspace's minimums (1 when it holds
    /// them).
    pub scale: f64,
    /// `None` while the assistant is closed.
    pub assistant: Option<AssistantLimits>,
}

/// Lay saved `sizes` out in `row`, as Allotment shares a row among the
/// panes on screen in proportion to their weights; the minimums and the
/// assistant's 280-600 then hold what is shown (I1), never the file.
///
/// The sidebar takes its share of the row, within [`SidebarLimits`]. The
/// open assistant takes its share of the workspace, so a sidebar held at a
/// limit leaves the assistant's divider where its weights put it, within
/// [`AssistantLimits`].
pub fn layout(sizes: &[f64], assistant_open: bool, row: Row) -> Layout {
    let available = finite_width(row.available);
    // Shares of the largest weight, so a sum of huge weights cannot
    // overflow.
    let weights = entries(sizes);
    let largest = weights.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    let [sidebar, content, assistant] = weights.map(|weight| weight / largest);
    let workspace_weight = if assistant_open {
        content + assistant
    } else {
        content
    };
    let total = if row.sidebar {
        sidebar + workspace_weight
    } else {
        workspace_weight
    };
    // The open assistant's share of the row, as it paints beside a sidebar
    // at its own share; the minimums keep room for it.
    let preferred = assistant_open
        .then(|| (assistant / total * available).clamp(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH));
    let (sidebar, workspace, scale) = if row.sidebar {
        let limits = SidebarLimits::new(available, preferred);
        let width = (sidebar / total * available).clamp(limits.sidebar_min, limits.sidebar_max);
        (
            Some(SidebarPane { width, limits }),
            available - width,
            limits.scale,
        )
    } else {
        (None, available, workspace_scale(available, preferred))
    };
    let assistant = preferred.map(|preferred| {
        let mut limits = AssistantLimits::new(scale, preferred);
        if !limits.shrunk() {
            limits.assistant = limits.painted(assistant / workspace_weight * workspace, workspace);
        }
        limits
    });
    Layout {
        sidebar,
        workspace,
        scale,
        assistant,
    }
}

/// The divider a drag moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Between the sidebar and the workspace.
    Sidebar,
    /// Between the main pane and the open assistant.
    Assistant,
}

/// What a drag left on screen, in pixels at 100%.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    /// The sidebar; `None` in a row without one.
    pub sidebar: Option<f64>,
    /// The workspace beside the sidebar (the whole row without one).
    pub workspace: f64,
    /// The open assistant painted inside the workspace; `None` while it
    /// was closed.
    pub assistant: Option<f64>,
}

/// The save for a drag of `divider` that left `shown` on screen (I2), into
/// a file whose `sizes` and assistant state (`open`) were read at save
/// time: Electron may have changed either since the drag began.
///
/// The displayed widths are written as they are, so [`layout`] of the save
/// in the same row shows them again (I5):
///
/// - the open assistant, shown and open in the file: the sidebar, the main
///   pane (the workspace less the assistant) and the assistant;
/// - otherwise a sidebar drag writes the sidebar and the workspace beside
///   it, which the closed layout gives the content, and keeps the
///   assistant entry, the width it opens at;
/// - an assistant drag into a file that is closed now saves nothing: the
///   sidebar did not move and the closed layout shows no assistant;
/// - in a row without a sidebar the sidebar entry is kept, and the content
///   and the assistant share their entries' sum as they were shown.
///
/// Nothing is saved for a width that is not positive (the split was never
/// laid out), or when any value written would not be a positive finite
/// number.
pub fn after_drag(sizes: &[f64], divider: Divider, shown: Shown, open: bool) -> Option<Vec<f64>> {
    let measured = |width: f64| width.is_finite() && width > 0.0;
    if !measured(shown.workspace) || shown.sidebar.is_some_and(|width| !measured(width)) {
        return None;
    }
    let assistant = shown
        .assistant
        .filter(|width| open && measured(*width) && *width < shown.workspace);
    let [kept_sidebar, content, kept_assistant] = entries(sizes);
    let saved = match (divider, shown.sidebar, assistant) {
        (Divider::Sidebar, Some(sidebar), Some(assistant)) => {
            [sidebar, shown.workspace - assistant, assistant]
        }
        (Divider::Sidebar, Some(sidebar), None) => [sidebar, shown.workspace, kept_assistant],
        (Divider::Sidebar, None, _) | (Divider::Assistant, _, None) => return None,
        (Divider::Assistant, Some(sidebar), Some(assistant)) => {
            [sidebar, shown.workspace - assistant, assistant]
        }
        (Divider::Assistant, None, Some(assistant)) => {
            let pair = content + kept_assistant;
            let share = assistant / shown.workspace;
            [kept_sidebar, pair * (1.0 - share), pair * share]
        }
    };
    saved
        .iter()
        .all(|width| measured(*width))
        .then(|| saved.to_vec())
}

/// The save for opening (`open`) or closing the assistant in `row` (I3;
/// `None` before the shell first laid out, when the weights are taken for
/// widths).
///
/// Opening splits the content's weight so the assistant opens at the width
/// its entry names (within 280-600) in the workspace the closed layout
/// shows, or, in a workspace too narrow for it and the main pane's 250, at
/// the share the shrunk minimums give it. Closing gives the content the
/// assistant's weight back and keeps the width the assistant painted as
/// the width it reopens at. Either way the sidebar's share of the row is
/// unchanged.
pub fn after_assistant_toggle(sizes: &[f64], open: bool, row: Option<Row>) -> Vec<f64> {
    let [sidebar, content, assistant] = entries(sizes);
    if open {
        let preferred = assistant.clamp(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH);
        let workspace = row.map_or(content, |row| layout(sizes, false, row).workspace);
        let share = preferred / workspace.max(preferred + MAIN_BESIDE_ASSISTANT_MIN_WIDTH);
        vec![sidebar, content * (1.0 - share), content * share]
    } else {
        let painted = row
            .and_then(|row| layout(sizes, true, row).assistant)
            .map_or(assistant, |limits| {
                if limits.shrunk() {
                    limits.preferred
                } else {
                    limits.assistant
                }
            });
        vec![sidebar, content + assistant, painted]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    #[track_caller]
    fn assert_close(a: f64, b: f64) {
        assert!(close(a, b), "{a} != {b}");
    }

    #[track_caller]
    fn assert_all_close(a: &[f64], b: &[f64]) {
        assert!(
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| close(*a, *b)),
            "{a:?} != {b:?}"
        );
    }

    fn row(available: f64) -> Row {
        Row {
            available,
            sidebar: true,
        }
    }

    fn sidebar_px(sizes: &[f64], open: bool, available: f64) -> f64 {
        layout(sizes, open, row(available)).sidebar.unwrap().width
    }

    fn assistant_px(sizes: &[f64], available: f64) -> f64 {
        layout(sizes, true, row(available))
            .assistant
            .unwrap()
            .assistant
    }

    /// Whether the sidebar shows its saved share, which no limit held.
    fn at_share(sizes: &[f64], open: bool, available: f64) -> bool {
        let [sidebar, content, assistant] = entries(sizes);
        let total = sidebar + content + if open { assistant } else { 0.0 };
        let shown = layout(sizes, open, row(available));
        let pane = shown.sidebar.unwrap();
        !pane.limits.shrunk() && close(pane.width, sidebar / total * available)
    }

    /// Saved files the properties run over: Electron's pixel widths, the
    /// review's cases, weights far from the row's pixels, an assistant
    /// wider than it paints, and an unreadable entry.
    const FILES: &[&[f64]] = &[
        &[300.0, 900.0, 350.0],
        &[300.0, 400.0, 280.0],
        &[300.0, 400.0, 350.0],
        &[480.0, 720.0],
        &[400.0, 800.0, 600.0],
        &[450.0, 450.0, 600.0],
        &[100.0, 2000.0, 600.0],
        &[1000.0, 200.0, 280.0],
        &[30.0, 40.0, 28.0],
        &[300.0, 900.0, 1040.0],
        &[0.0, 900.0, 600.0],
        &[900.0, 300.0, 600.0],
        &[100.0, 600.0, 300.0],
        &[3e306, 9e306, 3e306],
    ];
    /// Rows from a narrow window at 150% to a wide one.
    const ROWS: &[f64] = &[552.0, 900.0, 1100.0, 1200.0, 1600.0, 2400.0, 3200.0];

    fn targets(min: f64, max: f64) -> impl Iterator<Item = f64> {
        (0..=6).map(move |step| min + (max - min) * f64::from(step) / 6.0)
    }

    fn assert_saved(saved: &[f64]) {
        assert_eq!(saved.len(), 3, "{saved:?}");
        assert!(
            saved.iter().all(|size| size.is_finite() && *size > 0.0),
            "{saved:?}"
        );
    }

    #[test]
    fn unreadable_sizes_fall_back_to_the_defaults() {
        let defaults = vec![300.0, 900.0, 350.0];
        for sizes in [
            &[][..],
            &[400.0][..],
            &[400.0, 0.0][..],
            &[-1.0, 900.0][..],
            &[400.0, 800.0, f64::NAN][..],
            &[400.0, f64::INFINITY][..],
        ] {
            assert_eq!(normalize(sizes), defaults, "{sizes:?}");
        }
        assert_eq!(normalize(&[400.0, 800.0]), vec![400.0, 800.0, 350.0]);
        assert_eq!(
            normalize(&[400.0, 800.0, 500.0, 7.0]),
            vec![400.0, 800.0, 500.0, 7.0]
        );
        // An unreadable sidebar falls back, a readable assistant is kept,
        // and nothing is clamped.
        assert_eq!(entries(&[0.0, 900.0, 600.0]), [300.0, 900.0, 600.0]);
        assert_eq!(entries(&[400.0, 800.0]), [400.0, 800.0, 350.0]);
        assert_eq!(entries(&[400.0, 800.0, 1040.0]), [400.0, 800.0, 1040.0]);
        assert_eq!(entries(&[400.0, 800.0, f64::NAN]), [300.0, 900.0, 350.0]);
    }

    #[test]
    fn the_panes_take_their_saved_share_of_the_row() {
        assert_close(sidebar_px(&[300.0, 900.0], false, 1200.0), 300.0);
        assert_close(sidebar_px(&[300.0, 900.0], false, 1600.0), 400.0);
        // The open assistant's weight joins the share, and it paints its
        // own share of the row.
        assert_close(sidebar_px(&[400.0, 1000.0, 600.0], true, 2000.0), 400.0);
        assert_close(assistant_px(&[400.0, 1000.0, 600.0], 2000.0), 600.0);
        assert_close(sidebar_px(&[400.0, 1000.0, 600.0], false, 1400.0), 400.0);
        // As Allotment lays out `defaultSizes`: [300, 900, 350] in 1,600.
        assert_close(
            assistant_px(&[300.0, 900.0, 350.0], 1600.0),
            350.0 / 1550.0 * 1600.0,
        );
        // An unreadable value lays out as the defaults.
        assert_close(sidebar_px(&[0.0, 900.0], false, 1200.0), 300.0);
        // Without a sidebar the workspace is the row.
        let alone = layout(
            &[300.0, 900.0, 300.0],
            true,
            Row {
                available: 1200.0,
                sidebar: false,
            },
        );
        assert_eq!(alone.sidebar, None);
        assert_close(alone.workspace, 1200.0);
        assert_close(alone.assistant.unwrap().assistant, 300.0);
    }

    #[test]
    fn the_sidebar_stays_within_the_pane_limits() {
        let limits = SidebarLimits::new(1000.0, None);
        assert_eq!(
            limits,
            SidebarLimits {
                sidebar_min: 200.0,
                sidebar_max: 600.0,
                workspace_min: 400.0,
                scale: 1.0,
            }
        );
        assert_close(sidebar_px(&[10.0, 990.0], false, 1000.0), 200.0);
        assert_close(sidebar_px(&[900.0, 100.0], false, 1000.0), 600.0);
    }

    #[test]
    fn an_open_assistant_keeps_room_for_itself_and_the_main_pane() {
        // 1,200 px beside a 600 px assistant: the workspace keeps 850.
        let limits = SidebarLimits::new(1200.0, Some(600.0));
        assert_eq!(
            limits,
            SidebarLimits {
                sidebar_min: 200.0,
                sidebar_max: 350.0,
                workspace_min: 850.0,
                scale: 1.0,
            }
        );
        // A narrow assistant still leaves the content Electron's 400.
        assert_eq!(SidebarLimits::new(1200.0, Some(100.0)).workspace_min, 400.0);
        // [900, 300, 600] in 1,800: every pane at its share.
        let shown = layout(&[900.0, 300.0, 600.0], true, row(1800.0));
        assert_close(shown.sidebar.unwrap().width, 900.0);
        assert_close(shown.assistant.unwrap().assistant, 600.0);
        // In 1,200 the assistant's third is 400, so the sidebar's half is
        // held at 550 to leave room for it and the main pane's 250, and the
        // assistant keeps that room.
        let shown = layout(&[900.0, 300.0, 600.0], true, row(1200.0));
        assert_close(shown.sidebar.unwrap().width, 550.0);
        assert_close(shown.workspace, 650.0);
        assert_close(shown.assistant.unwrap().assistant, 400.0);
    }

    #[test]
    fn a_row_too_narrow_for_both_minimums_shrinks_them_to_fit() {
        // A 900 px window at 150%: 552 px at 100% beside the activity bar.
        let limits = SidebarLimits::new(552.0, None);
        assert!((limits.sidebar_min - 184.0).abs() < 1e-9, "{limits:?}");
        assert!((limits.workspace_min - 368.0).abs() < 1e-9, "{limits:?}");
        assert!(limits.sidebar_max >= limits.sidebar_min, "{limits:?}");
        assert!(limits.sidebar_min + limits.workspace_min <= 552.0 + 1e-9);
        assert_close(sidebar_px(&[300.0, 900.0], false, 552.0), 184.0);
        // With the assistant open, and with no row at all.
        let open = SidebarLimits::new(552.0, Some(600.0));
        assert!(
            open.sidebar_min + open.workspace_min <= 552.0 + 1e-9,
            "{open:?}"
        );
        assert!(open.sidebar_max >= open.sidebar_min, "{open:?}");
        for available in [0.0, -10.0, f64::NAN] {
            let none = SidebarLimits::new(available, Some(600.0));
            assert_eq!(
                none,
                SidebarLimits {
                    sidebar_min: 0.0,
                    sidebar_max: 0.0,
                    workspace_min: 0.0,
                    scale: 0.0,
                },
                "{available}"
            );
        }
    }

    #[test]
    fn the_assistant_split_shrinks_with_the_row_it_sits_in() {
        // A row that holds every minimum leaves Electron's limits.
        assert_eq!(
            AssistantLimits::new(1.0, 350.0),
            AssistantLimits {
                main_min: 250.0,
                assistant_min: 280.0,
                assistant_max: 600.0,
                assistant: 350.0,
                preferred: 350.0,
            }
        );
        assert!(!AssistantLimits::new(1.0, 350.0).shrunk());
        // A 900 px window at 150% beside a 350 px assistant: the
        // workspace's 600 shrank to 414, and the main pane and the
        // assistant fill exactly that.
        let row = SidebarLimits::new(552.0, Some(350.0));
        assert!(row.shrunk(), "{row:?}");
        let split = AssistantLimits::new(row.scale, 350.0);
        assert!(split.shrunk(), "{split:?}");
        assert!(
            (split.main_min + split.assistant_max - row.workspace_min).abs() < 1e-9,
            "{split:?} in {row:?}"
        );
        assert_eq!(split.assistant_min, split.assistant_max);
        assert_eq!(split.assistant, split.assistant_max);
        assert!((split.assistant - 350.0 * row.scale).abs() < 1e-9);
        // Without a sidebar the workspace is the row.
        let scale = workspace_scale(552.0, Some(350.0));
        assert!((scale - 552.0 / 600.0).abs() < 1e-9, "{scale}");
        let alone = AssistantLimits::new(scale, 350.0);
        assert!((alone.main_min + alone.assistant_max - 552.0).abs() < 1e-9);
        assert_eq!(workspace_scale(1200.0, Some(350.0)), 1.0);
        assert_eq!(workspace_scale(300.0, None), 0.75);
        // An unreadable width or scale stays inside the limits.
        assert_eq!(AssistantLimits::new(1.0, f64::NAN).assistant, 350.0);
        assert_eq!(AssistantLimits::new(1.0, 900.0).assistant, 600.0);
        assert_eq!(AssistantLimits::new(f64::NAN, 350.0).assistant_max, 0.0);
        // GPUI Kit's state past the cap paints the cap, and the main pane
        // keeps its minimum.
        let limits = AssistantLimits::new(1.0, 350.0);
        assert_eq!(limits.painted(1040.0, 2000.0), 600.0);
        assert_eq!(limits.painted(1040.0, 700.0), 450.0);
        assert_eq!(limits.painted(100.0, 700.0), 280.0);
        assert_eq!(limits.painted(f64::NAN, 700.0), 350.0);
    }

    /// Review round 4, finding 1: `[300, 400, 280]` in a 1,600 px row. The
    /// earlier save clamped the content to 250 and wrote `[300, 250, 600]`,
    /// which moved the sidebar from 490 to 417 px.
    #[test]
    fn an_assistant_drag_keeps_the_sidebar_where_it_was() {
        let before = [300.0, 400.0, 280.0];
        let at = row(1600.0);
        let shown = layout(&before, true, at);
        let sidebar = shown.sidebar.unwrap().width;
        assert_close(sidebar, 300.0 / 980.0 * 1600.0);
        // The workspace has room for a 600 px assistant.
        let dragged = drag(&shown, at, Divider::Assistant, 600.0);
        assert_eq!(dragged.assistant, Some(600.0));
        let saved = after_drag(&before, Divider::Assistant, dragged, true).unwrap();
        assert_all_close(&saved, &[sidebar, 1600.0 - sidebar - 600.0, 600.0]);
        let after = layout(&saved, true, at);
        assert_close(after.sidebar.unwrap().width, sidebar);
        assert_close(after.assistant.unwrap().assistant, 600.0);
        // Past the cap: GPUI Kit's 1,040 paints 600 and saves the same.
        let past = drag(&shown, at, Divider::Assistant, 1040.0);
        assert_eq!(
            after_drag(&before, Divider::Assistant, past, true),
            Some(saved)
        );
    }

    /// Review round 4, finding 2: `[300, 400, 350]` in a 1,600 px row. The
    /// earlier open clamped the saved content to 250 and moved the sidebar
    /// from 686 to 533 px, and closing saved that.
    #[test]
    fn opening_the_assistant_keeps_the_sidebar_where_it_was() {
        let closed = [300.0, 400.0, 350.0];
        let at = row(1600.0);
        let sidebar = sidebar_px(&closed, false, 1600.0);
        assert_close(sidebar, 300.0 / 700.0 * 1600.0);
        let open = after_assistant_toggle(&closed, true, Some(at));
        assert_eq!(open[0], 300.0);
        assert_close(open[1] + open[2], 400.0);
        let opened = layout(&open, true, at);
        assert_close(opened.sidebar.unwrap().width, sidebar);
        // It opens at the 350 its entry names.
        assert_close(opened.assistant.unwrap().assistant, 350.0);
        // Closing restores the file and the divider.
        let again = after_assistant_toggle(&open, false, Some(at));
        assert_eq!(again[0], 300.0);
        assert_close(again[1], 400.0);
        assert_close(again[2], 350.0);
        assert_close(sidebar_px(&again, false, 1600.0), sidebar);
        // Before the first layout the weights are taken for widths.
        let unknown = after_assistant_toggle(&[300.0, 900.0, 350.0], true, None);
        assert_all_close(&unknown, &[300.0, 550.0, 350.0]);
        assert_all_close(
            &after_assistant_toggle(&unknown, false, None),
            &[300.0, 900.0, 350.0],
        );
        // Unreadable sizes start from the defaults.
        assert_all_close(
            &after_assistant_toggle(&[], true, None),
            &[300.0, 550.0, 350.0],
        );
    }

    /// I1: the minimums hold what is shown, and a save writes them only as
    /// the widths the clamped panes showed.
    #[test]
    fn the_minimums_hold_the_layout_and_a_save_writes_what_they_showed() {
        // [300, 400] is 700 of weight in a 1,600 px row: a drag to the
        // sidebar's 200 minimum saves the 200 and the 1,400 beside it.
        let at = row(1600.0);
        let shown = layout(&[300.0, 400.0], false, at);
        let dragged = drag(&shown, at, Divider::Sidebar, 200.0);
        let saved = after_drag(&[300.0, 400.0], Divider::Sidebar, dragged, false).unwrap();
        assert_all_close(&saved, &[200.0, 1400.0, 350.0]);
        assert_close(sidebar_px(&saved, false, 1600.0), 200.0);
        // A weight wider than the 600 the assistant paints is saved as the
        // 600 it showed.
        let file = [300.0, 900.0, 1040.0];
        let shown = layout(&file, true, at);
        assert_close(shown.assistant.unwrap().assistant, 600.0);
        let dragged = drag(&shown, at, Divider::Sidebar, 400.0);
        let saved = after_drag(&file, Divider::Sidebar, dragged, true).unwrap();
        assert_all_close(&saved, &[400.0, 600.0, 600.0]);
    }

    /// Review round 5, finding 2: `[900, 300, 600]` in a 1,200 px row holds
    /// the sidebar at its 550 maximum beside a 400 px assistant. Saving the
    /// assistant's weight let the release lay the sidebar out at 600 and
    /// snap the assistant to 280.
    #[test]
    fn an_assistant_drag_beside_a_sidebar_at_its_maximum_keeps_both() {
        let file = [900.0, 300.0, 600.0];
        let at = row(1200.0);
        let shown = layout(&file, true, at);
        assert_close(shown.sidebar.unwrap().width, 550.0);
        assert_close(shown.assistant.unwrap().assistant, 400.0);
        let dragged = drag(&shown, at, Divider::Assistant, 300.0);
        let saved = after_drag(&file, Divider::Assistant, dragged, true).unwrap();
        assert_all_close(&saved, &[550.0, 350.0, 300.0]);
        let after = layout(&saved, true, at);
        assert_close(after.sidebar.unwrap().width, 550.0);
        assert_close(after.assistant.unwrap().assistant, 300.0);
    }

    /// Review round 5, finding 3: `[100, 600, 300]` in a 1,200 px row holds
    /// the sidebar at its 200 minimum beside a 333 px assistant. Keeping the
    /// assistant's weight laid it out at 360 after a sidebar drag to 300.
    #[test]
    fn a_sidebar_drag_off_its_minimum_keeps_the_assistant() {
        let file = [100.0, 600.0, 300.0];
        let at = row(1200.0);
        let shown = layout(&file, true, at);
        assert_close(shown.sidebar.unwrap().width, 200.0);
        let assistant = shown.assistant.unwrap().assistant;
        assert_close(assistant, 1000.0 / 3.0);
        let dragged = drag(&shown, at, Divider::Sidebar, 300.0);
        assert_eq!(dragged.assistant, Some(assistant));
        let saved = after_drag(&file, Divider::Sidebar, dragged, true).unwrap();
        assert_all_close(&saved, &[300.0, 900.0 - assistant, assistant]);
        let after = layout(&saved, true, at);
        assert_close(after.sidebar.unwrap().width, 300.0);
        assert_close(after.assistant.unwrap().assistant, assistant);
    }

    /// Review round 5, finding 1: Electron closes the assistant while
    /// native drags it. `[300, 400, 280]` in a 1,600 px row and a drag to
    /// 600 wrote `[300, 312.5, 367.5]`, which the closed layout showed with
    /// the sidebar at 784 px instead of 686.
    #[test]
    fn an_assistant_drag_into_a_file_closed_since_saves_nothing() {
        let file = [300.0, 400.0, 280.0];
        let at = row(1600.0);
        let shown = layout(&file, true, at);
        let dragged = drag(&shown, at, Divider::Assistant, 600.0);
        assert_eq!(after_drag(&file, Divider::Assistant, dragged, false), None);
        assert_close(sidebar_px(&file, false, 1600.0), 300.0 / 700.0 * 1600.0);
        // A sidebar drag into a file closed since writes the sidebar and the
        // workspace the closed content takes, so the sidebar stays put.
        let dragged = drag(&shown, at, Divider::Sidebar, 450.0);
        let saved = after_drag(&file, Divider::Sidebar, dragged, false).unwrap();
        assert_all_close(&saved, &[450.0, 1150.0, 280.0]);
        assert_close(sidebar_px(&saved, false, 1600.0), 450.0);
    }

    /// Review round 5, finding 5: finite weights near `f64::MAX` must not
    /// overflow into a save that serializes as `null`.
    #[test]
    fn huge_weights_never_save_a_value_that_is_not_finite() {
        let file = [3e306, 9e306, 3e306];
        let at = row(1200.0);
        let shown = layout(&file, true, at);
        let dragged = drag(&shown, at, Divider::Assistant, 400.0);
        let saved = after_drag(&file, Divider::Assistant, dragged, true).unwrap();
        assert_saved(&saved);
        assert!(!serde_json::to_string(&saved).unwrap().contains("null"));
        assert_close(saved[2], 400.0);
        assert_close(layout(&saved, true, at).assistant.unwrap().assistant, 400.0);
        // Weights whose sum overflows still lay out.
        let huge = [f64::MAX, f64::MAX, f64::MAX];
        let shown = layout(&huge, true, at);
        assert!(shown.sidebar.unwrap().width.is_finite(), "{shown:?}");
        assert!(shown.assistant.unwrap().assistant.is_finite(), "{shown:?}");
        // Without a sidebar the content and assistant entries keep their
        // sum, which overflows here: nothing is saved and the file stays.
        let alone = Row {
            available: 1200.0,
            sidebar: false,
        };
        let shown = layout(&huge, true, alone);
        let dragged = drag(&shown, alone, Divider::Assistant, 400.0);
        assert_eq!(after_drag(&huge, Divider::Assistant, dragged, true), None);
    }

    /// I5: entries that hold every minimum and sum to the row lay out as
    /// themselves.
    #[test]
    fn layout_is_the_identity_for_widths_that_fit_the_row() {
        for &available in ROWS {
            if available < SIDEBAR_MIN_WIDTH + CONTENT_MIN_WIDTH {
                continue;
            }
            let at = row(available);
            for sidebar in targets(SIDEBAR_MIN_WIDTH, available - CONTENT_MIN_WIDTH) {
                let content = available - sidebar;
                let shown = layout(&[sidebar, content], false, at);
                assert!(close(shown.sidebar.unwrap().width, sidebar), "{shown:?}");
                for assistant in targets(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH) {
                    let main = content - assistant;
                    if main < MAIN_BESIDE_ASSISTANT_MIN_WIDTH {
                        continue;
                    }
                    let case = format!("[{sidebar}, {main}, {assistant}] in {available}");
                    let shown = layout(&[sidebar, main, assistant], true, at);
                    assert!(close(shown.sidebar.unwrap().width, sidebar), "{case}");
                    assert!(close(shown.workspace, content), "{case}");
                    assert!(
                        close(shown.assistant.unwrap().assistant, assistant),
                        "{case}"
                    );
                }
            }
        }
    }

    /// What the app shows when a drag of `divider` ends at `target` from
    /// `shown` in `row`: the sidebar divider moves the sidebar and the
    /// workspace while GPUI Kit's fixed assistant panel keeps the width it
    /// painted, within its limits; the assistant divider moves the
    /// assistant within the workspace.
    fn drag(shown: &Layout, row: Row, divider: Divider, target: f64) -> Shown {
        match divider {
            Divider::Sidebar => {
                let workspace = row.available - target;
                Shown {
                    sidebar: Some(target),
                    workspace,
                    assistant: shown
                        .assistant
                        .map(|limits| limits.painted(limits.assistant, workspace)),
                }
            }
            Divider::Assistant => Shown {
                sidebar: shown.sidebar.map(|pane| pane.width),
                workspace: shown.workspace,
                assistant: shown
                    .assistant
                    .map(|limits| limits.painted(target, shown.workspace)),
            },
        }
    }

    /// I5 for one drag: the save lays out in the same row at exactly the
    /// widths the drag showed, so no divider moves on release.
    #[track_caller]
    fn assert_lays_out_as_shown(sizes: &[f64], open: bool, at: Row, divider: Divider, to: f64) {
        let case = format!("{sizes:?} open={open} in {at:?}, {divider:?} to {to}");
        let shown = layout(sizes, open, at);
        let dragged = drag(&shown, at, divider, to);
        let saved = after_drag(sizes, divider, dragged, open).unwrap();
        assert_saved(&saved);
        let after = layout(&saved, open, at);
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-6 * b.abs().max(1.0);
        if let Some(sidebar) = dragged.sidebar {
            let width = after.sidebar.unwrap().width;
            assert!(near(width, sidebar), "{case}: {saved:?} shows {after:?}");
        } else {
            let [sidebar, content, assistant] = entries(sizes);
            assert_eq!(saved[0], sidebar, "{case}");
            assert!(close(saved[1] + saved[2], content + assistant), "{case}");
        }
        assert!(
            near(after.workspace, dragged.workspace),
            "{case}: {saved:?} shows {after:?}"
        );
        if let Some(assistant) = dragged.assistant {
            let width = after.assistant.unwrap().assistant;
            assert!(near(width, assistant), "{case}: {saved:?} shows {after:?}");
        }
    }

    /// I5 for the sidebar divider, over every file, row and target the
    /// limits allow, with the assistant open and closed, from any start,
    /// including a sidebar held at a limit.
    #[test]
    fn a_sidebar_drag_lays_out_as_it_was_shown() {
        for sizes in FILES {
            for &available in ROWS {
                for open in [false, true] {
                    let at = row(available);
                    let limits = layout(sizes, open, at).sidebar.unwrap().limits;
                    if limits.shrunk() {
                        // The shrunk minimums pin the divider; app.rs saves
                        // nothing.
                        continue;
                    }
                    for target in targets(limits.sidebar_min, limits.sidebar_max) {
                        assert_lays_out_as_shown(sizes, open, at, Divider::Sidebar, target);
                    }
                }
            }
        }
    }

    /// I5 for the assistant divider, with and without a sidebar beside the
    /// workspace, from any start, including a sidebar held at a limit.
    #[test]
    fn an_assistant_drag_lays_out_as_it_was_shown() {
        for sizes in FILES {
            for &available in ROWS {
                for sidebar in [true, false] {
                    let at = Row { available, sidebar };
                    let shown = layout(sizes, true, at);
                    let limits = shown.assistant.unwrap();
                    if limits.shrunk() {
                        continue;
                    }
                    let most = limits.painted(f64::MAX, shown.workspace);
                    for target in targets(limits.assistant_min, most) {
                        assert_lays_out_as_shown(sizes, true, at, Divider::Assistant, target);
                    }
                }
            }
        }
    }

    /// I3: opening and closing keep the sidebar's saved share, and a round
    /// trip restores the file.
    #[test]
    fn toggling_the_assistant_keeps_the_sidebar_share() {
        for sizes in FILES {
            for &available in ROWS {
                for at in [None, Some(row(available))] {
                    let case = format!("{sizes:?} in {at:?}");
                    let [sidebar, content, assistant] = entries(sizes);
                    let open = after_assistant_toggle(sizes, true, at);
                    assert_saved(&open);
                    assert_eq!(open[0], sidebar, "{case}");
                    assert!(close(open[1] + open[2], content), "{case}: {open:?}");
                    let closed = after_assistant_toggle(sizes, false, at);
                    assert_saved(&closed);
                    assert_eq!(closed[0], sidebar, "{case}");
                    assert!(close(closed[1], content + assistant), "{case}: {closed:?}");
                    let Some(at) = at else { continue };
                    // The divider stays put wherever no limit holds it.
                    if at_share(sizes, false, available) && at_share(&open, true, available) {
                        let before = sidebar_px(sizes, false, available);
                        assert!(
                            close(sidebar_px(&open, true, available), before),
                            "{case}: {open:?}"
                        );
                        let preferred = assistant.clamp(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH);
                        let opened = layout(&open, true, at);
                        if opened.workspace >= preferred + MAIN_BESIDE_ASSISTANT_MIN_WIDTH {
                            assert!(
                                close(opened.assistant.unwrap().assistant, preferred),
                                "{case}: {open:?}"
                            );
                            let back = after_assistant_toggle(&open, false, Some(at));
                            assert!(close(back[1], content), "{case}: {back:?}");
                            assert!(close(back[2], preferred), "{case}: {back:?}");
                        }
                    }
                    if at_share(sizes, true, available) && at_share(&closed, false, available) {
                        assert!(
                            close(
                                sidebar_px(&closed, false, available),
                                sidebar_px(sizes, true, available)
                            ),
                            "{case}: {closed:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_drag_that_was_never_laid_out_saves_nothing() {
        let shown = |sidebar, workspace| Shown {
            sidebar: Some(sidebar),
            workspace,
            assistant: None,
        };
        for (sidebar, workspace) in [(0.0, 950.0), (250.0, -1.0), (f64::NAN, 950.0)] {
            assert_eq!(
                after_drag(
                    &[300.0, 900.0],
                    Divider::Sidebar,
                    shown(sidebar, workspace),
                    false
                ),
                None,
                "{sidebar} beside {workspace}"
            );
        }
        // An assistant no narrower than the workspace was never laid out.
        let wide = Shown {
            assistant: Some(950.0),
            ..shown(250.0, 950.0)
        };
        assert_eq!(
            after_drag(&[300.0, 900.0], Divider::Assistant, wide, true),
            None
        );
        // No sidebar, no sidebar drag.
        let alone = Shown {
            sidebar: None,
            ..shown(250.0, 950.0)
        };
        assert_eq!(
            after_drag(&[300.0, 900.0], Divider::Sidebar, alone, false),
            None
        );
        // Extra entries are dropped; an unreadable assistant saves its
        // default.
        assert_eq!(
            after_drag(
                &[300.0, 900.0, 420.0, 5.0],
                Divider::Sidebar,
                shown(250.0, 950.0),
                false
            ),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_drag(
                &[300.0, 900.0, 0.0],
                Divider::Sidebar,
                shown(250.0, 950.0),
                false
            ),
            Some(vec![250.0, 950.0, 350.0])
        );
    }
}
