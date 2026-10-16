use buddy_core::terminal::{CellColor, ScreenSnapshot, TerminalSize};
use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, FontStyle,
    FontWeight, GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad,
    Pixels, Point, ShapedLine, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, px,
    relative, rgb, size,
};

use super::TerminalGrid;

pub(super) struct GridElement {
    pub grid: Entity<TerminalGrid>,
}

pub(super) struct Prepaint {
    lines: Vec<(ShapedLine, Point<Pixels>)>,
    backgrounds: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    height: Pixels,
    background: Hsla,
}

fn color(
    value: CellColor,
    screen: &ScreenSnapshot,
    foreground: Hsla,
    background: Hsla,
    light: bool,
) -> Hsla {
    let index = match value {
        CellColor::Rgb(r, g, b) => {
            return rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into();
        }
        CellColor::Palette(index) => usize::from(index),
    };
    if let Some(Some([r, g, b])) = screen.colors.get(index) {
        return rgb((u32::from(*r) << 16) | (u32::from(*g) << 8) | u32::from(*b)).into();
    }
    const DARK: [u32; 16] = [
        0x1e1e1e, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
        0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
    ];
    const LIGHT: [u32; 16] = [
        0x222222, 0xa31515, 0x008000, 0x795e26, 0x0451a5, 0xaf00db, 0x007f7f, 0x666666, 0x808080,
        0xd02020, 0x008000, 0x795e26, 0x0060c0, 0xaf00db, 0x007f7f, 0x999999,
    ];
    match index {
        0..=15 => rgb(if light { LIGHT[index] } else { DARK[index] }).into(),
        16..=231 => {
            let index = index - 16;
            let level = |value| if value == 0 { 0 } else { 55 + value * 40 };
            rgb(((level(index / 36) << 16) | (level(index / 6 % 6) << 8) | level(index % 6)) as u32)
                .into()
        }
        232..=255 => {
            let value = (8 + (index - 232) * 10) as u32;
            rgb((value << 16) | (value << 8) | value).into()
        }
        257 => background,
        _ => foreground,
    }
}

impl IntoElement for GridElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let theme = cx.theme();
        let foreground = theme.foreground;
        let default_foreground = foreground;
        let background = theme.background;
        let selection_color = theme.primary;
        let light = background.l > 0.5;
        let mut font = window.text_style().font();
        font.family = theme.mono_font_family.clone();
        let font_size = crate::zoom::scaled(cx, 14.0);
        let height = crate::zoom::scaled(cx, 20.0);
        let base = TextRun {
            len: 1,
            font: font.clone(),
            color: foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = window
            .text_system()
            .shape_line("M".into(), font_size, std::slice::from_ref(&base), None)
            .width
            .max(px(1.0));
        let columns = (bounds.size.width / width).floor().clamp(2.0, 240.0) as u16;
        let rows = (bounds.size.height / height).floor().clamp(2.0, 100.0) as u16;
        self.grid.update(cx, |grid, _| {
            grid.bounds = Some(bounds);
            grid.cell_size = size(width, height);
            if let Ok(size) = TerminalSize::new(columns, rows) {
                let _ = grid.session.resize(size);
            }
        });
        let grid = self.grid.read(cx);
        let screen = grid.screen.clone();
        let selected = grid.selection_range();
        let mut result = Prepaint {
            lines: Vec::new(),
            backgrounds: Vec::new(),
            cursor: None,
            height,
            background,
        };
        for cell in &screen.cells {
            let position = point(
                bounds.left() + width * cell.column as f32,
                bounds.top() + height * cell.row as f32,
            );
            let mut foreground = color(
                cell.style.foreground,
                &screen,
                foreground,
                background,
                light,
            );
            let mut cell_background = color(
                cell.style.background,
                &screen,
                default_foreground,
                background,
                light,
            );
            if cell.style.inverse {
                std::mem::swap(&mut foreground, &mut cell_background);
            }
            if cell.style.dim {
                foreground.a *= 0.65;
            }
            let index = cell.row * usize::from(screen.size.columns()) + cell.column;
            if selected
                .as_ref()
                .is_some_and(|range| index < range.end && index + cell.width > range.start)
            {
                cell_background = selection_color;
                cell_background.a = 0.35;
            }
            if cell_background != background {
                result.backgrounds.push(fill(
                    Bounds::new(position, size(width * cell.width as f32, height)),
                    cell_background,
                ));
            }
            if cell.style.hidden || cell.text.is_empty() || cell.text == " " {
                continue;
            }
            let mut font = font.clone();
            if cell.style.bold {
                font.weight = FontWeight::BOLD;
            }
            if cell.style.italic {
                font.style = FontStyle::Italic;
            }
            let run = TextRun {
                len: cell.text.len(),
                font,
                color: foreground,
                background_color: None,
                underline: cell.style.underline.then_some(UnderlineStyle {
                    color: Some(foreground),
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: cell
                    .style
                    .strikeout
                    .then_some(gpui_kit::StrikethroughStyle {
                        thickness: px(1.0),
                        color: Some(foreground),
                    }),
            };
            result.lines.push((
                window
                    .text_system()
                    .shape_line(cell.text.clone().into(), font_size, &[run], None),
                position,
            ));
        }
        if let Some((row, column)) = screen.cursor {
            let position = point(
                bounds.left() + width * column as f32,
                bounds.top() + height * row as f32,
            );
            if grid.focus.is_focused(window) {
                let mut cursor = foreground;
                cursor.a = 0.35;
                result.cursor = Some(fill(Bounds::new(position, size(width, height)), cursor));
            }
            if !grid.composition.is_empty() {
                let run = TextRun {
                    len: grid.composition.len(),
                    underline: Some(UnderlineStyle {
                        color: Some(foreground),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base
                };
                result.lines.push((
                    window.text_system().shape_line(
                        grid.composition.clone().into(),
                        font_size,
                        &[run],
                        None,
                    ),
                    position,
                ));
            }
        }
        result
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.grid.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.grid.clone()),
            cx,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, state.background));
            for background in state.backgrounds.drain(..) {
                window.paint_quad(background);
            }
            for (line, position) in &state.lines {
                let _ = line.paint(*position, state.height, TextAlign::Left, None, window, cx);
            }
            if let Some(cursor) = state.cursor.take() {
                window.paint_quad(cursor);
            }
        });
    }
}
