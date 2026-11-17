//! Unit tests of the gallery's selection.

use super::*;

const ORDER: [i64; 6] = [10, 11, 12, 13, 14, 15];

fn selected(selection: &Selection) -> Vec<i64> {
    selection.in_order(&ORDER)
}

#[test]
fn a_click_selects_one_photo() {
    let mut selection = Selection::default();
    selection.select(Some(12));
    assert_eq!(selected(&selection), [12]);
    assert_eq!(selection.focus(), Some(12));
    selection.select(None);
    assert!(selected(&selection).is_empty());
    assert_eq!(selection.focus(), None);
}

#[test]
fn ctrl_click_adds_and_removes() {
    let mut selection = Selection::default();
    selection.select(Some(10));
    selection.toggle(13);
    assert_eq!(selected(&selection), [10, 13]);
    assert_eq!(selection.focus(), Some(13));
    selection.toggle(13);
    assert_eq!(selected(&selection), [10]);
    assert_eq!(selection.focus(), None);
}

#[test]
fn shift_click_replaces_its_own_range() {
    let mut selection = Selection::default();
    selection.select(Some(11));
    selection.extend(&ORDER, 13);
    assert_eq!(selected(&selection), [11, 12, 13]);
    selection.extend(&ORDER, 10);
    assert_eq!(selected(&selection), [10, 11]);
    assert_eq!(selection.focus(), Some(10));
}

#[test]
fn shift_click_adds_to_a_ctrl_click_selection() {
    let mut selection = Selection::default();
    selection.select(Some(10));
    selection.toggle(13);
    selection.extend(&ORDER, 15);
    assert_eq!(selected(&selection), [10, 13, 14, 15]);
}

#[test]
fn shift_click_without_an_anchor_is_a_click() {
    let mut selection = Selection::default();
    selection.extend(&ORDER, 14);
    assert_eq!(selected(&selection), [14]);
}

#[test]
fn the_focus_moves_only_within_the_selection() {
    let mut selection = Selection::default();
    selection.select(Some(10));
    selection.toggle(12);
    selection.focus_on(10);
    assert_eq!(selection.focus(), Some(10));
    selection.focus_on(11);
    assert_eq!(selection.focus(), Some(10));
    assert_eq!(selected(&selection), [10, 12]);
}

#[test]
fn select_all_keeps_the_focus() {
    let mut selection = Selection::default();
    selection.select(Some(12));
    selection.select_all(&ORDER);
    assert_eq!(selected(&selection), ORDER);
    assert_eq!(selection.focus(), Some(12));
}

#[test]
fn arrows_move_the_focus_and_stop_at_the_ends() {
    let mut selection = Selection::default();
    selection.move_by(&ORDER, 1);
    assert_eq!(selected(&selection), [10]);
    selection.move_by(&ORDER, 3);
    assert_eq!(selected(&selection), [13]);
    selection.move_by(&ORDER, 3);
    assert_eq!(selected(&selection), [13]);
    selection.move_by(&ORDER, -1);
    assert_eq!(selection.focus(), Some(12));
    selection.move_by(&ORDER, -5);
    assert_eq!(selection.focus(), Some(12));
    selection.move_by(&[], 1);
    assert_eq!(selection.focus(), Some(12));
}

#[test]
fn a_context_menu_acts_on_the_selection_it_is_part_of() {
    let mut selection = Selection::default();
    selection.select(Some(14));
    selection.toggle(11);
    assert_eq!(selection.targets(&ORDER, 14), [11, 14]);
    assert_eq!(selection.targets(&ORDER, 12), [12]);
}

#[test]
fn photos_that_left_the_gallery_leave_the_selection() {
    let mut selection = Selection::default();
    selection.select(Some(11));
    selection.extend(&ORDER, 13);
    selection.retain(&[10, 12, 13].into_iter().collect());
    assert_eq!(selected(&selection), [12, 13]);
    assert_eq!(selection.focus(), Some(13));
    selection.retain(&[10].into_iter().collect());
    assert_eq!(selection.focus(), None);
    selection.extend(&ORDER, 12);
    assert_eq!(selected(&selection), [12]);
}
