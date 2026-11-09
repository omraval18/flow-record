//! The state machine behind the checkbox list, independent of any terminal.

/// What a key press asks the list to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    /// Toggle the highlighted row.
    Toggle,
    /// Select every row, or clear them all when everything is already selected.
    ToggleAll,
    /// Toggle the row at this zero-based index.
    Jump(usize),
    Confirm,
    /// Finish with nothing selected.
    Skip,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    /// Indices in the order they were selected.
    Done(Vec<usize>),
    Aborted,
}

#[derive(Debug, Clone)]
pub struct Selection {
    len: usize,
    cursor: usize,
    /// Selected rows in selection order, which becomes the argument order.
    order: Vec<usize>,
}

impl Selection {
    pub fn new(len: usize) -> Self {
        Self {
            len,
            cursor: 0,
            order: Vec::with_capacity(len),
        }
    }

    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// 1-based position of `row` in the selection order, if selected.
    pub fn badge(&self, row: usize) -> Option<usize> {
        self.order.iter().position(|&r| r == row).map(|p| p + 1)
    }

    pub fn apply(&mut self, action: Action) -> Outcome {
        match action {
            Action::Up => self.cursor = (self.cursor + self.len - 1) % self.len.max(1),
            Action::Down => self.cursor = (self.cursor + 1) % self.len.max(1),
            Action::Toggle => self.toggle(self.cursor),
            Action::Jump(row) if row < self.len => self.toggle(row),
            Action::Jump(_) => {}
            Action::ToggleAll if self.order.len() == self.len => self.order.clear(),
            Action::ToggleAll => {
                for row in 0..self.len {
                    if !self.order.contains(&row) {
                        self.order.push(row);
                    }
                }
            }
            Action::Confirm => return Outcome::Done(self.order.clone()),
            Action::Skip => return Outcome::Done(Vec::new()),
            Action::Abort => return Outcome::Aborted,
        }
        Outcome::Continue
    }

    fn toggle(&mut self, row: usize) {
        match self.order.iter().position(|&r| r == row) {
            Some(at) => {
                self.order.remove(at);
            }
            None => self.order.push(row),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(sel: &mut Selection, actions: &[Action]) -> Outcome {
        actions.iter().fold(Outcome::Continue, |_, &a| sel.apply(a))
    }

    #[test]
    fn selection_order_is_the_order_rows_were_toggled() {
        let mut sel = Selection::new(3);
        let done = press(
            &mut sel,
            &[Action::Jump(2), Action::Jump(0), Action::Confirm],
        );
        assert_eq!(done, Outcome::Done(vec![2, 0]));
    }

    #[test]
    fn toggling_twice_deselects_and_later_picks_renumber() {
        let mut sel = Selection::new(3);
        press(
            &mut sel,
            &[Action::Jump(0), Action::Jump(1), Action::Jump(0)],
        );
        assert_eq!((sel.badge(0), sel.badge(1)), (None, Some(1)));
    }

    #[test]
    fn the_cursor_wraps_in_both_directions() {
        let mut sel = Selection::new(3);
        sel.apply(Action::Up);
        assert_eq!(sel.cursor(), 2);
        sel.apply(Action::Down);
        sel.apply(Action::Down);
        assert_eq!(sel.cursor(), 1);
    }

    #[test]
    fn toggle_acts_on_the_highlighted_row() {
        let mut sel = Selection::new(3);
        press(&mut sel, &[Action::Down, Action::Toggle]);
        assert_eq!(sel.badge(1), Some(1));
    }

    #[test]
    fn toggle_all_selects_everything_then_clears_it() {
        let mut sel = Selection::new(3);
        sel.apply(Action::Jump(2));
        sel.apply(Action::ToggleAll);
        assert_eq!(
            sel.apply(Action::Confirm),
            Outcome::Done(vec![2, 0, 1]),
            "existing picks keep their position"
        );
        sel.apply(Action::ToggleAll);
        assert_eq!(sel.apply(Action::Confirm), Outcome::Done(vec![]));
    }

    #[test]
    fn jumping_past_the_end_is_ignored() {
        let mut sel = Selection::new(2);
        sel.apply(Action::Jump(5));
        assert_eq!(sel.apply(Action::Confirm), Outcome::Done(vec![]));
    }

    #[test]
    fn skip_discards_selections_and_abort_reports_cancellation() {
        let mut sel = Selection::new(2);
        sel.apply(Action::Jump(0));
        assert_eq!(sel.apply(Action::Skip), Outcome::Done(vec![]));
        assert_eq!(sel.apply(Action::Abort), Outcome::Aborted);
    }
}
