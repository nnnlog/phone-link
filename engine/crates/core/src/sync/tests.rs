use std::sync::Arc;

use super::*;

fn poisoned() -> Arc<Mutex<u32>> {
    let m = Arc::new(Mutex::new(1));
    let m2 = Arc::clone(&m);
    let _ = std::thread::spawn(move || {
        let mut g = m2.lock_or_panic();
        *g = 2;
        panic!("poison it");
    })
    .join();
    m
}

#[test]
fn recovering_takes_the_state_the_panic_left() {
    assert_eq!(*poisoned().lock_or_recover(), 2);
}

#[test]
#[should_panic(expected = "lock poisoned by an earlier panic")]
fn panicking_stops_on_a_poisoned_lock() {
    let lock = poisoned();
    let _guard = lock.lock_or_panic();
}
