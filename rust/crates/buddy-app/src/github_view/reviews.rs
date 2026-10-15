use super::*;

impl GitHubView {
    pub(super) fn poll_review(&mut self, cx: &mut Context<Self>) {
        if self.review_loading
            || self
                .review_poll_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(20))
        {
            return;
        }
        let Some(key) = self.current_write_key() else {
            return;
        };
        let Some(watch) = self.review_watches.get(&key).cloned() else {
            return;
        };
        let Some(ticket) = self.selection.ticket() else {
            return;
        };
        self.review_loading = true;
        self.review_poll_at = Some(Instant::now());
        let client = self.clients.for_account(watch.account.clone());
        let (abort, registration) = AbortHandle::new_pair();
        self.review_abort = Some(abort);
        let rx = Runtime::global(cx).spawn(Abortable::new(
            async move { watch.poll(&client).await },
            registration,
        ));
        cx.spawn(async move |this,cx| {if let Ok(Ok(result))=rx.await {this.update(cx,|this,cx| {
            if !this.selection.accepts(&ticket) {return;}this.review_loading=false;this.review_abort=None;
            let status=match result {
                Ok(github::ReviewProgress::Waiting)=>"Waiting for a new or updated bot review. The request has been submitted.".into(),
                Ok(github::ReviewProgress::Completed {..})=>{this.review_watches.remove(&key);"A new or updated bot review is available. Refresh Conversation or Reviews to read it.".into()},
                Err(error)=>format!("Review monitoring: {error} The submitted request will not be repeated."),
            };
            this.review_status.insert(key,status);cx.notify();
        }).ok();}}).detach();
    }
}
