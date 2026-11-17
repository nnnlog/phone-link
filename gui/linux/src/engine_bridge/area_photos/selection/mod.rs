//! The gallery's selection, as a file manager reads clicks: a click selects one photo, Ctrl-click
//! adds or removes one, Shift-click selects a range from the last photo clicked, Ctrl+A selects every
//! photo, and the arrow keys move the focus. The focus is the photo the arrow keys move from and the
//! viewer shows: the last one clicked or moved to, always one of the selection.

use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub(super) struct Selection {
    selected: HashSet<i64>,
    focus: Option<i64>,
    /// Where a Shift-click range starts.
    anchor: Option<i64>,
    /// The selection a Shift-click range adds to: the one when the anchor was set.
    range_base: HashSet<i64>,
}

impl Selection {
    pub(super) fn focus(&self) -> Option<i64> {
        self.focus
    }

    pub(super) fn contains(&self, id: i64) -> bool {
        self.selected.contains(&id)
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.selected.iter().copied()
    }

    /// The selected photos in gallery order.
    fn in_order(&self, order: &[i64]) -> Vec<i64> {
        order.iter().copied().filter(|id| self.selected.contains(id)).collect()
    }

    /// What a context menu acts on: the selection when the clicked photo is part of it, otherwise the
    /// clicked photo alone.
    pub(super) fn targets(&self, order: &[i64], clicked: i64) -> Vec<i64> {
        if self.selected.contains(&clicked) { self.in_order(order) } else { vec![clicked] }
    }

    /// A click: the photo alone is selected (`None`: nothing is), and a Shift-click range starts there.
    pub(super) fn select(&mut self, id: Option<i64>) {
        self.selected = id.into_iter().collect();
        self.anchor = id;
        self.range_base.clear();
        self.focus = id;
    }

    /// A Ctrl-click: the photo joins the selection, or leaves it. Either way a Shift-click range starts
    /// there and adds to the selection as it now stands.
    pub(super) fn toggle(&mut self, id: i64) {
        self.anchor = Some(id);
        if self.selected.remove(&id) {
            if self.focus == Some(id) {
                self.focus = None;
            }
        } else {
            self.selected.insert(id);
            self.focus = Some(id);
        }
        self.range_base = self.selected.clone();
    }

    /// A Shift-click: the photos from the anchor to this one, in gallery order, join the selection as
    /// it stood when the anchor was set, so a second Shift-click replaces the first one's range. Without
    /// an anchor it is a click.
    pub(super) fn extend(&mut self, order: &[i64], id: i64) {
        let from = self.anchor.and_then(|anchor| order.iter().position(|item| *item == anchor));
        let to = order.iter().position(|item| *item == id);
        let (Some(from), Some(to)) = (from, to) else {
            self.select(Some(id));
            return;
        };
        let mut selected = self.range_base.clone();
        selected.extend(&order[from.min(to)..=from.max(to)]);
        self.selected = selected;
        self.focus = Some(id);
    }

    /// The focus moves to `id` when it is selected; the selection and the anchor stay.
    pub(super) fn focus_on(&mut self, id: i64) {
        if self.selected.contains(&id) {
            self.focus = Some(id);
        }
    }

    /// Select All: every photo the gallery lists; the focus and the anchor stay.
    pub(super) fn select_all(&mut self, order: &[i64]) {
        self.selected = order.iter().copied().collect();
    }

    /// Moves the focus `offset` photos through the gallery, selecting that photo alone; the first photo
    /// when nothing has the focus. A move past either end keeps the selection.
    pub(super) fn move_by(&mut self, order: &[i64], offset: i64) {
        let Some(first) = order.first() else { return };
        let Some(index) = self.focus.and_then(|focus| order.iter().position(|item| *item == focus)) else {
            self.select(Some(*first));
            return;
        };
        let target = i64::try_from(index).ok().and_then(|index| index.checked_add(offset));
        if let Some(id) = target.and_then(|target| usize::try_from(target).ok()).and_then(|target| order.get(target)) {
            self.select(Some(*id));
        }
    }

    /// Drops the photos the gallery no longer lists.
    pub(super) fn retain(&mut self, listed: &HashSet<i64>) {
        self.selected.retain(|id| listed.contains(id));
        self.range_base.retain(|id| listed.contains(id));
        if self.anchor.is_some_and(|id| !listed.contains(&id)) {
            self.anchor = None;
        }
        if self.focus.is_some_and(|id| !listed.contains(&id)) {
            self.focus = None;
        }
    }
}

#[cfg(test)]
mod tests;
