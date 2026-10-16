use super::*;

fn context() -> Context {
    Context {
        account: "synthetic-account".into(),
        model: "fixture-model".into(),
        review: None,
    }
}

fn run(saved: bool) -> Execution {
    Execution::new(context(), "Synthetic prompt 界".into(), saved).unwrap()
}

#[test]
fn saved_execution_requires_a_known_pending_id_and_claims_send_only_once() {
    let mut run = run(true);
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
    assert!(run.record_created(run.key(), "pending-id".into()));
    let send = run.begin_send().unwrap();
    assert_eq!(send.prompt, "Synthetic prompt 界");
    assert_eq!(send.context, context());
    assert_eq!(run.phase(), Phase::Sending);
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
}

#[test]
fn saved_prompt_and_answer_share_a_storage_budget_after_prompt_ownership_moves() {
    let mut run = Execution::new(context(), "x".repeat(MAX_PROMPT), true).unwrap();
    run.record_created(run.key(), "known-id".into());
    let request = run.begin_send().unwrap();
    assert_eq!(request.prompt.len(), MAX_PROMPT);
    assert_eq!(run.output_limit(), MAX_SAVED_COMBINED - MAX_PROMPT);
    assert!(run.delta(run.key(), "one", &"x".repeat(run.output_limit())));
    assert!(!run.delta(run.key(), "one", "x"));
    assert_eq!(run.failure(), Some(Failure::OutputLimit));
    assert_eq!(run.output().len(), MAX_SAVED_COMBINED - MAX_PROMPT);
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
}

#[test]
fn unknown_creation_and_late_acknowledgement_never_start_a_model_turn() {
    let mut run = run(true);
    assert!(run.record_unknown(run.key()));
    assert!(run.record_created(run.key(), "late-id".into()));
    assert_eq!(run.result_id(), Some("late-id"));
    assert_eq!(run.phase(), Phase::Failed);
    assert_eq!(run.failure(), Some(Failure::RecordUnknown));
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
}

#[test]
fn streamed_output_requires_final_message_and_idle_before_success() {
    let mut run = run(false);
    run.begin_send().unwrap();
    assert!(run.delta(run.key(), "message", "Hello "));
    assert!(run.delta(run.key(), "message", "界"));
    assert_eq!(run.output(), "Hello 界");
    assert!(run.final_message(run.key(), "message", "Hello 界!"));
    assert_eq!(run.phase(), Phase::Streaming);
    assert!(run.idle(run.key()));
    assert_eq!(run.phase(), Phase::Completed);
    assert_eq!(run.output(), "Hello 界!");
    assert!(!run.delta(run.key(), "message", "late"));
}

#[test]
fn multiple_assistant_messages_do_not_duplicate_delta_and_final_content() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.delta(run.key(), "one", "First");
    run.final_message(run.key(), "one", "First answer");
    run.delta(run.key(), "two", "Second");
    run.final_message(run.key(), "two", "Second 界");
    run.idle(run.key());
    assert_eq!(run.output(), "First answer\n\nSecond 界");
    assert_eq!(run.phase(), Phase::Completed);
}

#[test]
fn idle_without_a_final_answer_is_incomplete_and_keeps_partial_text() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.delta(run.key(), "one", "partial");
    run.idle(run.key());
    assert_eq!(run.phase(), Phase::Failed);
    assert_eq!(run.failure(), Some(Failure::IncompleteStream));
    assert_eq!(run.output(), "partial");
}

#[test]
fn cancellation_blocks_success_and_keeps_partial_output() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.delta(run.key(), "one", "partial");
    assert!(run.cancel());
    assert_eq!(run.phase(), Phase::Cancelling);
    assert!(!run.final_message(run.key(), "one", "late success"));
    assert!(run.abort_confirmed(run.key()));
    assert_eq!(run.phase(), Phase::Cancelled);
    assert_eq!(run.output(), "partial");
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
}

#[test]
fn cancellation_before_record_creation_can_reconcile_its_late_id() {
    let mut run = run(true);
    run.cancel();
    assert!(run.record_created(run.key(), "cancelled-record".into()));
    assert_eq!(run.phase(), Phase::Cancelled);
    assert_eq!(run.result_id(), Some("cancelled-record"));
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
}

#[test]
fn lost_connection_during_abort_reports_uncertainty() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.cancel();
    assert!(run.failed(run.key(), Failure::ServiceUnavailable));
    assert_eq!(run.failure(), Some(Failure::CancellationUnknown));
    assert_eq!(run.phase(), Phase::Failed);
}

#[test]
fn persistence_retry_keeps_the_captured_answer_and_cannot_resend() {
    let mut run = run(true);
    run.record_created(run.key(), "known-id".into());
    run.begin_send().unwrap();
    run.final_message(run.key(), "one", "Captured answer");
    run.idle(run.key());
    assert_eq!(run.phase(), Phase::AwaitingPersistence);
    assert!(!run.persisted(run.key(), "different-id"));
    assert!(run.persistence_failed(run.key()));
    assert_eq!(run.output(), "Captured answer");
    assert!(run.retry_persistence());
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
    assert!(run.persisted(run.key(), "known-id"));
    assert_eq!(run.phase(), Phase::Completed);
    assert_eq!(run.result_id(), Some("known-id"));
}

#[test]
fn late_events_from_an_old_context_cannot_change_a_new_run() {
    let mut old = run(false);
    old.begin_send().unwrap();
    let old_key = old.key();
    let mut new_context = context();
    new_context.account = "replacement-account".into();
    let mut new = Execution::new(new_context.clone(), "new prompt".into(), false).unwrap();
    new.begin_send().unwrap();
    assert!(!new.delta(old_key, "one", "private old answer"));
    assert!(!new.final_message(old_key, "one", "old final"));
    assert!(!new.idle(old_key));
    assert!(!new.failed(old_key, Failure::ServiceUnavailable));
    assert_eq!(new.output(), "");
    assert_eq!(new.context(), &new_context);
    assert_eq!(new.phase(), Phase::Sending);
}

#[test]
fn uncertain_send_never_authorizes_a_replay() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.failed(run.key(), Failure::SendUnknown);
    assert!(matches!(run.begin_send(), Err(Failure::NotPrepared)));
    assert_eq!(run.failure(), Some(Failure::SendUnknown));
}

#[test]
fn output_and_event_limits_stop_instead_of_silently_truncating() {
    let mut overflow = run(false);
    overflow.begin_send().unwrap();
    assert!(!overflow.delta(overflow.key(), "one", &"x".repeat(MAX_OUTPUT + 1)));
    assert_eq!(overflow.failure(), Some(Failure::OutputLimit));
    assert!(overflow.output().is_empty());
    let mut events = run(false);
    events.begin_send().unwrap();
    for _ in 0..MAX_EVENTS {
        assert!(events.delta(events.key(), "one", ""));
    }
    assert!(!events.delta(events.key(), "one", ""));
    assert_eq!(events.failure(), Some(Failure::EventLimit));
}

#[test]
fn incomplete_message_switches_and_old_message_replays_are_rejected() {
    let mut run = run(false);
    run.begin_send().unwrap();
    run.delta(run.key(), "one", "partial");
    assert!(!run.delta(run.key(), "two", "replacement"));
    assert_eq!(run.failure(), Some(Failure::IncompleteStream));
    let mut run = self::run(false);
    run.begin_send().unwrap();
    run.final_message(run.key(), "one", "first");
    run.final_message(run.key(), "two", "second");
    assert!(!run.final_message(run.key(), "one", "late first"));
    assert_eq!(run.failure(), Some(Failure::IncompleteStream));
}

#[test]
fn invalid_contexts_and_unpersisted_reviews_are_rejected_before_execution() {
    for account in [
        "-bad-account",
        "bad--account",
        "bad-account-",
        "bad/account",
    ] {
        let mut invalid = context();
        invalid.account = account.into();
        assert!(
            matches!(
                Execution::new(invalid, "prompt".into(), false),
                Err(Failure::InvalidContext)
            ),
            "invalid account accepted"
        );
    }
    let mut review = context();
    review.review = Some(ReviewContext {
        repository: "owner/repository".into(),
        pull_request: 12,
        head: "a".repeat(40),
    });
    assert!(matches!(
        Execution::new(review.clone(), "prompt".into(), false),
        Err(Failure::InvalidContext)
    ));
    let execution = Execution::new(review.clone(), "prompt".into(), true).unwrap();
    assert_eq!(execution.context(), &review);
    review.review.as_mut().unwrap().repository = "owner/repo/../other".into();
    assert!(matches!(
        Execution::new(review, "prompt".into(), true),
        Err(Failure::InvalidContext)
    ));
    assert!(matches!(
        Execution::new(context(), " ".into(), false),
        Err(Failure::InvalidPrompt)
    ));
}

#[test]
fn review_freshness_compares_captured_repository_pr_and_head_without_rebinding() {
    let mut context = context();
    let captured = ReviewContext {
        repository: "Owner/Repo".into(),
        pull_request: 12,
        head: "a".repeat(40),
    };
    context.review = Some(captured.clone());
    let run = Execution::new(context.clone(), "review".into(), true).unwrap();
    assert_eq!(run.review_freshness(None), ReviewFreshness::Unknown);
    assert_eq!(
        run.review_freshness(Some(&captured)),
        ReviewFreshness::Current
    );
    let mut live = captured.clone();
    live.head = "b".repeat(40);
    assert_eq!(run.review_freshness(Some(&live)), ReviewFreshness::Stale);
    live = captured.clone();
    live.repository = "other/repository".into();
    assert_eq!(run.review_freshness(Some(&live)), ReviewFreshness::Stale);
    live = captured.clone();
    live.pull_request = 13;
    assert_eq!(run.review_freshness(Some(&live)), ReviewFreshness::Stale);
    live.head.clear();
    assert_eq!(run.review_freshness(Some(&live)), ReviewFreshness::Unknown);
    assert_eq!(run.context(), &context);
    assert_eq!(
        self::run(false).review_freshness(Some(&captured)),
        ReviewFreshness::NotAReview
    );
}
