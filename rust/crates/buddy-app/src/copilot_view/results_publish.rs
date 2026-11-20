use super::*;
use crate::settings::Settings;
use buddy_core::copilot_results::publish::Publication;
use buddy_core::github::GitHubClients;
use std::collections::BTreeMap;

pub(super) struct State {
    pub pending: Option<Publication>,
    attempts: BTreeMap<String, &'static str>,
    clients: GitHubClients,
}
impl Default for State {
    fn default() -> Self {
        Self {
            pending: None,
            attempts: BTreeMap::new(),
            clients: GitHubClients::configured(),
        }
    }
}
impl ResultsView {
    fn publication_candidate(&self, cx: &Context<Self>) -> Result<Publication, &'static str> {
        let result = self
            .selected
            .as_ref()
            .ok_or("Choose a saved review first.")?;
        let accounts = if cx.has_global::<Settings>() {
            Settings::global(cx).config.github.accounts.as_slice()
        } else {
            &[]
        };
        Publication::from_result(result, accounts)
    }
    fn prepare_publication(&mut self, cx: &mut Context<Self>) {
        self.publication.pending = None;
        self.delete_confirm = false;
        self.notice = None;
        if self.busy() {
            return;
        }
        match self.publication_candidate(cx) {
            Ok(candidate) if !self.publication.attempts.contains_key(&candidate.result_id)
                && self.publication.attempts.len() < 128 => self.publication.pending = Some(candidate),
            Ok(_) => self.notice = Some("This session already attempted publication, or reached its publication limit. No comment was sent.".into()),
            Err(error) => self.notice = Some(error.into()),
        }
        cx.notify();
    }
    fn confirm_publication(&mut self, cx: &mut Context<Self>) {
        let Some(prepared) = self.publication.pending.take() else {
            return;
        };
        if self.busy() || !cx.has_global::<Runtime>() {
            return;
        }
        if self.publication_candidate(cx).as_ref() != Ok(&prepared)
            || self.publication.attempts.contains_key(&prepared.result_id)
        {
            self.notice =
                Some("The saved review or account changed. Prepare publication again.".into());
            cx.notify();
            return;
        }
        let Ok((selection, pending)) = prepared.pending() else {
            return;
        };
        let Ok(confirmed) = pending.confirm(&selection) else {
            return;
        };
        let client = self.publication.clients.for_account(prepared.account);
        let id = prepared.result_id;
        // Record intent before spawning. Neither refresh nor a dropped worker can resend it.
        self.publication
            .attempts
            .insert(id.clone(), "Publication pending. No automatic resend.");
        self.write_loading = true;
        let rx = Runtime::global(cx).spawn(async move { client.execute(confirmed).await });
        cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |this, cx| {
                this.write_loading = false;
                let status = if matches!(result, Ok(Ok(_))) {
                    "Review comment published. This session will not publish this result again."
                } else {
                    "Publication unconfirmed. Check the PR on GitHub before any further action. This session will not resend it."
                };
                this.publication.attempts.insert(id, status);
                this.flush_open(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
    pub(super) fn publication_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut controls = v_flex().w_full().min_w_0().gap_2();
        let Some(result) = &self.selected else {
            return controls.into_any_element();
        };
        if let Some(status) = self.publication.attempts.get(&result.id) {
            return controls.child(*status).into_any_element();
        }
        if result.category.as_deref() != Some("pr-review") || result.status != Status::Completed {
            return controls.into_any_element();
        }
        match self.publication_candidate(cx) {
            Ok(_) => {
                controls = controls.child(
                    Button::new("copilot-publish-review")
                        .label("Publish to PR…")
                        .disabled(self.busy() || self.publication.attempts.len() >= 128)
                        .on_click(cx.listener(|this, _, _, cx| this.prepare_publication(cx))),
                )
            }
            Err(error) => return controls.child(error).into_any_element(),
        }
        if let Some(prepared) = &self.publication.pending {
            controls = controls
                .child(format!("Post saved review to {} #{} as {}?", prepared.pull.repository.full_name(), prepared.pull.number, prepared.account.username()))
                .child(format!("Configured account scope: {}. Captured head: {}. This is the saved answer; current PR freshness has not been checked.", prepared.account.key(), prepared.captured_head.as_deref().unwrap_or("not recorded")))
                .child("Publishes the displayed saved answer with Buddy/model attribution as an ordinary comment. No assistant runs and no formal review is submitted. Publication protection lasts for this app session only.")
                .child(h_flex().gap_2()
                    .child(Button::new("copilot-confirm-publish").label("Confirm publication").disabled(self.busy())
                        .on_click(cx.listener(|this, _, _, cx| this.confirm_publication(cx))))
                    .child(Button::new("copilot-cancel-publish").label("Cancel publication")
                        .on_click(cx.listener(|this, _, _, cx| { this.publication.pending = None; cx.notify(); }))));
        }
        controls.into_any_element()
    }
}

#[cfg(test)]
#[path = "results_publish_tests.rs"]
mod tests;
