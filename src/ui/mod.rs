//! Everything the user sees or types, behind one trait.
//!
//! Flows such as [`record`](crate::record) and [`edit`](crate::edit) depend on [`Ui`]
//! rather than on the terminal, so they are exercised in tests with a scripted
//! implementation instead of a TTY.

mod selection;
mod style;
mod terminal;

pub use selection::{Action, Outcome, Selection};
pub use style::{tilde, Pal};
pub use terminal::Terminal;

use crate::Result;

/// The first line of a (possibly multi-line) command, marked with `…` when truncated.
pub fn first_line(text: &str) -> String {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_owned()
    }
}

/// One row of a multi-select list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub title: String,
    pub detail: String,
}

pub trait Ui {
    fn pal(&self) -> Pal;

    /// Print a line of status output.
    fn say(&mut self, text: &str);

    /// Ask a free-form question. Returns [`Error::Cancelled`](crate::Error::Cancelled)
    /// when the input is closed.
    fn ask(&mut self, label: &str, hint: &str) -> Result<String>;

    /// Let the user tick any number of `items`. Returns row indices in selection order.
    fn select_many(&mut self, label: &str, items: &[Choice]) -> Result<Vec<usize>>;

    fn confirm(&mut self, label: &str) -> Result<bool> {
        let answer = self.ask(label, "[y/N]")?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }
}

#[cfg(test)]
pub mod testing {
    //! A [`Ui`] that replays canned answers and records what was said.

    use super::{Choice, Pal, Ui};
    use crate::{Error, Result};
    use std::collections::VecDeque;

    #[derive(Debug)]
    pub enum Reply {
        Text(String),
        Pick(Vec<usize>),
    }

    /// `text("a")` answers the next `ask`.
    pub fn text(s: &str) -> Reply {
        Reply::Text(s.to_owned())
    }

    /// `pick(&[1, 0])` answers the next `select_many`.
    pub fn pick(rows: &[usize]) -> Reply {
        Reply::Pick(rows.to_vec())
    }

    #[derive(Debug, Default)]
    pub struct Scripted {
        replies: VecDeque<Reply>,
        pub said: Vec<String>,
        pub asked: Vec<String>,
    }

    impl Scripted {
        pub fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
            Self {
                replies: replies.into_iter().collect(),
                ..Self::default()
            }
        }

        /// Everything said so far, joined for substring assertions.
        pub fn transcript(&self) -> String {
            self.said.join("\n")
        }

        pub fn exhausted(&self) -> bool {
            self.replies.is_empty()
        }
    }

    impl Ui for Scripted {
        fn pal(&self) -> Pal {
            Pal::plain()
        }

        fn say(&mut self, text: &str) {
            self.said.push(text.to_owned());
        }

        /// An empty script behaves like closed input.
        fn ask(&mut self, label: &str, _hint: &str) -> Result<String> {
            self.asked.push(label.to_owned());
            match self.replies.pop_front() {
                Some(Reply::Text(answer)) => Ok(answer),
                Some(Reply::Pick(_)) => {
                    panic!("script expected a pick but `{label}` asked for text")
                }
                None => Err(Error::Cancelled),
            }
        }

        fn select_many(&mut self, label: &str, _items: &[Choice]) -> Result<Vec<usize>> {
            self.asked.push(label.to_owned());
            match self.replies.pop_front() {
                Some(Reply::Pick(rows)) => Ok(rows),
                Some(Reply::Text(_)) => {
                    panic!("script expected text but `{label}` asked for a pick")
                }
                None => Err(Error::Cancelled),
            }
        }
    }
}
