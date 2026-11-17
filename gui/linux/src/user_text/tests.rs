use super::*;

#[test]
fn success_has_no_failure_text() {
    assert_eq!(outcome_failure(&RequestOutcome::Success), None);
    assert!(outcome_failure(&RequestOutcome::TimedOut).is_some());
}

#[test]
fn invalid_input_reads_as_a_sentence() {
    let text = feature_error(&FeatureApiError::InvalidInput { reason: "nothing to send".to_owned() });
    assert_eq!(text, "Nothing to send.");
}

#[test]
fn a_store_failure_points_at_the_keyring() {
    let error = EngineError::Setup { message: "store failure: Secret Service: no service".to_owned() };
    assert!(engine_error(&error).contains("keyring"));
}
