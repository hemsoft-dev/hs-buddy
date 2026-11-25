//! The shell split saved in `ui.paneSizes`: `[sidebar, content, assistant]`
//! widths in pixels at 100% zoom, as Electron's `useAppLayout` keeps them
//! (`src/appUtils.ts`, `src/hooks/useAppLayout.ts`).
//!
//! Electron hands the saved sizes to Allotment as `defaultSizes`, which
//! shares the available width among the panes in proportion to them, so the
//! sidebar's width is its share of the row, not an absolute width.

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

/// The share of the minimums `wanted` that a row `available` pixels wide
/// can hold: 1 when it holds them all, less in proportion when it cannot.
fn fit(available: f64, wanted: f64) -> f64 {
    let available = if available.is_finite() {
        available.max(0.0)
    } else {
        0.0
    };
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
        let available = if available.is_finite() {
            available.max(0.0)
        } else {
            0.0
        };
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

/// The limits of the split between the main pane and the open assistant,
/// in a workspace whose minimums shrank by `scale` (from
/// [`SidebarLimits::scale`] or [`workspace_scale`]), for an assistant
/// `assistant` pixels wide.
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
    /// The assistant's width on its first frame.
    pub assistant: f64,
}

impl AssistantLimits {
    pub fn new(scale: f64, assistant: f64) -> Self {
        let scale = if scale.is_finite() {
            scale.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let assistant = if assistant.is_finite() {
            assistant.clamp(ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH)
        } else {
            DEFAULT_ASSISTANT_PANE_SIZE
        };
        let (assistant_min, assistant_max) = if scale < 1.0 {
            (assistant * scale, assistant * scale)
        } else {
            (ASSISTANT_MIN_WIDTH, ASSISTANT_MAX_WIDTH)
        };
        Self {
            main_min: MAIN_BESIDE_ASSISTANT_MIN_WIDTH * scale,
            assistant_min,
            assistant_max,
            assistant: assistant * scale,
        }
    }

    /// Whether the minimums shrank, which holds the assistant at its shrunk
    /// width.
    pub fn shrunk(&self) -> bool {
        self.main_min < MAIN_BESIDE_ASSISTANT_MIN_WIDTH
    }
}

/// The sidebar width for a row `available` pixels wide. Allotment shares
/// the row in proportion to the saved sizes of the panes on screen
/// (sidebar and content, plus the assistant while it is open), then holds
/// the sidebar within `limits`.
pub fn sidebar_width(
    sizes: &[f64],
    assistant_open: bool,
    available: f64,
    limits: SidebarLimits,
) -> f64 {
    let sizes = normalize(sizes);
    let panes = if assistant_open { 3 } else { 2 };
    let total: f64 = sizes.iter().take(panes).sum();
    let share = sizes[0] / total * available;
    share.clamp(limits.sidebar_min, limits.sidebar_max)
}

/// `handlePaneChange` for a drag of the sidebar divider, from the widths
/// the drag measured: the sidebar, the workspace beside it, and the
/// assistant painted inside the workspace (`None` while it was closed).
/// Electron's content pane is the workspace without the assistant; beside
/// an assistant it is at least the main pane's minimum, which the nested
/// split holds it to. The sidebar and content replace the first two
/// entries of the loaded (normalized) sizes and the assistant entry is
/// kept. A width that is not positive (the split was never laid out)
/// changes nothing.
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
    let content = match assistant.filter(|width| measured(*width)) {
        Some(assistant) => (workspace - assistant).max(MAIN_BESIDE_ASSISTANT_MIN_WIDTH),
        None => workspace,
    };
    Some(vec![sidebar, content, normalize(sizes)[2]])
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    /// The limits for a wide row: Electron's minimums.
    const WIDE: SidebarLimits = SidebarLimits {
        sidebar_min: 200.0,
        sidebar_max: f64::MAX,
        workspace_min: 400.0,
        scale: 1.0,
    };

    #[test]
    fn the_sidebar_takes_its_saved_share_of_the_row() {
        assert_eq!(sidebar_width(&[300.0, 900.0], false, 1200.0, WIDE), 300.0);
        assert_eq!(sidebar_width(&[300.0, 900.0], false, 1600.0, WIDE), 400.0);
        // The assistant's saved width joins the share while it is open.
        assert_eq!(
            sidebar_width(&[400.0, 1000.0, 600.0], true, 1000.0, WIDE),
            200.0
        );
        assert_eq!(
            sidebar_width(&[400.0, 1000.0, 600.0], false, 1400.0, WIDE),
            400.0
        );
        // An unreadable value lays out as the defaults.
        assert_eq!(sidebar_width(&[0.0, 900.0], false, 1200.0, WIDE), 300.0);
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
        assert_eq!(sidebar_width(&[10.0, 990.0], false, 1000.0, limits), 200.0);
        assert_eq!(sidebar_width(&[900.0, 100.0], false, 1000.0, limits), 600.0);
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
        assert_eq!(
            sidebar_width(&[900.0, 300.0, 600.0], true, 1200.0, limits),
            350.0
        );
        // A narrow assistant still leaves the content Electron's 400.
        assert_eq!(SidebarLimits::new(1200.0, Some(100.0)).workspace_min, 400.0);
    }

    #[test]
    fn a_row_too_narrow_for_both_minimums_shrinks_them_to_fit() {
        // A 900 px window at 150%: 552 px at 100% beside the activity bar.
        let limits = SidebarLimits::new(552.0, None);
        assert!((limits.sidebar_min - 184.0).abs() < 1e-9, "{limits:?}");
        assert!((limits.workspace_min - 368.0).abs() < 1e-9, "{limits:?}");
        assert!(limits.sidebar_max >= limits.sidebar_min, "{limits:?}");
        assert!(limits.sidebar_min + limits.workspace_min <= 552.0 + 1e-9);
        assert!((sidebar_width(&[300.0, 900.0], false, 552.0, limits) - 184.0).abs() < 1e-9);
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
    }

    #[test]
    fn a_drag_saves_both_widths_and_keeps_the_assistant() {
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 350.0])
        );
        // Extra entries are dropped; an unreadable value saves the default.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0, 5.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 0.0], 250.0, 950.0, None),
            Some(vec![250.0, 950.0, 350.0])
        );
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 0.0, 950.0, None), None);
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 250.0, -1.0, None), None);
    }

    #[test]
    fn the_content_is_the_workspace_without_the_painted_assistant() {
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 600.0], 350.0, 850.0, Some(600.0)),
            Some(vec![350.0, 250.0, 600.0])
        );
        // A workspace squeezed past its limits never saves a negative or
        // empty content; the main pane keeps its minimum.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 600.0], 800.0, 400.0, Some(600.0)),
            Some(vec![800.0, 250.0, 600.0])
        );
        // The assistant entry saved is the file's, whatever was painted.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0], 300.0, 900.0, Some(350.0)),
            Some(vec![300.0, 550.0, 420.0])
        );
    }
}
