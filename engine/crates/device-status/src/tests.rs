use super::*;
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::Environment;

/// Each control compares record changes with its own baseline, and its timer sees a change already in the
/// record whose event is still queued.
#[test]
fn each_control_compares_with_its_own_baseline() {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    let mut record = PhoneRecord::new(&phone);
    let mut ps = PhoneState::default();
    ps.ringer.pending = Some((1, RingerPending::Vibrate));
    ps.ringer.seen = Some(Seen::of(&record));

    // The phone's ringer change is written; DND is activated before its event is processed.
    record.apply_device_status_item(&ValueSet::new().with("ringerMode", 1i32));
    ps.dnd.pending = Some((2, DndPending::TurningOn));
    ps.dnd.seen = Some(Seen::of(&record));
    assert!(ps.ringer.unprocessed_change(&record, Seen::ringer_reset), "the ringer has not compared it");
    assert!(!ps.dnd.unprocessed_change(&record, Seen::dnd_reset), "the DND activation saw it");

    // The mirror case: a DND change written after the DND activation is the DND control's.
    record.apply_device_status_item(&ValueSet::new().with("doNotDisturbStatus", 2i32));
    assert!(ps.dnd.unprocessed_change(&record, Seen::dnd_reset));

    // A control that has compared nothing has nothing unprocessed.
    assert!(!ControlState::<DndPending>::default().unprocessed_change(&record, Seen::dnd_reset));
}
