use super::*;

impl GitHubView {
    pub(super) fn set_review_status(&mut self, key: (String, github::ReviewKind), status: String) {
        if !self.review_status.contains_key(&key) {
            while self.review_status.len() >= 64 {
                let Some(old) = self.review_status.keys().next().cloned() else {
                    break;
                };
                self.review_status.remove(&old);
                self.review_watches.remove(&old);
            }
        }
        self.review_status.insert(key, status);
    }

    pub(super) fn track_review(&mut self, watch: github::ReviewWatch) {
        let key = (
            format!("{}:{}", watch.account.key(), watch.pull.key()),
            watch.kind,
        );
        self.set_review_status(
            key.clone(),
            format!(
                "{} review requested. Waiting for a new or updated bot review.",
                watch.kind.label()
            ),
        );
        self.review_watches.insert(key, watch);
        self.review_poll_at = None;
    }

    pub(super) fn record_review_result(
        &mut self,
        key: (String, github::ReviewKind),
        result: Result<github::ReviewProgress, GitHubError>,
    ) {
        let kind = key.1.label();
        let status = match result {
            Ok(github::ReviewProgress::Waiting) => format!(
                "{kind}: waiting for a new or updated bot review. The request has been submitted."
            ),
            Ok(github::ReviewProgress::Completed { .. }) => {
                self.review_watches.remove(&key);
                format!(
                    "{kind}: a new or updated review is available. Refresh Conversation or Reviews to read it."
                )
            }
            Err(error) => format!(
                "{kind} review monitoring: {error} The submitted request will not be repeated."
            ),
        };
        self.set_review_status(key, status);
    }

    pub(super) fn poll_review(&mut self, cx: &mut Context<Self>) {
        if self.review_loading
            || self
                .review_poll_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(20))
        {
            return;
        }
        let Some(resource) = self.current_write_key() else {
            return;
        };
        let watches: Vec<_> = self
            .review_watches
            .iter()
            .filter(|((key, _), _)| key == &resource)
            .map(|(key, watch)| (key.clone(), watch.clone()))
            .collect();
        if watches.is_empty() {
            return;
        }
        let Some(ticket) = self.selection.ticket() else {
            return;
        };
        self.review_loading = true;
        self.review_poll_at = Some(Instant::now());
        let clients = self.clients.clone();
        let (abort, registration) = AbortHandle::new_pair();
        self.review_abort = Some(abort);
        let rx = Runtime::global(cx).spawn(Abortable::new(
            async move {
                let mut results = Vec::new();
                for (key, watch) in watches {
                    let client = clients.for_account(watch.account.clone());
                    results.push((key, watch.poll(&client).await));
                }
                results
            },
            registration,
        ));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(results)) = rx.await {
                this.update(cx, |this, cx| {
                    if !this.selection.accepts(&ticket) {
                        return;
                    }
                    this.review_loading = false;
                    this.review_abort = None;
                    for (key, result) in results {
                        this.record_review_result(key, result);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}
