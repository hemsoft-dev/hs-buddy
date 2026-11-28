//! Electron's status and quota colours on Buddy's surfaces.
//!
//! Electron draws fixed hues (`CopilotResultPanel.css`, `quotaUtils.ts`)
//! made for a dark surface. Buddy Dark keeps them. On Buddy Light they
//! measured as low as 1.3:1, below the 3:1 that status icons and the quota
//! bar need, so Light uses the same hue darkened until it keeps 3:1 on every
//! surface it is drawn on: a sidebar row at rest, hovered or selected, and
//! the badge and its bar track.
use gpui_kit::component::theme::Theme;
use gpui_kit::{Hsla, rgb};

/// `.status-completed` and `getQuotaColor` below 50%.
pub(super) const TEAL: u32 = 0x4ec9b0;
/// `getQuotaColor` from 50%.
pub(super) const YELLOW: u32 = 0xdcd34a;
/// `.status-pending` and `getQuotaColor` from 75%.
pub(super) const ORANGE: u32 = 0xe89b3c;
/// `.status-failed` and `getQuotaColor` from 90%.
pub(super) const RED: u32 = 0xe85d5d;

/// One of Electron's colours as Buddy draws it in the active theme.
pub(super) fn legible(electron: u32, theme: &Theme) -> Hsla {
    if theme.mode.is_dark() {
        return rgb(electron).into();
    }
    rgb(match electron {
        TEAL => 0x0f766e,
        YELLOW => 0x8a7a00,
        ORANGE => 0xb45309,
        RED => 0xc42b2b,
        other => other,
    })
    .into()
}

/// Secondary text, such as a row's age or a note under the list. Electron
/// uses `--text-secondary` at 70%; `muted.foreground` (that colour here)
/// measured 3.2:1 on a selected row in Buddy Dark, so this is the theme's
/// foreground at 75%, which keeps 4.5:1 on every row state in both themes.
pub(super) fn quiet_text(theme: &Theme) -> Hsla {
    theme.secondary_foreground.opacity(0.75)
}
