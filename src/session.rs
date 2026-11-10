//! The per-shell session log written while recording.
//!
//! The file starts with `#<TAB><start directory>[<TAB>fish]`, followed by one line per finished
//! command: `<exit status><TAB><command>`. Backslash, newline and tab are escaped so a
//! multi-line command always occupies exactly one line. Each shell records into its own
//! file, so simultaneous terminals never interleave.

use crate::render::Dialect;
use crate::{Error, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Commands that are never worth replaying.
const SKIPPED: &[&str] = &["exit", "logout"];

/// Exit status of a process interrupted with Ctrl-C (a dev server stopped on purpose).
const SIGINT_STATUS: i32 = 130;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub status: i32,
    pub text: String,
}

impl Recorded {
    /// Whether the command belongs in a workflow. Failures are excluded by default.
    pub const fn succeeded(&self) -> bool {
        self.status == 0 || self.status == SIGINT_STATUS
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recording {
    /// Directory the recording started in; decides project vs. global scope.
    pub cwd: PathBuf,
    pub commands: Vec<Recorded>,
    /// The language the commands were typed in.
    pub shell: Dialect,
}

impl Recording {
    /// Parse a session log. Malformed lines are skipped so a truncated log still yields
    /// everything that was recorded.
    pub fn parse(text: &str) -> Self {
        let mut lines = text.lines();
        let header = lines
            .next()
            .and_then(|l| l.strip_prefix("#\t"))
            .unwrap_or("");
        let (dir, shell) = header.split_once('\t').unwrap_or((header, ""));
        let cwd = PathBuf::from(unescape(dir));
        let shell = if shell == "fish" {
            Dialect::Fish
        } else {
            Dialect::Posix
        };
        let commands = lines
            .filter_map(|line| {
                let (status, text) = line.split_once('\t')?;
                Some(Recorded {
                    status: status.parse().ok()?,
                    text: unescape(text),
                })
            })
            .collect();
        Self {
            cwd,
            commands,
            shell,
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .map_err(|e| Error::io(path, e))
    }
}

/// Create a new, empty session log in `dir` and return its path.
pub fn create(dir: &Path, cwd: &Path, shell: Dialect) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("{}-{nanos}", std::process::id()));
    let tag = if shell == Dialect::Fish { "\tfish" } else { "" };
    let header = format!("#\t{}{tag}\n", escape(&cwd.to_string_lossy()));
    // `create_new` guarantees two shells can never share a file.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| Error::io(&path, e))?;
    file.write_all(header.as_bytes())
        .map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

/// Whether a typed command belongs in a workflow. Running another workflow
/// (`fr build`) does, so workflows compose; controlling fr itself (`fr stop`, `fr list`,
/// `fr -n x`) does not, and neither do `exit`, `logout` or blank input.
pub fn worth_recording(command: &str) -> bool {
    let mut words = command.split_whitespace();
    match words.next() {
        None => false,
        Some("fr") => {
            let rest: Vec<String> = shlex::split(command)
                .unwrap_or_default()
                .into_iter()
                .skip(1)
                .collect();
            crate::cli::is_replay(&rest)
        }
        Some(program) => !SKIPPED.contains(&program),
    }
}

/// Append a finished command. Returns `false` when it is not worth recording.
pub fn append(path: &Path, status: i32, command: &str) -> Result<bool> {
    if !worth_recording(command) {
        return Ok(false);
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    writeln!(file, "{status}\t{}", escape(command)).map_err(|e| Error::io(path, e))?;
    Ok(true)
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\t' => out.push_str(r"\t"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn commands_round_trip_through_the_log_unchanged() {
        let dir = tempdir().unwrap();
        let log = create(dir.path(), Path::new("/work/my project"), Dialect::Fish).unwrap();
        let commands = [
            "echo hi",
            "cat <<EOF\nline one\n\ttabbed\nEOF",
            r"printf '%s\n' 'back\slash' \\ end",
            "trailing backslash \\",
            "tab\tinside",
        ];
        for (i, c) in commands.iter().enumerate() {
            assert!(append(&log, i as i32, c).unwrap());
        }

        let recording = Recording::read(&log).unwrap();
        assert_eq!(recording.cwd, Path::new("/work/my project"));
        assert_eq!(recording.shell, Dialect::Fish);
        let texts: Vec<_> = recording.commands.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, commands);
        assert_eq!(
            recording
                .commands
                .iter()
                .map(|c| c.status)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn every_command_occupies_exactly_one_line() {
        let dir = tempdir().unwrap();
        let log = create(dir.path(), Path::new("/"), Dialect::Posix).unwrap();
        append(&log, 0, "a\nb\nc").unwrap();
        assert_eq!(
            fs::read_to_string(&log).unwrap().lines().count(),
            2,
            "header + one command"
        );
    }

    #[test]
    fn fr_exit_and_blank_input_are_not_recorded() {
        let dir = tempdir().unwrap();
        let log = create(dir.path(), Path::new("/"), Dialect::Posix).unwrap();
        for skipped in [
            "fr",
            "fr list",
            "fr stop",
            "fr -n build",
            "fr edit x",
            "exit",
            "logout",
            "   ",
            "",
        ] {
            assert!(!append(&log, 0, skipped).unwrap(), "{skipped:?}");
        }
        for kept in [
            "frobnicate",
            "fr build",
            "fr -g deploy prod",
            "fr run build",
        ] {
            assert!(append(&log, 0, kept).unwrap(), "{kept:?}");
        }
        assert_eq!(Recording::read(&log).unwrap().commands.len(), 4);
    }

    #[test]
    fn parse_skips_malformed_lines_and_survives_a_missing_header() {
        let r = Recording::parse("#\t/x\n0\tgood\nnot a record\nabc\tbad status\n2\tfailed\n");
        assert_eq!(
            r.commands,
            [
                Recorded {
                    status: 0,
                    text: "good".into()
                },
                Recorded {
                    status: 2,
                    text: "failed".into()
                }
            ]
        );

        let headerless = Recording::parse("0\tls\n");
        assert_eq!(headerless.cwd, PathBuf::new());
        assert!(
            headerless.commands.is_empty(),
            "the first line is always the header"
        );
    }

    #[test]
    fn only_success_and_interrupt_count_as_succeeded() {
        let status = |status| {
            Recorded {
                status,
                text: String::new(),
            }
            .succeeded()
        };
        assert!(status(0) && status(130));
        assert!(!status(1) && !status(127) && !status(137));
    }

    #[test]
    fn two_logs_never_collide() {
        let dir = tempdir().unwrap();
        let a = create(dir.path(), Path::new("/"), Dialect::Posix).unwrap();
        let b = create(dir.path(), Path::new("/"), Dialect::Posix).unwrap();
        assert_ne!(a, b);
    }
}
