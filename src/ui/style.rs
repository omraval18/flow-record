//! ANSI styling that switches itself off for pipes and `NO_COLOR`.

use std::io::IsTerminal;
use std::path::Path;

/// A palette bound to one output stream. When colour is off every method returns its
/// input unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pal {
    color: bool,
}

impl Pal {
    /// Colour for stderr, which carries all prompts and status output.
    pub fn stderr() -> Self {
        Self::detect(std::io::stderr().is_terminal())
    }

    /// Colour for stdout, which carries listings.
    pub fn stdout() -> Self {
        Self::detect(std::io::stdout().is_terminal())
    }

    /// No styling at all; used by tests and plain output.
    pub const fn plain() -> Self {
        Self { color: false }
    }

    fn detect(is_terminal: bool) -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Self {
            color: is_terminal && !no_color,
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }
}

/// Shorten a leading `$HOME` to `~` for display.
pub fn tilde(path: &Path) -> String {
    let shown = path.display().to_string();
    match std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty())
    {
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => shown,
        },
        None => shown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_palette_returns_text_unchanged() {
        let p = Pal::plain();
        assert_eq!(p.bold("x"), "x");
        assert_eq!(p.red("x"), "x");
    }

    #[test]
    fn coloured_palette_wraps_text_in_reset_terminated_sequences() {
        let p = Pal { color: true };
        assert_eq!(p.green("ok"), "\x1b[32mok\x1b[0m");
    }
}
