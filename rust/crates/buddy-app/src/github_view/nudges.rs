use super::*;
use buddy_core::github::nudge::{NudgeClient, NudgeError};

impl GitHubView {
    pub(super) fn lookup_nudge(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.write_busy || self.must_reload {
            return;
        }
        let Some(pull) = self.data.as_ref().and_then(|data| data.pull.clone()) else {
            return;
        };
        let Some(ticket) = self.selection.ticket() else {
            return;
        };
        if self
            .current_write_key()
            .is_some_and(|key| self.unknown_nudges.contains(&key))
        {
            return;
        }
        self.cancel_read();
        self.pending = None;
        self.loading = true;
        let email = self.nudge_email.read(cx).value().to_string();
        let github = self.clients.for_account(ticket.account.clone());
        let captured = ticket.clone();
        let (abort, registration) = AbortHandle::new_pair();
        self.read_abort = Some(abort);
        let rx = Runtime::global(cx).spawn(Abortable::new(
            async move {
                let email = if email.trim().is_empty() {
                    github
                        .user_profile(&pull.author, false)
                        .await
                        .map_err(|_| NudgeError::Email)?
                        .email
                        .ok_or(NudgeError::Email)?
                } else {
                    email
                };
                NudgeClient::from_environment()?
                    .prepare(captured, pull.target, pull.title, &email)
                    .await
            },
            registration,
        ));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(result)) = rx.await {
                this.update(cx, |this, cx| {
                    if !this.selection.accepts(&ticket) {
                        return;
                    }
                    this.loading = false;
                    this.read_abort = None;
                    match result {
                        Ok(pending) => this.pending_nudge = Some(pending),
                        Err(error) => this.status = Some(error.to_string()),
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    fn send_nudge(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_nudge.take() else {
            return;
        };
        let account = pending.account().clone();
        let key = format!("{}:{}", account.key(), pending.pull().key());
        let confirmed = match pending.confirm(&self.selection) {
            Ok(confirmed) => confirmed,
            Err(error) => {
                self.status = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        self.write_busy = true;
        let rx = Runtime::global(cx).spawn(confirmed.send());
        cx.spawn(async move |this, cx| {
            let result = rx.await.unwrap_or(Err(NudgeError::UnknownSend));
            this.update(cx, |this, cx| {
                this.write_busy = false;
                if result == Err(NudgeError::UnknownSend) {
                    this.unknown_nudges.insert(key);
                }
                if this.account.as_ref() == Some(&account) {
                    this.status = Some(match result {
                        Ok(()) => "Slack nudge sent to the confirmed recipient.".into(),
                        Err(error) => error.to_string(),
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn nudge_confirmation(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut content = v_flex().gap_2();
        if let Some(pending) = &self.pending_nudge {
            content = content
                .child(format!(
                    "Send to {} ({}) in {}?",
                    pending.recipient, pending.email, pending.workspace
                ))
                .child(format!(
                    "GitHub account: {} / {}",
                    pending.account().organization(),
                    pending.account().username()
                ))
                .child(div().whitespace_normal().child(pending.message()))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("github-send-nudge")
                                .label("Send Slack nudge")
                                .disabled(self.write_busy)
                                .on_click(cx.listener(|this, _, _, cx| this.send_nudge(cx))),
                        )
                        .child(Button::new("github-cancel-nudge").label("Cancel").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.pending_nudge = None;
                                cx.notify();
                            }),
                        )),
                );
        }
        if let Some(key) = self.current_write_key()
            && self.unknown_nudges.contains(&key)
        {
            content=content.child("The previous Slack send has an unknown result. Check the conversation before sending another nudge.")
                .child(Button::new("github-nudge-checked").label("I've checked Slack; allow another nudge").on_click(cx.listener(move|this,_,_,cx|{this.unknown_nudges.remove(&key);cx.notify();})));
        }
        content
    }
}
