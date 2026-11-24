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

/// The sidebar width for a row `available` pixels wide. Allotment shares
/// the row in proportion to the saved sizes of the panes on screen
/// (sidebar and content, plus the assistant while it is open), then holds
/// the sidebar at 200 or more and leaves the content at least 400.
pub fn sidebar_width(sizes: &[f64], assistant_open: bool, available: f64) -> f64 {
    let sizes = normalize(sizes);
    let panes = if assistant_open { 3 } else { 2 };
    let total: f64 = sizes.iter().take(panes).sum();
    let share = sizes[0] / total * available;
    share
        .min(available - CONTENT_MIN_WIDTH)
        .max(SIDEBAR_MIN_WIDTH)
}

/// `handlePaneChange` for a drag of the sidebar divider: the sidebar and
/// content widths replace the first two entries of the loaded (normalized)
/// sizes and the assistant width is kept. Widths that are not positive
/// change nothing.
pub fn after_sidebar_drag(sizes: &[f64], sidebar: f64, content: f64) -> Option<Vec<f64>> {
    if !(sidebar.is_finite() && sidebar > 0.0 && content.is_finite() && content > 0.0) {
        return None;
    }
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

    #[test]
    fn the_sidebar_takes_its_saved_share_of_the_row() {
        assert_eq!(sidebar_width(&[300.0, 900.0], false, 1200.0), 300.0);
        assert_eq!(sidebar_width(&[300.0, 900.0], false, 1600.0), 400.0);
        // The assistant's saved width joins the share while it is open.
        assert_eq!(sidebar_width(&[400.0, 1000.0, 600.0], true, 1000.0), 200.0);
        assert_eq!(sidebar_width(&[400.0, 1000.0, 600.0], false, 1400.0), 400.0);
        // An unreadable value lays out as the defaults.
        assert_eq!(sidebar_width(&[0.0, 900.0], false, 1200.0), 300.0);
    }

    #[test]
    fn the_sidebar_stays_within_the_pane_limits() {
        assert_eq!(sidebar_width(&[10.0, 990.0], false, 1000.0), 200.0);
        assert_eq!(sidebar_width(&[900.0, 100.0], false, 1000.0), 600.0);
        // A row too narrow for both minimums keeps the sidebar's.
        assert_eq!(sidebar_width(&[300.0, 900.0], false, 500.0), 200.0);
    }

    #[test]
    fn a_drag_saves_both_widths_and_keeps_the_assistant() {
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0], 250.0, 950.0),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0], 250.0, 950.0),
            Some(vec![250.0, 950.0, 350.0])
        );
        // Extra entries are dropped; an unreadable value saves the default.
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 420.0, 5.0], 250.0, 950.0),
            Some(vec![250.0, 950.0, 420.0])
        );
        assert_eq!(
            after_sidebar_drag(&[300.0, 900.0, 0.0], 250.0, 950.0),
            Some(vec![250.0, 950.0, 350.0])
        );
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 0.0, 950.0), None);
        assert_eq!(after_sidebar_drag(&[300.0, 900.0], 250.0, -1.0), None);
    }
}
