use super::*;

#[test]
fn scope_family_pairs() {
    assert_eq!(ScopeFamily::Rps.authorization_portal(), "MSM");
    assert_eq!(ScopeFamily::FirstPartyPortal.authorization_portal(), "FirstPartyAppsRepo");
    assert_eq!(ScopeFamily::FirstPartyPortal.dcg_scope(Environment::Prod), "https://dcg.microsoft.com/DCG.ReadWrite");
}
