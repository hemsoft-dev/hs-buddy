//! Buddy theme: the CSS variables from `src/index.css` expressed as a
//! gpui-component theme, plus the user's appearance overrides from config.

use buddy_core::config::{AppConfig, ThemeName};
use gpui_kit::component::theme::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Hsla, Rgba, SharedString};

const BUDDY_THEMES: &str = include_str!("../assets/themes/buddy.json");
const DARK_THEME_NAME: &str = "Buddy Dark";
const LIGHT_THEME_NAME: &str = "Buddy Light";

/// Colors the Electron CSS defines that have no gpui-component equivalent.
/// Some entries are not read yet; later views (settings, PR lists) use them.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct BuddyPalette {
    pub text_heading: Hsla,
    pub text_secondary: Hsla,
    pub text_muted: Hsla,
    pub bg_hover: Hsla,
    pub bg_tertiary: Hsla,
    pub border_secondary: Hsla,
    pub activity_bar_bg: Hsla,
    pub activity_bar_fg: Hsla,
    pub activity_bar_fg_active: Hsla,
    pub activity_bar_hover: Hsla,
    pub status_bar_fg: Hsla,
    pub accent_success: Hsla,
    pub accent_warning: Hsla,
    pub accent_error: Hsla,
    pub accent_purple: Hsla,
    pub accent_blue: Hsla,
    pub gold: Hsla,
    pub orange: Hsla,
    pub gold_soft: Hsla,
}

impl Global for BuddyPalette {}

impl BuddyPalette {
    fn dark() -> Self {
        Self {
            text_heading: hex("#ffffff"),
            text_secondary: hex("#858585"),
            text_muted: hex("#6e6e6e"),
            bg_hover: hex("#2a2d2e"),
            bg_tertiary: hex("#2d2d30"),
            border_secondary: hex("#3e3e42"),
            activity_bar_bg: hex("#333333"),
            activity_bar_fg: hex("#858585"),
            activity_bar_fg_active: hex("#ffffff"),
            activity_bar_hover: rgba_hex(0xffffff, 0.05),
            status_bar_fg: hex("#9d9d9d"),
            accent_success: hex("#4ec9b0"),
            accent_warning: hex("#dcdcaa"),
            accent_error: hex("#f48771"),
            accent_purple: hex("#c792ea"),
            accent_blue: hex("#569cd6"),
            gold: hex("#ffd700"),
            orange: hex("#ff9500"),
            gold_soft: hex("#ffb347"),
        }
    }

    fn light() -> Self {
        Self {
            text_heading: hex("#000000"),
            text_secondary: hex("#616161"),
            text_muted: hex("#8b8b8b"),
            bg_hover: rgba_hex(0x000000, 0.04),
            bg_tertiary: hex("#ececec"),
            border_secondary: hex("#d4d4d4"),
            activity_bar_bg: hex("#2c2c2c"),
            activity_bar_fg: hex("#858585"),
            activity_bar_fg_active: hex("#ffffff"),
            activity_bar_hover: rgba_hex(0xffffff, 0.1),
            status_bar_fg: hex("#616161"),
            accent_success: hex("#107c10"),
            accent_warning: hex("#ca5010"),
            accent_error: hex("#d13438"),
            accent_purple: hex("#5c2d91"),
            accent_blue: hex("#0066cc"),
            gold: hex("#9a6700"),
            orange: hex("#ff9500"),
            gold_soft: hex("#ffb347"),
        }
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

/// Parse a `#rrggbb` or `#rrggbbaa` string; falls back to magenta so a typo
/// is visible rather than silently black.
pub fn hex(value: &str) -> Hsla {
    try_hex(value).unwrap_or_else(|| hex("#ff00ff"))
}

pub fn try_hex(value: &str) -> Option<Hsla> {
    Rgba::try_from(value.trim()).ok().map(Into::into)
}

fn rgba_hex(rgb: u32, alpha: f32) -> Hsla {
    let mut color: Hsla = gpui_kit::rgb(rgb).into();
    color.a = alpha;
    color
}

/// Port of `lightenColor(hex, percent)` from `appearanceUtils.ts`: adds
/// `percent` of full scale to every RGB channel, clamped to white.
pub fn lighten(color: Hsla, percent: f32) -> Hsla {
    let rgba = color.to_rgb();
    let delta = percent / 100.0;
    Rgba {
        r: (rgba.r + delta).min(1.0),
        g: (rgba.g + delta).min(1.0),
        b: (rgba.b + delta).min(1.0),
        a: rgba.a,
    }
    .into()
}

fn darken(color: Hsla, percent: f32) -> Hsla {
    let rgba = color.to_rgb();
    let f = 1.0 - percent / 100.0;
    Rgba {
        r: rgba.r * f,
        g: rgba.g * f,
        b: rgba.b * f,
        a: rgba.a,
    }
    .into()
}

/// Register the Buddy themes, select the configured mode, then overlay the
/// user's appearance settings the same way `useAppAppearance.ts` does.
pub fn install(config: &AppConfig, cx: &mut App) {
    if let Err(err) = ThemeRegistry::global_mut(cx).load_themes_from_str(BUDDY_THEMES) {
        log::error!("failed to load bundled themes: {err:#}");
    }

    let registry = ThemeRegistry::global(cx);
    let dark = registry.themes().get(DARK_THEME_NAME).cloned();
    let light = registry.themes().get(LIGHT_THEME_NAME).cloned();
    Theme::update(cx, |theme| {
        if let Some(dark) = dark {
            theme.dark_theme = dark;
        }
        if let Some(light) = light {
            theme.light_theme = light;
        }
    });

    let mode = match config.ui.theme {
        ThemeName::Dark => ThemeMode::Dark,
        ThemeName::Light => ThemeMode::Light,
    };
    Theme::change(mode, None, cx);

    let mut palette = if mode.is_dark() {
        BuddyPalette::dark()
    } else {
        BuddyPalette::light()
    };
    apply_user_overrides(config, &mut palette, cx);
    cx.set_global(palette);
    crate::zoom::theme_installed(cx);
}

fn apply_user_overrides(config: &AppConfig, palette: &mut BuddyPalette, cx: &mut App) {
    let ui = &config.ui;
    let accent = try_hex(&ui.accent_color);
    let bg_primary = try_hex(&ui.bg_primary);
    let bg_secondary = try_hex(&ui.bg_secondary);
    let font_color = try_hex(&ui.font_color);
    let status_bar_bg = try_hex(&ui.status_bar_bg);
    let status_bar_fg = try_hex(&ui.status_bar_fg);

    let installed = cx.text_system().all_font_names();
    let font_family = installed
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&ui.font_family))
        .then(|| SharedString::from(ui.font_family.clone()));
    let mono_font_family = installed
        .iter()
        .find(|name| name.eq_ignore_ascii_case(&ui.mono_font_family))
        .map(|name| SharedString::from(name.clone()))
        // Theme::update resolves the Kit's platform default against installed
        // alternatives. Do not retain the bundled Cascadia name when absent.
        .unwrap_or_else(|| gpui_kit::base::TypographyTokens::default().mono);

    if let Some(color) = font_color {
        palette.text_heading = lighten(color, 20.0);
    }
    if let Some(color) = status_bar_fg {
        palette.status_bar_fg = color;
    }

    Theme::update(cx, |theme| {
        // Buddy's custom palettes omit syntax colors. Match the editor's
        // fallback to the active mode, including after a theme change.
        theme.highlight_theme = if theme.mode.is_dark() {
            gpui_kit::component::highlighter::HighlightTheme::default_dark()
        } else {
            gpui_kit::component::highlighter::HighlightTheme::default_light()
        };
        if let Some(accent) = accent {
            theme.colors.primary = accent;
            theme.colors.primary_hover = lighten(accent, 15.0);
            theme.colors.primary_active = darken(accent, 15.0);
            theme.colors.ring = accent;
            theme.colors.sidebar_primary = accent;
        }
        if let Some(bg) = bg_primary {
            theme.colors.background = bg;
            theme.colors.list = bg;
            theme.colors.tab_active = bg;
        }
        if let Some(bg) = bg_secondary {
            theme.colors.secondary = bg;
            theme.colors.sidebar = bg;
            theme.colors.popover = bg;
            theme.colors.tab_bar = bg;
        }
        if let Some(fg) = font_color {
            theme.colors.foreground = fg;
            theme.colors.secondary_foreground = fg;
            theme.colors.sidebar_foreground = fg;
            theme.colors.popover_foreground = fg;
        }
        if let Some(bg) = status_bar_bg {
            theme.colors.status_bar = bg;
        }
        if let Some(family) = font_family {
            theme.font_family = family;
        }
        theme.mono_font_family = mono_font_family;
    });
}

/// WCAG 2 contrast helpers shared by the color tests.
#[cfg(test)]
pub(crate) mod wcag {
    use gpui_kit::{Hsla, Rgba};

    /// Relative luminance, not HSL lightness.
    pub fn luminance(color: Hsla) -> f32 {
        let rgb = color.to_rgb();
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgb.r) + 0.7152 * linear(rgb.g) + 0.0722 * linear(rgb.b)
    }

    /// Composite a translucent `top` over an opaque `base`.
    pub fn over(top: Hsla, base: Hsla) -> Hsla {
        let (top, base) = (top.to_rgb(), base.to_rgb());
        let mix = |a: f32, b: f32| a * top.a + b * (1. - top.a);
        Rgba {
            r: mix(top.r, base.r),
            g: mix(top.g, base.g),
            b: mix(top.b, base.b),
            a: 1.,
        }
        .into()
    }

    pub fn contrast(text: Hsla, background: Hsla) -> f32 {
        let (a, b) = (luminance(text), luminance(background));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}

#[cfg(test)]
mod tests {
    use super::wcag::{contrast, over};
    use super::*;
    use buddy_core::application_settings::LocalEdit;

    #[gpui_kit::test]
    fn danger_button_labels_meet_text_contrast(cx: &mut gpui_kit::TestAppContext) {
        // Issue #854: the stock danger button drew `danger` on a 20% `danger`
        // tint, 3.6:1 in light. These mirror gpui-component 0.7.1's
        // `ButtonVariant::Danger` styles, which are private to the crate.
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut failures = Vec::new();
            for mode in [ThemeName::Dark, ThemeName::Light] {
                // Selecting a theme applies its incumbent color preset.
                let mut config = AppConfig::default();
                LocalEdit::Theme(mode).apply(&mut config).unwrap();
                install(&config, cx);
                let theme = Theme::global(cx);
                let (label, outline) = (theme.button_danger_foreground, theme.danger);
                let states = [
                    // `.danger()`: label and background per state.
                    ("normal", label, theme.button_danger),
                    ("hover", label, theme.button_danger_hover),
                    ("pressed", label, theme.button_danger_active),
                    // `.danger().outline()` ("Close shell"): `danger` on a faint tint.
                    ("outline", outline, theme.danger.opacity(0.1)),
                    ("outline hover", outline, theme.danger_hover.opacity(0.2)),
                ];
                for (state, text, background) in states {
                    let ratio = contrast(text, over(background, theme.background));
                    if ratio < 4.5 {
                        failures.push(format!("{mode:?} {state} {ratio:.2}:1"));
                    }
                }
            }
            assert!(failures.is_empty(), "below 4.5:1: {failures:?}");
        });
    }

    #[gpui_kit::test]
    fn unavailable_code_font_uses_resolved_platform_default(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let expected = Theme::global(cx).mono_font_family.clone();
            let mut config = AppConfig::default();
            config.ui.mono_font_family = "__Buddy_Uninstalled_Mono_Fixture__".into();

            install(&config, cx);
            assert_eq!(Theme::global(cx).mono_font_family, expected);

            config.ui.theme = ThemeName::Light;
            install(&config, cx);
            assert_eq!(Theme::global(cx).mono_font_family, expected);
        });
    }

    #[test]
    fn light_palette_gold_meets_small_text_contrast() {
        // The 13px dashboard subtitle uses this color on the light preset's
        // white background.
        let contrast = contrast(BuddyPalette::light().gold, hex("#ffffff"));
        assert!(contrast >= 4.5, "light subtitle contrast is {contrast}:1");
    }
}
