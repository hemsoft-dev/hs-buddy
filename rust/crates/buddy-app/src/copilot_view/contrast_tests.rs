//! Every colour pair the Recent Results section and the premium usage badge
//! draw, measured in Buddy Light and Buddy Dark: text at 4.5:1, status icons
//! and the quota bar at the 3:1 that non-text needs (PR #1028 rounds 3-4).
use super::*;
use crate::theme::wcag::{contrast, over};
use buddy_core::application_settings::LocalEdit;
use buddy_core::config::{AppConfig, ThemeName};
use buddy_core::copilot_results::Status;
use gpui_kit::Hsla;
use gpui_kit::component::theme::Theme;

#[gpui_kit::test]
fn every_recent_results_and_badge_colour_keeps_its_contrast_in_both_themes(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let mut failures = Vec::new();
        let mut measured = 0;
        for mode in [ThemeName::Light, ThemeName::Dark] {
            // Selecting a theme applies its incumbent color preset.
            let mut config = AppConfig::default();
            LocalEdit::Theme(mode).apply(&mut config).unwrap();
            crate::theme::install(&config, cx);
            let theme = Theme::global(cx);
            let mut check = |what: &str, color: Hsla, background: Hsla, minimum: f32| {
                measured += 1;
                let ratio = contrast(over(color, background), background);
                if ratio < minimum {
                    failures.push(format!("{mode:?} {what}: {ratio:.2}:1, needs {minimum}:1"));
                }
            };
            // The sidebar sits on the window background. gpui-component
            // 0.8.1's ghost button, used for the header and each row, is
            // transparent at rest, `accent` on hover (half of it in dark)
            // and `secondary_active` when selected, with these labels.
            let page = theme.background;
            let hover = if theme.mode.is_dark() {
                (*theme.tokens.accent).opacity(0.5)
            } else {
                *theme.tokens.accent
            };
            let rows = [
                ("row", page, theme.secondary_foreground),
                ("hovered row", over(hover, page), theme.accent_foreground),
                (
                    "selected row",
                    over(*theme.tokens.secondary_active, page),
                    theme.secondary_foreground,
                ),
            ];
            let quiet = palette::quiet_text(theme);
            for (state, surface, label) in rows {
                // The header and row labels, and the chevron that takes
                // the label's colour.
                check(&format!("{state} label"), label, surface, 4.5);
                check(&format!("{state} age"), quiet, surface, 4.5);
                for status in [
                    Status::Pending,
                    Status::Running,
                    Status::Completed,
                    Status::Failed,
                ] {
                    let icon = recent::status_color(status, theme);
                    check(&format!("{state} {status:?} icon"), icon, surface, 3.0);
                }
            }
            // "No results yet", "could not be refreshed" and the partial note.
            check("section note", quiet, page, 4.5);
            let badge_surface = over(theme.secondary, page);
            for percent_remaining in [60.0, 40.0, 20.0, 5.0] {
                let quota = premium_badge::Quota {
                    remaining: percent_remaining,
                    entitlement: 100.0,
                    percent_used: 100.0 - percent_remaining,
                };
                let colors = premium_badge::colors(&quota, theme);
                assert_eq!(colors.background, theme.secondary);
                let badge = over(colors.background, page);
                assert_eq!(badge, badge_surface);
                let tier = format!("{:#x} tier", quota.color());
                check(&format!("figures, {tier}"), colors.figures, badge, 4.5);
                check(&format!("zap icon, {tier}"), colors.tier, badge, 3.0);
                // The fill is drawn on the bar's track.
                let track = over(colors.track, badge);
                check(&format!("bar fill, {tier}"), colors.tier, track, 3.0);
            }
        }
        assert_eq!(measured, 2 * (3 * 6 + 1 + 4 * 3));
        assert!(failures.is_empty(), "below the minimum: {failures:#?}");
    });
}

#[gpui_kit::test]
fn dark_keeps_electrons_hues_and_light_darkens_the_same_ones(cx: &mut gpui_kit::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for (mode, expected) in [
            (ThemeName::Dark, [0xe89b3c, 0x4ec9b0, 0xe85d5d, 0xdcd34a]),
            (ThemeName::Light, [0xb45309, 0x0f766e, 0xc42b2b, 0x8a7a00]),
        ] {
            let mut config = AppConfig::default();
            LocalEdit::Theme(mode).apply(&mut config).unwrap();
            crate::theme::install(&config, cx);
            let theme = Theme::global(cx);
            let shown: Vec<Hsla> = [
                palette::ORANGE,
                palette::TEAL,
                palette::RED,
                palette::YELLOW,
            ]
            .into_iter()
            .map(|electron| palette::legible(electron, theme))
            .collect();
            let expected: Vec<Hsla> = expected
                .into_iter()
                .map(|hex| gpui_kit::rgb(hex).into())
                .collect();
            assert_eq!(shown, expected, "{mode:?}");
            // Each status takes its hue; running is the theme's info blue.
            assert_eq!(recent::status_color(Status::Pending, theme), shown[0]);
            assert_eq!(recent::status_color(Status::Completed, theme), shown[1]);
            assert_eq!(recent::status_color(Status::Failed, theme), shown[2]);
            assert_eq!(recent::status_color(Status::Running, theme), theme.info);
        }
    });
}
