//! The real terminal: prompts on stderr, answers from stdin.
//!
//! Stdout is reserved for machine-readable output (the script the shell function evals),
//! while stdin stays attached to the user's terminal even inside `$(...)`, so prompting
//! works in both cases.

use super::selection::{Action, Outcome, Selection};
use super::{Choice, Pal, Ui};
use crate::{Error, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(Debug)]
pub struct Terminal {
    pal: Pal,
}

impl Terminal {
    pub fn new() -> Self {
        Self { pal: Pal::stderr() }
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui for Terminal {
    fn pal(&self) -> Pal {
        self.pal
    }

    fn say(&mut self, text: &str) {
        eprintln!("{text}");
    }

    fn ask(&mut self, label: &str, hint: &str) -> Result<String> {
        let p = self.pal;
        let hint = if hint.is_empty() {
            String::new()
        } else {
            format!(" {}", p.dim(hint))
        };
        eprint!("  {} {}{hint} {} ", p.cyan("?"), p.bold(label), p.dim("›"));
        io::stderr().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            return Err(Error::Cancelled);
        }
        Ok(line.trim_end_matches(['\n', '\r']).to_owned())
    }

    fn select_many(&mut self, label: &str, items: &[Choice]) -> Result<Vec<usize>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        if io::stdin().is_terminal() && io::stderr().is_terminal() {
            self.select_interactive(label, items)
        } else {
            self.select_typed(label, items)
        }
    }
}

impl Terminal {
    /// Fallback for pipes and dumb terminals: numbered rows, answered by typing numbers.
    fn select_typed(&mut self, label: &str, items: &[Choice]) -> Result<Vec<usize>> {
        let p = self.pal;
        for (i, item) in items.iter().enumerate() {
            self.say(&format!(
                "    {}  {}  {}",
                p.cyan(&(i + 1).to_string()),
                item.title,
                p.dim(&item.detail)
            ));
        }
        self.say("");
        let answer = self.ask(label, "numbers (e.g. 1 2) · a = all · Enter = none")?;
        Ok(parse_numbers(&answer, items.len()))
    }

    fn select_interactive(&mut self, label: &str, items: &[Choice]) -> Result<Vec<usize>> {
        let picked = run_checkbox(self.pal, label, items)?;
        let p = self.pal;
        match &picked {
            Some(rows) if !rows.is_empty() => {
                let titles: Vec<&str> = rows.iter().map(|&i| items[i].title.as_str()).collect();
                self.say(&format!(
                    "  {} {}  {}",
                    p.green("✓"),
                    p.bold(label),
                    titles.join(", ")
                ));
            }
            Some(_) => self.say(&p.dim(&format!("  – {label}: none"))),
            None => {}
        }
        picked.ok_or(Error::Cancelled)
    }
}

/// Parse `1 2`, `1,2` or `a`/`all` into zero-based rows, dropping duplicates and anything
/// out of range, keeping the order typed.
fn parse_numbers(answer: &str, len: usize) -> Vec<usize> {
    if matches!(answer.trim(), "a" | "all") {
        return (0..len).collect();
    }
    let mut rows = Vec::new();
    for n in answer
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter_map(|t| t.parse::<usize>().ok())
    {
        if (1..=len).contains(&n) && !rows.contains(&(n - 1)) {
            rows.push(n - 1);
        }
    }
    rows
}

/// The frame drawn for the checkbox list: a header and one row per item. Lines end in
/// `\r\n` because the terminal is in raw mode while it is shown.
fn frame(pal: Pal, label: &str, items: &[Choice], sel: &Selection) -> String {
    let mut out = format!(
        "  {} {}  {}\r\n",
        pal.cyan("?"),
        pal.bold(label),
        pal.dim("↑↓ move · space select · a all · enter confirm · esc skip")
    );
    for (row, item) in items.iter().enumerate() {
        let badge = match sel.badge(row) {
            Some(n) => pal.green(&format!("[{n}]")),
            None => pal.dim("[ ]"),
        };
        let (pointer, title) = if row == sel.cursor() {
            (pal.cyan("❯"), pal.bold(&item.title))
        } else {
            (" ".to_owned(), item.title.clone())
        };
        out.push_str(&format!(
            "    {pointer} {badge} {title}  {}\r\n",
            pal.dim(&item.detail)
        ));
    }
    out
}

fn action_for(key: KeyEvent) -> Option<Action> {
    Some(match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Abort,
        KeyCode::Up | KeyCode::Char('k') => Action::Up,
        KeyCode::Down | KeyCode::Char('j') => Action::Down,
        KeyCode::Char(' ') => Action::Toggle,
        KeyCode::Char('a') => Action::ToggleAll,
        KeyCode::Char(d @ '1'..='9') => Action::Jump(d as usize - '1' as usize),
        KeyCode::Enter => Action::Confirm,
        KeyCode::Esc | KeyCode::Char('q') => Action::Skip,
        _ => return None,
    })
}

/// Restores the terminal even if drawing fails half way.
struct RawMode;

impl RawMode {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stderr(), cursor::Show);
    }
}

/// Show the list and block until the user decides. `None` means Ctrl-C.
fn run_checkbox(pal: Pal, label: &str, items: &[Choice]) -> Result<Option<Vec<usize>>> {
    let mut out = io::stderr();
    let height = u16::try_from(items.len() + 1).unwrap_or(u16::MAX);
    let mut sel = Selection::new(items.len());
    let _raw = RawMode::enter()?;
    queue!(out, cursor::Hide)?;

    let mut redraw = false;
    let outcome = loop {
        if redraw {
            queue!(out, cursor::MoveUp(height))?;
        }
        redraw = true;
        queue!(out, terminal::Clear(ClearType::FromCursorDown))?;
        out.write_all(frame(pal, label, items, &sel).as_bytes())?;
        out.flush()?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if let Some(action) = action_for(key) {
            match sel.apply(action) {
                Outcome::Continue => {}
                done => break done,
            }
        }
    };

    queue!(
        out,
        cursor::MoveUp(height),
        terminal::Clear(ClearType::FromCursorDown),
        cursor::Show
    )?;
    out.flush()?;
    Ok(match outcome {
        Outcome::Done(rows) => Some(rows),
        Outcome::Aborted | Outcome::Continue => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::selection::Action;

    fn items(titles: &[&str]) -> Vec<Choice> {
        titles
            .iter()
            .map(|t| Choice {
                title: (*t).into(),
                detail: "used in 2 steps".into(),
            })
            .collect()
    }

    #[test]
    fn typed_answers_keep_order_and_drop_duplicates_and_out_of_range_rows() {
        assert_eq!(parse_numbers("2 1", 3), [1, 0]);
        assert_eq!(parse_numbers("1,1, 3", 3), [0, 2]);
        assert_eq!(parse_numbers("0 4 x", 3), Vec::<usize>::new());
        assert_eq!(parse_numbers("", 3), Vec::<usize>::new());
    }

    #[test]
    fn typed_answer_a_selects_everything_in_list_order() {
        assert_eq!(parse_numbers(" a ", 3), [0, 1, 2]);
        assert_eq!(parse_numbers("all", 2), [0, 1]);
    }

    #[test]
    fn frame_shows_the_cursor_and_selection_order() {
        let list = items(&["demo", "3000", "extra"]);
        let mut sel = Selection::new(3);
        sel.apply(Action::Jump(1));
        sel.apply(Action::Jump(0));
        sel.apply(Action::Down);
        let text = frame(Pal::plain(), "Make dynamic", &list, &sel);
        let rows: Vec<&str> = text.split("\r\n").collect();

        assert!(rows[0].contains("Make dynamic"));
        assert!(rows[1].contains("[2] demo") && !rows[1].contains('❯'));
        assert!(rows[2].contains("❯ [1] 3000"), "{}", rows[2]);
        assert!(rows[3].contains("[ ] extra"));
    }

    #[test]
    fn keys_map_to_actions() {
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(action_for(key(KeyCode::Char('j'))), Some(Action::Down));
        assert_eq!(action_for(key(KeyCode::Char('3'))), Some(Action::Jump(2)));
        assert_eq!(action_for(key(KeyCode::Esc)), Some(Action::Skip));
        assert_eq!(action_for(key(KeyCode::Char('z'))), None);
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Abort)
        );
        assert_eq!(
            action_for(key(KeyCode::Char('c'))),
            None,
            "a plain `c` is not Ctrl-C"
        );
    }
}
