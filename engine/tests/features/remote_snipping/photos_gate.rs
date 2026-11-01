//! The photos feature's own new-photo notification is not posted for a phone remote snipping covers.

use crate::support::photos_world as support;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use phonelink_engine::features::NewPhotoGate;
use phonelink_engine::types::DcgClient;

use support::{it, seed, world};

struct FixedGate(AtomicBool);

impl NewPhotoGate for FixedGate {
    fn covered(&self, _phone: &DcgClient) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[tokio::test(start_paused = true)]
async fn a_covered_phone_gets_no_photos_notification() {
    let world = world();
    let gate = Arc::new(FixedGate(AtomicBool::new(true)));
    world.photos.set_new_photo_gate(gate.clone()).unwrap();
    assert!(world.photos.set_new_photo_gate(gate.clone()).is_err(), "the gate is set once");

    let now = world.now_ms();
    seed(&world, &[it(1, now - 1_000)]).await;
    assert!(world.w.notifier.posted().is_empty());

    gate.0.store(false, Ordering::SeqCst);
    seed(&world, &[it(2, now - 500)]).await;
    assert_eq!(world.w.notifier.posted().len(), 1);
}
