//! MSA tokens: the per-identity [`MsaClient`] over
//! the shared device-bound broker — a silent token when a session is seeded, the interactive web
//! sign-in when none is, the RPS scope strings, and the error mapping.

use crate::support::auth_world;

use auth_world::harness::{Harness, SEED_PUID, da_property_json};
use phonelink_engine::auth::msa::MsaError;
use phonelink_engine::edge::{SecureStore, WebSignInError, WebSignInKind};
use phonelink_engine::types::Environment;

#[tokio::test]
async fn silent_token_when_a_session_is_seeded() {
    let h = Harness::new().await;
    let service = h.signed_in().await; // seeds a device-bound session

    // One silent exchange per scope, each issuing its own token; the second call is cached.
    assert_eq!(service.msa().access_token(Environment::Prod).await.unwrap(), "msa-at-1");
    assert_eq!(service.msa().strong_auth_token(Environment::Prod).await.unwrap(), "msa-at-2");
    assert_eq!(service.msa().access_token(Environment::Prod).await.unwrap(), "msa-at-1");

    assert_eq!(
        h.msa.scopes(),
        [service.msa().scope(Environment::Prod).unwrap(), service.msa().strong_auth_scope(Environment::Prod).unwrap()]
    );
    assert!(
        h.msa
            .exchanges()
            .iter()
            .all(|e| !e.is_interactive() && e.client_id == service.config().identity.profile.msa_client_id)
    );

    assert!(h.web.kinds().is_empty(), "no web view");
    assert!(!service.msa().needs_attention());
    assert_eq!(service.msa().account_puid().as_deref(), Some(SEED_PUID));
}

#[tokio::test]
async fn interactive_sign_in_runs_the_web_view() {
    let h = Harness::new().await;

    // No seeded session: sign_in drives the fake web view, which returns a DAProperty.
    let service = h.service();
    service.sign_in().await.unwrap();

    assert_eq!(service.msa().account_puid().as_deref(), Some(SEED_PUID));
    assert_eq!(h.web.kinds(), [WebSignInKind::Login]);

    // The next exchange spends the page's inline flow token.
    assert_eq!(service.msa().access_token(Environment::Prod).await.unwrap(), "msa-at-1");
    let exchanges = h.msa.exchanges();
    assert_eq!(exchanges.len(), 1);
    assert_eq!((exchanges[0].inline_ux.as_str(), exchanges[0].inline_ft.as_deref()), ("TokenBroker", Some("ft-1")));
}

#[tokio::test]
async fn a_different_account_switches() {
    let h = Harness::new().await;
    let first = h.signed_in().await; // SEED_PUID
    first.ensure_provisioned().await.unwrap();

    // A new sign-in as another account: the web view returns a different PUID.
    h.web.queue(Ok(da_property_json("0003OTHERACCOUNT")));
    // Clear the seeded session so sign_in runs the web view again.
    h.store.delete("auth/live/user".into()).unwrap();

    let service = h.service();
    service.sign_in().await.unwrap();
    assert_eq!(service.msa().account_puid().as_deref(), Some("0003OTHERACCOUNT"));
}

#[tokio::test]
async fn a_declined_web_sign_in_is_interaction_required() {
    let h = Harness::new().await;
    h.web.queue(Err(WebSignInError::Declined));
    let service = h.service();
    let err = service.sign_in().await.unwrap_err();
    assert!(matches!(err, phonelink_engine::auth::AuthError::Msa(MsaError::Declined)), "{err:?}");
}

#[tokio::test]
async fn rps_scope_strings() {
    let h = Harness::new().await;
    let service = h.service();
    assert_eq!(service.msa().scope(Environment::Prod).unwrap(), "service::msatoken.dcg.microsoft.com::MBI_SSL");
    assert_eq!(service.msa().scope(Environment::Beta).unwrap(), "service::msatoken.dcg-beta.microsoft.com::MBI_SSL");
    assert_eq!(
        service.msa().strong_auth_scope(Environment::Prod).unwrap(),
        "service::msatoken.dcg.microsoft.com::SAPI"
    );
}

#[tokio::test]
async fn a_failed_web_view_is_interaction_required() {
    let h = Harness::new().await;
    h.web.queue(Err(WebSignInError::Failed { reason: "no view".into() }));
    let service = h.service();
    let err = service.sign_in().await.unwrap_err();
    assert!(matches!(err, phonelink_engine::auth::AuthError::UserInteractionRequired(_)), "{err:?}");
}
