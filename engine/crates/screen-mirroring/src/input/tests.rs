use super::testing::RecordingSink;
use super::*;
use phonelink_core::sync::MutexExt;

const A: u16 = 0x00;
const LEFT_OPTION: u16 = 0x3A;
const CAPS: u16 = 0x39;

/// A pipe on an open channel, unfocused, with a 1080 × 2400 phone.
fn pipe() -> (InputPipe, Arc<RecordingSink>) {
    let sink = Arc::new(RecordingSink::default());
    let mut p = InputPipe::new();
    p.set_sink(Some(sink.clone()));
    p.set_device_resolution(Some((1080, 2400)));
    p.focus(false, false);

    sink.frames.lock_or_panic().clear();
    *sink.state.lock_or_panic() = InputState::default();
    (p, sink)
}

fn frames(sink: &RecordingSink) -> usize {
    sink.frames.lock_or_panic().len()
}

fn mouse(sink: &RecordingSink) -> phonelink_nano::channel::input::Mouse {
    sink.state.lock_or_panic().mice[&0]
}

fn key_counter(sink: &RecordingSink, u: u8) -> u8 {
    sink.state.lock_or_panic().keyboards.get(&0).map_or(0, |k| k.counters[usize::from(u)])
}

#[tokio::test(start_paused = true)]
async fn an_opened_channel_takes_the_input() {
    let sink = Arc::new(RecordingSink::default());
    let mut p = InputPipe::new();
    p.set_device_resolution(Some((1080, 2400)));
    assert!(!p.focused());

    p.set_sink(Some(sink.clone()));
    assert!(p.focused(), "the window's input goes to the phone once the channel opens");

    p.mouse_move(0.5, 0.25);
    assert_eq!((mouse(&sink).x, mouse(&sink).y), (540, 600));
}

#[tokio::test(start_paused = true)]
async fn moves_are_gated_scaled_and_paced() {
    let (mut p, sink) = pipe();
    p.mouse_move(0.5, 0.5);
    assert_eq!(frames(&sink), 0, "nothing while unfocused");

    p.focus(true, false);
    p.mouse_move(0.5, 0.25);
    assert_eq!((mouse(&sink).x, mouse(&sink).y), (540, 600));

    p.mouse_move(0.6, 0.25);
    assert_eq!(frames(&sink), 1, "a move within 8 ms is dropped");

    tokio::time::advance(MOVE_INTERVAL).await;
    p.mouse_move(0.6, 0.25);
    assert_eq!(mouse(&sink).x, 648);

    tokio::time::advance(MOVE_INTERVAL).await;
    p.mouse_move(1.5, 0.2);
    assert_eq!(frames(&sink), 2, "a hover outside the video is not sent");

    p.set_device_resolution(Some((2400, 1080)));
    p.mouse_move(0.5, 0.5);
    assert_eq!((mouse(&sink).x, mouse(&sink).y), (1200, 540), "scaled to the current device resolution");
}

#[tokio::test(start_paused = true)]
async fn a_press_focuses_moves_first_and_carries_its_position() {
    let (mut p, sink) = pipe();
    p.mouse_button(MouseButton::Left, true, 0.1, 0.1);
    assert!(p.focused(), "a press takes the input focus");
    let f = sink.frames.lock_or_panic().clone();
    assert_eq!(f.len(), 2, "a move, then the press");
    assert_eq!((f[0].mice[&0].x, f[0].mice[&0].buttons[0]), (108, 0));
    assert_eq!((f[1].mice[&0].x, f[1].mice[&0].buttons[0]), (108, 1));

    // Dragged outside the video while held: sent as is.
    tokio::time::advance(MOVE_INTERVAL).await;
    p.mouse_move(-0.5, 0.5);
    assert_eq!(mouse(&sink).x, -540, "a held press is captured");

    p.mouse_button(MouseButton::Left, false, 1.2, 0.5);
    let m = mouse(&sink);
    assert_eq!((m.buttons[0], m.x, m.y), (2, 1296, 1200), "released where it is, outside included");

    // A second press after a move sends no extra move but updates the position.
    let before = frames(&sink);
    p.mouse_button(MouseButton::Right, true, 0.5, 0.5);
    assert_eq!(frames(&sink), before + 1);
    let m = mouse(&sink);
    assert_eq!((m.buttons[2], m.x, m.y), (1, 540, 1200), "right is button 2");

    // A press outside the video is not sent, nor its release.
    p.mouse_button(MouseButton::Right, false, 0.5, 0.5);
    let before = frames(&sink);
    p.mouse_button(MouseButton::Left, true, 1.5, 0.5);
    p.mouse_button(MouseButton::Left, false, 0.5, 0.5);
    assert_eq!(frames(&sink), before);
}

#[tokio::test(start_paused = true)]
async fn a_chord_sends_only_its_first_button() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.mouse_button(MouseButton::Left, true, 0.5, 0.5);
    p.mouse_button(MouseButton::Right, true, 0.5, 0.5);
    p.mouse_button(MouseButton::Left, false, 0.5, 0.5);
    let m = mouse(&sink);
    assert_eq!((m.buttons[0], m.buttons[2]), (1, 0), "right is not sent; left stays pressed");

    p.mouse_button(MouseButton::Right, false, 0.5, 0.5);
    assert_eq!(mouse(&sink).buttons[0], 2, "left is released with the last button up");

    // A button the phone has no counter for presses nothing but moves the pointer.
    let before = frames(&sink);
    p.mouse_button(MouseButton::Other, true, 0.25, 0.5);
    assert_eq!(frames(&sink), before + 1);
    assert_eq!((mouse(&sink).x, mouse(&sink).buttons), (270, [2, 0, 0, 0, 0, 0, 0, 0]));

    p.mouse_button(MouseButton::Left, true, 0.3, 0.5);
    assert_eq!(mouse(&sink).buttons[0], 2, "left while another button is held is a move");
}

#[tokio::test(start_paused = true)]
async fn the_wheel_carries_the_position_and_takes_the_focus() {
    let (mut p, sink) = pipe();
    p.mouse_wheel(0.5, 0.5, 0, -120);
    assert!(p.focused(), "a scroll takes the input focus");

    p.mouse_wheel(0.5, 0.25, 40, 0);
    let f = sink.frames.lock_or_panic().clone();
    assert_eq!(f.len(), 2, "one frame per scroll, unpaced");
    assert_eq!((f[0].mice[&0].x, f[0].mice[&0].y, f[0].mice[&0].wheel_y), (540, 1200, -120));
    let m = f[1].mice[&0];
    assert_eq!((m.y, m.wheel_x, m.wheel_y), (600, 40, -120), "totals, horizontal first");

    p.mouse_wheel(1.2, 0.5, 0, 120);
    assert_eq!(frames(&sink), 2, "a scroll outside the video is not sent");
}

#[tokio::test(start_paused = true)]
async fn focus_loss_releases_everything_in_separate_frames() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.mouse_button(MouseButton::Left, true, 0.1, 0.1);
    p.touch(7, TouchPhase::Down, 0.5, 0.5, 0.5, 90.0, 10.0, 12.0);
    p.touch(8, TouchPhase::Down, 0.5, 0.5, 1.0, 0.0, 10.0, 12.0);
    {
        let s = sink.state.lock_or_panic();
        assert_eq!((s.fingers[&0].touch_count, s.fingers[&1].touch_count), (1, 1));
        assert_eq!((s.fingers[&0].pressure, s.fingers[&0].orientation), (Some(127), Some(63)));
    }
    p.key(A, KeyAction::Down, 0);
    p.key(0x01, KeyAction::Down, 0);

    let before = frames(&sink);
    p.focus(false, false);
    assert_eq!(
        frames(&sink),
        before + 5,
        "button at (0, 0), two fingers, two keys; the move to (0, 0) changes nothing"
    );

    let s = sink.state.lock_or_panic().clone();
    let m = s.mice[&0];
    assert_eq!((m.buttons[0], m.x, m.y), (2, 0, 0), "released at (0, 0)");
    for f in s.fingers.values() {
        assert_eq!((f.state(), f.x, f.y, f.pressure, f.width), (FingerState::OutOfRange, 0, 0, None, None));
    }
    assert_eq!((s.keyboards[&0].counters[0x04], s.keyboards[&0].counters[0x16]), (2, 2));

    let frames_now = frames(&sink);
    p.mouse_move(0.2, 0.2);
    p.mouse_button(MouseButton::Left, false, 0.2, 0.2);
    assert_eq!(frames(&sink), frames_now, "nothing after focus loss");

    // The key-up still comes; its release changes nothing and sends nothing.
    p.key(A, KeyAction::Up, 0);
    assert_eq!(frames(&sink), frames_now);
    assert_eq!(key_counter(&sink, 0x04), 2);
}

#[tokio::test(start_paused = true)]
async fn a_pressed_button_released_after_a_focus_loss_sends_only_a_position() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.mouse_button(MouseButton::Left, true, 0.5, 0.5);
    p.focus(false, false);
    p.focus(true, false);

    let before = frames(&sink);
    p.mouse_button(MouseButton::Left, false, 0.25, 0.5);
    assert_eq!(frames(&sink), before + 1);
    let m = mouse(&sink);
    assert_eq!((m.buttons[0], m.x), (2, 270), "the button was released already");
}

#[tokio::test(start_paused = true)]
async fn keys_send_their_usage_and_repeats() {
    let (mut p, sink) = pipe();
    p.key(A, KeyAction::Down, 0);
    assert_eq!(key_counter(&sink, 0x04), 1, "keys need no focus, only the channel");

    p.key(A, KeyAction::Repeat, 0);
    assert_eq!(key_counter(&sink, 0x04), 3, "a repeat is a release and a press in one frame");
    assert_eq!(frames(&sink), 2);

    p.key(A, KeyAction::Up, 0);
    p.key(A, KeyAction::Up, 0);
    assert_eq!(frames(&sink), 3, "a key-up without its press sends nothing");

    p.key(0x30, KeyAction::Down, 0);
    p.key(0x37, KeyAction::Down, 0);
    assert_eq!(frames(&sink), 3, "Tab and Command send nothing");

    p.key(0x68, KeyAction::Down, 0);
    p.key(0x68, KeyAction::Repeat, 0);
    p.key(0x68, KeyAction::Up, 0);
    assert_eq!(key_counter(&sink, 0x90), 2, "Kana taps LANG1 once");

    p.nav_button(NavButton::Back);
    assert_eq!(key_counter(&sink, 0x6E), 2, "Back is F19");
}

#[tokio::test(start_paused = true)]
async fn a_key_after_option_was_released_releases_the_held_keys() {
    let (mut p, sink) = pipe();
    p.key(0x38, KeyAction::Down, 1 << 17);
    p.key(LEFT_OPTION, KeyAction::Down, FLAG_OPTION | 1 << 17);
    p.key(A, KeyAction::Down, FLAG_OPTION | 1 << 17);
    assert_eq!(key_counter(&sink, 0xE1), 1, "with Option held nothing is released");

    p.key(A, KeyAction::Up, FLAG_OPTION | 1 << 17);
    p.key(LEFT_OPTION, KeyAction::Up, 1 << 17);
    p.key(0x0B, KeyAction::Down, 1 << 17);
    assert_eq!(key_counter(&sink, 0xE1), 2, "Shift is released before B");
    assert_eq!(key_counter(&sink, 0x05), 1);

    p.key(0x0B, KeyAction::Up, 1 << 17);
    p.key(0x38, KeyAction::Repeat, 1 << 17);
    assert_eq!(key_counter(&sink, 0xE1), 3, "a repeat presses the held key again");
}

#[tokio::test(start_paused = true)]
async fn caps_lock_is_brought_to_the_macs_on_the_first_key_after_a_focus_change() {
    let (mut p, sink) = pipe();
    // The phone starts with Caps Lock off; the Mac has it on.
    p.focus(true, true);
    assert_eq!(key_counter(&sink, 0x39), 0, "nothing until a key is pressed");

    p.key(A, KeyAction::Down, FLAG_CAPS_LOCK);
    assert_eq!(key_counter(&sink, 0x39), 2, "a Caps Lock tap before the key");
    assert_eq!(key_counter(&sink, 0x04), 1);

    p.key(A, KeyAction::Up, FLAG_CAPS_LOCK);
    p.key(0x01, KeyAction::Down, FLAG_CAPS_LOCK);
    assert_eq!(key_counter(&sink, 0x39), 2, "only the first key re-syncs");

    // Toggled while focused: sent as it happens, without repeats.
    p.key(CAPS, KeyAction::Down, 0);
    p.key(CAPS, KeyAction::Repeat, 0);
    p.key(CAPS, KeyAction::Up, 0);
    assert_eq!(key_counter(&sink, 0x39), 4);

    // Focus lost with it off; turned on and off again elsewhere: no tap.
    p.focus(false, false);
    p.focus(true, false);
    p.key(A, KeyAction::Down, 0);
    assert_eq!(key_counter(&sink, 0x39), 4, "unchanged: no tap");

    // Turned on elsewhere, then the first key after the focus gain is Caps Lock turning it off again.
    p.focus(false, false);
    p.focus(true, true);
    p.key(CAPS, KeyAction::Down, 0);
    p.key(CAPS, KeyAction::Up, 0);
    assert_eq!(key_counter(&sink, 0x39), 4, "a toggle back to the phone's state is not sent");

    p.key(CAPS, KeyAction::Down, FLAG_CAPS_LOCK);
    p.key(CAPS, KeyAction::Up, FLAG_CAPS_LOCK);
    assert_eq!(key_counter(&sink, 0x39), 6, "the next toggle is");

    // Lost with it on, turned off elsewhere: the first key after the loss re-syncs.
    p.focus(false, true);
    p.key(A, KeyAction::Down, 0);
    assert_eq!(key_counter(&sink, 0x39), 8, "a focus loss arms the re-sync too");
}

#[tokio::test(start_paused = true)]
async fn a_nav_button_releases_and_refocuses() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.mouse_move(0.5, 0.5);
    p.key(A, KeyAction::Down, 0);

    p.nav_button(NavButton::Home);
    assert_eq!(key_counter(&sink, 0x6A), 2, "Home is F15");
    assert_eq!((mouse(&sink).x, key_counter(&sink, 0x04)), (0, 2), "then a focus loss");
    assert!(p.focused(), "and the focus again");
}

#[tokio::test(start_paused = true)]
async fn taking_input_anew_releases_what_is_held() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.mouse_button(MouseButton::Left, true, 0.5, 0.5);

    p.take_input(true, false);
    assert!(p.focused());
    assert_eq!((mouse(&sink).buttons[0], mouse(&sink).x), (2, 0), "released at (0, 0)");

    p.take_input(false, false);
    assert!(!p.focused());

    let before = frames(&sink);
    p.take_input(false, false);
    assert_eq!(frames(&sink), before, "nothing more to release");
}

#[tokio::test(start_paused = true)]
async fn touches_take_the_lowest_free_index() {
    let (mut p, sink) = pipe();
    p.touch(1, TouchPhase::Down, 0.5, 0.5, 0.5, 0.0, 4.0, 4.0);
    assert!(p.focused(), "a touch takes the input focus");
    let f = sink.state.lock_or_panic().fingers[&0];
    assert_eq!((f.hover_count, f.touch_count), (1, 1), "out of range to in contact");

    p.touch(2, TouchPhase::Down, 0.2, 0.2, 0.5, 0.0, 4.0, 4.0);
    p.touch(1, TouchPhase::Up, 0.5, 0.5, 0.5, 0.0, 4.0, 4.0);
    let f = sink.state.lock_or_panic().fingers[&0];
    assert_eq!((f.hover_count, f.touch_count), (2, 2), "and back out of range");

    p.touch(3, TouchPhase::Down, 0.3, 0.3, 0.5, 0.0, 4.0, 4.0);
    assert_eq!(sink.state.lock_or_panic().fingers[&0].touch_count, 3, "index 0 is free again");

    tokio::time::advance(MOVE_INTERVAL).await;
    p.touch(2, TouchPhase::Moved, -0.1, 0.2, 0.5, 0.0, 4.0, 4.0);
    assert_eq!(sink.state.lock_or_panic().fingers[&1].x, -108, "a contact is captured");
}

#[tokio::test(start_paused = true)]
async fn the_pen_is_finger_200() {
    let (mut p, sink) = pipe();
    p.focus(true, false);
    p.pen(0.5, 0.5, 0.0, 0.0, true, false);
    p.pen(0.5, 0.5, 1.0, 359.0, true, true);
    let f = sink.state.lock_or_panic().fingers[&PEN_FINGER];
    assert_eq!(
        (f.hover_count, f.touch_count, f.width, f.pressure, f.orientation),
        (1, 1, None, Some(255), Some(255)),
        "in range, then in contact"
    );

    p.pen(0.6, 0.5, 0.5, 0.0, true, true);
    assert_eq!(sink.state.lock_or_panic().fingers[&PEN_FINGER].x, 648, "a move");

    let before = frames(&sink);
    p.pen(0.7, 0.5, 0.5, 0.0, true, true);
    assert_eq!(frames(&sink), before, "a move within 8 ms is dropped");

    p.pen(0.5, 0.5, 0.0, 0.0, false, false);
    let f = sink.state.lock_or_panic().fingers[&PEN_FINGER];
    assert_eq!((f.hover_count, f.touch_count), (2, 2), "out of range straight from contact steps both");

    // Focus loss leaves the pen alone.
    p.pen(0.5, 0.5, 0.5, 0.0, true, true);
    p.focus(false, false);
    assert_eq!(sink.state.lock_or_panic().fingers[&PEN_FINGER].state(), FingerState::InContact);
}
