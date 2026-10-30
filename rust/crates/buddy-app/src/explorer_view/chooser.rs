use super::*;
use gpui_kit::component::input::{Input, InputEvent};

impl ExplorerView {
    pub fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Absolute folder path")
                .default_value(self.root.to_string_lossy().into_owned())
        });
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_folder(window, cx);
            }
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));
        self.chooser = Some(input);
        self.chooser_error = None;
        cx.notify();
    }

    fn submit_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.chooser else {
            return;
        };
        let path = PathBuf::from(input.read(cx).value().trim());
        if !path.is_absolute() {
            self.chooser_error = Some("Enter an absolute folder path.".into());
            cx.notify();
            return;
        }
        self.open(path, cx);
        if let Some(focus) = &self.focus {
            window.focus(focus, cx);
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        let chooser = self.chooser.as_ref().map(|input| input.entity_id());
        let paths = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await;
            let _ = this.update(cx, |this, cx| {
                if this.chooser.as_ref().map(|input| input.entity_id()) != chooser {
                    return;
                }
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.open(path, cx);
                        }
                    }
                    Ok(Ok(None)) => {}
                    _ => {
                        this.chooser_error =
                            Some("The folder chooser is unavailable. Enter the path above.".into())
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn chooser_form(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut form = v_flex().p(zpx(24.)).gap(zpx(12.)).child("Open Folder");
        if let Some(input) = &self.chooser {
            form = form.child(Input::new(input).aria_label("Folder path"));
        }
        if let Some(error) = &self.chooser_error {
            form = form.child(error.clone());
        }
        form.child(
            h_flex()
                .gap(zpx(8.))
                .child(
                    Button::new("explorer-browse")
                        .label("Browse...")
                        .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                )
                .child(
                    Button::new("explorer-open").label("Open Folder").on_click(
                        cx.listener(|this, _, window, cx| this.submit_folder(window, cx)),
                    ),
                )
                .child(
                    Button::new("explorer-cancel")
                        .ghost()
                        .label("Cancel")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Close))),
                ),
        )
    }
}
