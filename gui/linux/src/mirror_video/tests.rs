use super::*;

#[test]
fn the_picture_keeps_its_aspect_centered_in_the_item() {
    assert_eq!(aspect_fit((1080, 2400), (540.0, 1200.0)), Some([0, 0, 540, 1200]), "the same aspect fills it");
    assert_eq!(aspect_fit((1080, 2400), (400.0, 780.0)), Some([25, 0, 351, 780]), "bars at the sides");
    assert_eq!(aspect_fit((1920, 1080), (400.0, 800.0)), Some([0, 288, 400, 225]), "bars above and below");
}

#[test]
fn nothing_is_drawn_without_an_area() {
    assert_eq!(aspect_fit((0, 2400), (400.0, 780.0)), None);
    assert_eq!(aspect_fit((1080, 2400), (0.0, 780.0)), None);
    assert_eq!(aspect_fit((1080, 2400), (400.0, 0.0)), None);
}
