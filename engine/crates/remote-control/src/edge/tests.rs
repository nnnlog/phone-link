use super::standins::RecordingPowerControl;
use super::*;

#[tokio::test]
async fn perform_maps_the_edge_outcome() {
    let power = RecordingPowerControl::new();
    assert!(perform(power.clone(), PowerAction::LockScreen).await);

    power.set_failing(true);
    assert!(!perform(power.clone(), PowerAction::Restart).await);
    assert_eq!(power.actions(), [PowerAction::LockScreen, PowerAction::Restart]);

    for action in [PowerAction::LockScreen, PowerAction::Sleep, PowerAction::ShutDown, PowerAction::Restart] {
        assert!(!perform(Arc::new(RefusingPowerControl), action).await, "{action:?}");
    }
}

struct Panicking;
impl PcPowerControl for Panicking {
    fn lock_screen(&self) -> Result<(), EdgeError> {
        panic!("edge bug")
    }
    fn sleep(&self) -> Result<(), EdgeError> {
        Ok(())
    }
    fn shut_down(&self) -> Result<(), EdgeError> {
        Ok(())
    }
    fn restart(&self) -> Result<(), EdgeError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_panicking_edge_is_a_failure() {
    assert!(!perform(Arc::new(Panicking), PowerAction::LockScreen).await);
}
