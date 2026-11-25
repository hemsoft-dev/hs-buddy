//! The shell split saved in `ui.paneSizes`: `[sidebar, content, assistant]`,
//! as Electron's `useAppLayout` keeps them (`src/appUtils.ts`,
//! `src/hooks/useAppLayout.ts`).
//!
//! # Weights
//!
//! Electron hands the saved sizes to Allotment as `defaultSizes`, which
//! shares the row among the panes on screen in proportion to them (the
//! sidebar and the content, and the assistant while it is open), so the
//! entries are weights, not widths. Electron writes the pixel widths of the
//! row it saved from; native writes the widths it shows converted into the
//! file's own weights, so the two agree on a row of the same width. While
//! the assistant is closed its entry is the width it opens at (Allotment's
//! `preferredSize`).
//!
//! # Invariants
//!
//! - **I1** Clamping and minimums apply only to what is displayed
//!   ([`layout`]), never to the values written to `ui.paneSizes`.
//! - **I2** Dragging a divider changes only the two entries on either side
//!   of it and keeps their sum, so the other divider keeps its share of the
//!   row. The sidebar divider changes the sidebar and the content (the
//!   workspace beside it is the content plus the open assistant, whose
//!   entry is kept); the assistant divider changes the content and the
//!   assistant. The value written is the dragged visible width converted
//!   back into the file's weights ([`after_sidebar_drag`],
//!   [`after_assistant_drag`]).
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
//!   could not be read at all, the dragged layout stays until a later read
//!   (`buddy-app`'s `app.rs`); it is never rebuilt from in-memory sizes the
//!   file may no longer hold.

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
    let [sidebar, content, assistant] = entries(sizes);
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

/// The save for a drag of the sidebar divider (I2), from the widths the
/// drag left: the sidebar, the workspace beside it, and the assistant
/// painted inside the workspace (`None` while it was closed). The sidebar
/// takes the share of the file's weights that its visible width is of the
/// row, and the content the rest of the workspace's weight; the assistant
/// entry is kept, so its divider keeps its share of the row. Whether the
/// assistant was open is what the drag measured: Electron may have toggled
/// it in the file since.
///
/// A file whose assistant weight is wider than the workspace the drag left
/// cannot keep it: that only happens while the assistant paints held at
/// its 600 maximum (a window widened since the save), and the workspace's
/// weight is then shared as the assistant and the main pane were painted.
/// A width that is not positive (the split was never laid out) changes
/// nothing.
pub fn after_sidebar_drag(
    sizes: &[f64],
    sidebar: f64,
    workspace: f64,
    assistant: Option<f64>,
) -> Option<Vec<f64>> {
    let measured = |width: f64| width.is_finite() && width > 0.0;
    if !(measured(sidebar) && measured(workspace)) {
        return None;
    }
    let [weight, content, kept] = entries(sizes);
    let painted = assistant.filter(|width| measured(*width));
    let beside = if painted.is_some() { kept } else { 0.0 };
    let total = weight + content + beside;
    let sidebar = sidebar * total / (sidebar + workspace);
    let workspace_weight = total - sidebar;
    let content = workspace_weight - beside;
    if content > 0.0 {
        return Some(vec![sidebar, content, kept]);
    }
    let share = painted? / workspace;
    (share > 0.0 && share < 1.0).then(|| {
        vec![
            sidebar,
            workspace_weight * (1.0 - share),
            workspace_weight * share,
        ]
    })
}

/// The save for a drag of the assistant's divider (I2) that left the
/// assistant painted `assistant` pixels wide (read through
/// [`AssistantLimits::painted`]) in a workspace `workspace` wide: the
/// content and the assistant share their weights as the drag shows them,
/// and the sidebar entry is kept, so the sidebar keeps its share of the
/// row. A width that is not positive, or an assistant no narrower than
/// the workspace, changes nothing.
pub fn after_assistant_drag(sizes: &[f64], assistant: f64, workspace: f64) -> Option<Vec<f64>> {
    let measured = |width: f64| width.is_finite() && width > 0.0;
    if !(measured(assistant) && measured(workspace) && assistant < workspace) {
        return None;
    }
    let [sidebar, content, weight] = entries(sizes);
    let pair = content + weight;
    let assistant = assistant * pair / workspace;
    Some(vec![sidebar, pair - assistant, assistant])
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
        let shown = layout(&before, true, row(1600.0));
        let sidebar = shown.sidebar.unwrap().width;
        assert_close(sidebar, 300.0 / 980.0 * 1600.0);
        // The workspace has room for a 600 px assistant.
        let dragged = shown.assistant.unwrap().painted(600.0, shown.workspace);
        assert_eq!(dragged, 600.0);
        let saved = after_assistant_drag(&before, dragged, shown.workspace).unwrap();
        assert_eq!(saved[0], 300.0);
        assert_close(saved[1] + saved[2], 680.0);
        let after = layout(&saved, true, row(1600.0));
        assert_close(after.sidebar.unwrap().width, sidebar);
        assert_close(after.assistant.unwrap().assistant, 600.0);
        // Past the cap: GPUI Kit's 1,040 paints 600 and saves the same.
        let past = shown.assistant.unwrap().painted(1040.0, shown.workspace);
        assert_eq!(
            after_assistant_drag(&before, past, shown.workspace),
            Some(saved)
        );
        for (width, workspace) in [
            (0.0, 900.0),
            (-5.0, 900.0),
            (f64::NAN, 900.0),
            (900.0, 900.0),
        ] {
            assert_eq!(
                after_assistant_drag(&before, width, workspace),
                None,
                "{width} in {workspace}"
            );
        }
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

    /// I1: the minimums hold what is shown, never what is saved.
    #[test]
    fn the_minimums_hold_the_layout_not_the_file() {
        // [300, 400] is 700 of weight in a 1,600 px row: a drag to the
        // sidebar's 200 minimum saves 87.5, which lays out at exactly 200.
        let saved = after_sidebar_drag(&[300.0, 400.0], 200.0, 1400.0, None).unwrap();
        assert_close(saved[0], 87.5);
        assert_close(saved[1], 612.5);
        assert_close(sidebar_px(&saved, false, 1600.0), 200.0);
        // An assistant drag that leaves the main pane its 250 px saves a
        // content weight below 250.
        let before = [300.0, 200.0, 280.0];
        let shown = layout(&before, true, row(1600.0));
        let saved = after_assistant_drag(&before, 600.0, shown.workspace).unwrap();
        assert!(saved[1] < MAIN_BESIDE_ASSISTANT_MIN_WIDTH, "{saved:?}");
        assert_close(assistant_px(&saved, 1600.0), 600.0);
        // A weight wider than the 600 the assistant paints is kept as saved.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 1040.0], 400.0, 2000.0, None),
            Some(vec![200.0, 1000.0, 1040.0])
        );
    }

    /// I2 for the sidebar divider, over every file, row and target the
    /// limits allow, with the assistant open and closed.
    #[test]
    fn a_sidebar_drag_changes_only_the_sidebar_and_the_content() {
        for sizes in FILES {
            for &available in ROWS {
                for open in [false, true] {
                    let shown = layout(sizes, open, row(available));
                    let pane = shown.sidebar.unwrap();
                    if pane.limits.shrunk() {
                        // The shrunk minimums pin the divider; app.rs saves
                        // nothing.
                        continue;
                    }
                    let [sidebar, content, assistant] = entries(sizes);
                    let beside = if open { assistant } else { 0.0 };
                    let total = sidebar + content + beside;
                    let painted = shown.assistant.map(|limits| limits.assistant);
                    for target in targets(pane.limits.sidebar_min, pane.limits.sidebar_max) {
                        let case = format!("{sizes:?} open={open} in {available} to {target}");
                        let saved =
                            after_sidebar_drag(sizes, target, available - target, painted).unwrap();
                        assert_saved(&saved);
                        assert!(
                            close(
                                saved[0] + saved[1] + if open { saved[2] } else { 0.0 },
                                total
                            ),
                            "{case}: {saved:?}"
                        );
                        let after = layout(&saved, open, row(available));
                        assert!(
                            close(after.sidebar.unwrap().width, target),
                            "{case}: {saved:?} shows {after:?}"
                        );
                        if saved[2] == assistant {
                            continue;
                        }
                        // Only an assistant weight wider than the workspace
                        // the drag left is shared out instead.
                        assert!(open, "{case}: {saved:?}");
                        assert!(
                            assistant / total * available >= available - target,
                            "{case}: {saved:?}"
                        );
                        assert!(
                            close(after.assistant.unwrap().assistant, painted.unwrap()),
                            "{case}: {saved:?} shows {after:?}"
                        );
                    }
                }
            }
        }
    }

    /// I2 for the assistant divider: the sidebar entry and the sum of the
    /// other two are kept, and a restart shows what the drag showed.
    #[test]
    fn an_assistant_drag_changes_only_the_content_and_the_assistant() {
        for sizes in FILES {
            for &available in ROWS {
                let shown = layout(sizes, true, row(available));
                let limits = shown.assistant.unwrap();
                if limits.shrunk() {
                    continue;
                }
                let [sidebar, content, assistant] = entries(sizes);
                let most = limits.painted(f64::MAX, shown.workspace);
                for target in targets(limits.assistant_min, most) {
                    let case = format!("{sizes:?} in {available} to {target}");
                    let width = limits.painted(target, shown.workspace);
                    let saved = after_assistant_drag(sizes, width, shown.workspace).unwrap();
                    assert_saved(&saved);
                    assert_eq!(saved[0], sidebar, "{case}");
                    assert!(close(saved[1] + saved[2], content + assistant), "{case}");
                    let after = layout(&saved, true, row(available));
                    if at_share(sizes, true, available) {
                        assert!(
                            close(after.sidebar.unwrap().width, shown.sidebar.unwrap().width),
                            "{case}: {saved:?} shows {after:?}"
                        );
                        assert!(
                            close(after.assistant.unwrap().assistant, width),
                            "{case}: {saved:?} shows {after:?}"
                        );
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
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 0.0, 950.0, None), None);
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 250.0, -1.0, None), None);
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0], f64::NAN, 950.0, None),
            None
        );
        // Extra entries are dropped; an unreadable assistant saves its
        // default.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0, 5.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 0.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 350.0])
        );
    }
}
