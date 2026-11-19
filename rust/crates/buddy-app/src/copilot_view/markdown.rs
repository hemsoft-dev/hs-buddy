//! Assistant output is text. Media and raw HTML cannot initiate resource loads.
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::text::{
    MarkdownExtensions, MarkdownNode, MarkdownParseContext, MarkdownPlugin, TextView,
    TextViewStyle, markdown_ast,
};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    ClipboardItem, ElementId, IntoElement, ParentElement, StyleRefinement, Styled, div, px,
};

#[cfg(test)]
#[path = "markdown_copy_tests.rs"]
mod copy_tests;

struct SuppressedMedia {
    block: bool,
}

fn media_label(node: &markdown_ast::Node) -> Option<String> {
    match node {
        markdown_ast::Node::Image(image) => Some(format!(
            "Image: {}",
            image.alt.chars().take(200).collect::<String>()
        )),
        markdown_ast::Node::ImageReference(image) => Some(format!(
            "Image: {}",
            image.alt.chars().take(200).collect::<String>()
        )),
        markdown_ast::Node::Html(_) => Some("HTML content omitted".into()),
        _ => None,
    }
}
impl MarkdownPlugin for SuppressedMedia {
    fn is_block(&self) -> bool {
        self.block
    }
    fn name(&self) -> &str {
        if self.block {
            "buddy-safe-block"
        } else {
            "buddy-safe-inline"
        }
    }
    fn parse(
        &self,
        node: &markdown_ast::Node,
        _: &MarkdownParseContext<'_>,
    ) -> Option<MarkdownNode> {
        media_label(node).map(|label| MarkdownNode::new(self.name(), ()).text(label))
    }
}

fn web_link(value: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(value).ok()?;
    (matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none())
    .then(|| parsed.to_string())
}

pub(crate) fn response(id: impl Into<ElementId>, source: String) -> impl IntoElement {
    TextView::markdown(id, source)
        .selectable(true)
        .style(TextViewStyle::default().code_block(StyleRefinement::default().pt(px(44.))))
        .code_block_actions(|block, _, cx| {
            let code = block.code();
            h_flex()
                .gap_2()
                .children(block.lang().filter(|lang| !lang.is_empty()).map(|lang| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(lang)
                }))
                .child(
                    Button::new(("buddy-copy-code", block.span.map_or(0, |span| span.start)))
                        .small()
                        .label("Copy code")
                        .accessibility_label("Copy code")
                        .tab_stop(true)
                        .disabled(code.is_empty())
                        .on_click(move |_, _, cx| {
                            if !code.is_empty() {
                                // Match Electron's pre/code text, including its final newline.
                                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                    "{code}\n"
                                )));
                            }
                        }),
                )
        })
        .markdown_extensions(
            MarkdownExtensions::default()
                .plugin(SuppressedMedia { block: true })
                .plugin(SuppressedMedia { block: false })
                .parser_revision(1),
        )
        .on_link_click(|url, _, _, cx| {
            if let Some(url) = web_link(url) {
                cx.open_url(&url);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_explicit_web_links_can_open() {
        for value in [
            "file:///private",
            "javascript:alert(1)",
            "data:text/plain,private",
            "command:run",
            "https://user:secret@example.com",
        ] {
            assert!(web_link(value).is_none());
        }
        assert_eq!(
            web_link("https://github.com/owner/repo"),
            Some("https://github.com/owner/repo".into())
        );
    }
    #[test]
    fn remote_images_and_raw_html_become_literal_labels() {
        let image = markdown_ast::Node::Image(markdown_ast::Image {
            alt: "Synthetic 界".into(),
            url: "https://private.invalid/tracker".into(),
            title: None,
            position: None,
        });
        assert_eq!(media_label(&image), Some("Image: Synthetic 界".into()));
        let html = markdown_ast::Node::Html(markdown_ast::Html {
            value: "<img src='https://private.invalid/tracker'>".into(),
            position: None,
        });
        assert_eq!(media_label(&html), Some("HTML content omitted".into()));
    }
}
