//! The shells fr integrates with, and the integration script each one loads.
//!
//! Every integration provides the same pieces: an `fr` function that `eval`s replay
//! scripts in the current shell, hooks that log each finished command, a recording
//! indicator, completion, and the helpers replay scripts call (`__fr_remember`, plus
//! `__fr_status` in fish).

use crate::render::{self, Dialect, Quote};
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shell {
    #[default]
    Zsh,
    Bash,
    Fish,
}

impl Shell {
    pub const ALL: [Self; 3] = [Self::Zsh, Self::Bash, Self::Fish];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Zsh => "zsh",
            Self::Bash => "bash",
            Self::Fish => "fish",
        }
    }

    pub const fn dialect(self) -> Dialect {
        match self {
            Self::Zsh | Self::Bash => Dialect::Posix,
            Self::Fish => Dialect::Fish,
        }
    }

    /// Where `eval "$(fr init …)"` belongs.
    pub const fn rc_file(self) -> &'static str {
        match self {
            Self::Zsh => "~/.zshrc",
            Self::Bash => "~/.bashrc",
            Self::Fish => "~/.config/fish/config.fish",
        }
    }

    pub const fn setup_line(self) -> &'static str {
        match self {
            Self::Zsh => "eval \"$(fr init zsh)\"",
            Self::Bash => "eval \"$(fr init bash)\"",
            Self::Fish => "fr init fish | source",
        }
    }

    const fn script(self) -> &'static str {
        match self {
            Self::Zsh => include_str!("../shell/fr.zsh"),
            Self::Bash => include_str!("../shell/fr.bash"),
            Self::Fish => include_str!("../shell/fr.fish"),
        }
    }

    /// The full integration: where to log commands, the shared script, then one
    /// function per shim so `next` works like `fr next`.
    pub fn integration(self, history_log: &Path, shims: &[String]) -> String {
        let dialect = self.dialect();
        let log =
            render::quote_in(dialect, &history_log.to_string_lossy(), Quote::Bare).into_owned();
        let mut out = match self {
            Self::Fish => format!("set -g __fr_log {log}\n"),
            Self::Zsh | Self::Bash => format!("__fr_log={log}\n"),
        };
        out.push_str(self.script());
        for name in shims {
            out.push_str("__fr_shim ");
            out.push_str(name);
            out.push('\n');
        }
        out
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_shells_are_rejected() {
        for shell in Shell::ALL {
            assert_eq!(Shell::parse(shell.name()), Some(shell));
        }
        assert_eq!(Shell::parse("tcsh"), None);
    }

    #[test]
    fn the_integration_points_at_the_log_and_defines_shims_last() {
        let text = Shell::Bash.integration(Path::new("/x y/history"), &["next".into()]);
        assert!(text.starts_with("__fr_log='/x y/history'\n"), "{text}");
        assert!(text.trim_end().ends_with("__fr_shim next"));
        let fish = Shell::Fish.integration(Path::new("/h"), &[]);
        assert!(fish.starts_with("set -g __fr_log /h\n"));
    }
}
