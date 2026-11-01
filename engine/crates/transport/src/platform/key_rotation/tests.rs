use super::*;
use std::time::Duration;

#[test]
fn ceil_seconds() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);

    assert_eq!(ceil_seconds_until(now + Duration::from_secs(10), now), 10);
    assert_eq!(ceil_seconds_until(now + Duration::from_millis(10_001), now), 11);
    assert_eq!(ceil_seconds_until(now + Duration::from_nanos(1), now), 1);
    assert_eq!(ceil_seconds_until(now, now), 0);
    assert_eq!(ceil_seconds_until(now - Duration::from_millis(5_300), now), -5);
    assert_eq!(ceil_seconds_until(now - Duration::from_secs(5), now), -5);
}

#[test]
fn reply_key_order() {
    assert_eq!(
        success(Kvp::new().with(TIME_TO_EXPIRE_KEY, "17")).to_body(),
        br#"{"TimeToExpireInSeconds":"17","CryptoKeyRotationResult":"0"}"#
    );
    assert_eq!(
        relationship_not_found().to_body(),
        br#"{"CryptoKeyRotationResult":"1","ResultDetails":"crypto trust relationship not found"}"#
    );
}
