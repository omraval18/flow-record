//! The command history the shell hooks keep, so a workflow can be saved after the fact
//! (`fr save`) and repeated habits can be spotted (`fr suggest`).
//!
//! The hooks append one line per finished command using only shell builtins, so logging
//! costs no process. Each line is
//! `<unix time>\t<shell pid>\t<exit status>\t<directory>\t<command>`, with backslash,
//! newline and tab escaped in the last two fields. The time may be empty (bash 3 has no
//! clock builtin). The log is trimmed from the front once it grows past a limit.

use crate::session::{unescape, Recorded};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LINES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Unix seconds; 0 when the shell could not tell.
    pub at: u64,
    pub pid: u32,
    pub status: i32,
    /// Where the command was typed.
    pub dir: PathBuf,
    pub command: String,
}

impl Entry {
    pub fn recorded(&self) -> Recorded {
        Recorded {
            status: self.status,
            text: self.command.clone(),
        }
    }
}

/// Parse the log, skipping lines that are malformed (a write cut short, a newer format).
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(5, '\t');
            let (at, pid, status, dir, command) =
                (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
            Some(Entry {
                at: at.parse().unwrap_or(0),
                pid: pid.parse().ok()?,
                status: status.parse().ok()?,
                dir: PathBuf::from(unescape(dir)),
                command: unescape(command),
            })
        })
        .filter(|e| !e.command.trim().is_empty())
        .collect()
}

/// Read the log; a missing log is an empty history.
pub fn read(path: &Path) -> Vec<Entry> {
    fs::read_to_string(path)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// Keep the log bounded. Called opportunistically; failure only means it stays large.
pub fn compact(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.len() <= MAX_BYTES {
        return Ok(());
    }
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let kept = lines[lines.len().saturating_sub(KEEP_LINES)..].join("\n");
    let staging = path.with_extension("tmp");
    fs::write(&staging, kept + "\n")?;
    fs::rename(&staging, path)
}

/// Split the history into runs of consecutive commands from one shell, breaking at
/// long pauses, which is the unit repeated sequences are looked for in.
pub fn sessions(entries: &[Entry]) -> Vec<Vec<&Entry>> {
    const PAUSE_SECS: u64 = 60 * 60;
    let mut by_shell: Vec<(u32, Vec<&Entry>)> = Vec::new();
    for e in entries {
        match by_shell.iter_mut().find(|(pid, _)| *pid == e.pid) {
            Some((_, run)) => run.push(e),
            None => by_shell.push((e.pid, vec![e])),
        }
    }
    let mut out = Vec::new();
    for (_, run) in by_shell {
        let mut current: Vec<&Entry> = Vec::new();
        for e in run {
            let paused = current
                .last()
                .is_some_and(|prev| prev.at > 0 && e.at > prev.at + PAUSE_SECS);
            if paused {
                out.push(std::mem::take(&mut current));
            }
            current.push(e);
        }
        out.push(current);
    }
    out.retain(|s| !s.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(at: u64, pid: u32, command: &str) -> Entry {
        Entry {
            at,
            pid,
            status: 0,
            dir: PathBuf::from("/w"),
            command: command.into(),
        }
    }

    #[test]
    fn lines_written_by_the_hooks_parse_back() {
        let log = "1700000000\t42\t0\t/my\\tdir\tcat <<EOF\\nhi\\nEOF\n\
                   \t43\t1\t/w\tfalse\n\
                   garbage\n\
                   1\tnot-a-pid\t0\t/w\tls\n\
                   2\t44\t0\t/w\t   \n";
        assert_eq!(
            parse(log),
            [
                Entry {
                    at: 1_700_000_000,
                    pid: 42,
                    status: 0,
                    dir: PathBuf::from("/my\tdir"),
                    command: "cat <<EOF\nhi\nEOF".into()
                },
                Entry {
                    at: 0,
                    pid: 43,
                    status: 1,
                    dir: PathBuf::from("/w"),
                    command: "false".into()
                }
            ]
        );
    }

    #[test]
    fn sessions_separate_shells_and_long_pauses() {
        let entries = [
            entry(100, 1, "a"),
            entry(101, 2, "x"),
            entry(102, 1, "b"),
            entry(100_000, 1, "c"),
            entry(0, 2, "y"),
        ];
        let commands: Vec<Vec<&str>> = sessions(&entries)
            .iter()
            .map(|s| s.iter().map(|e| e.command.as_str()).collect())
            .collect();
        assert_eq!(commands, [vec!["a", "b"], vec!["c"], vec!["x", "y"]]);
    }

    #[test]
    fn compaction_keeps_the_newest_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("history");
        let line = format!("1\t1\t0\t/w\t{}\n", "x".repeat(200));
        let mut text = "1\t1\t0\t/w\toldest\n".to_owned();
        text.push_str(&line.repeat(12_000));
        text.push_str("1\t1\t0\t/w\tnewest\n");
        fs::write(&path, text).unwrap();

        compact(&path).unwrap();

        let entries = read(&path);
        assert_eq!(entries.len(), KEEP_LINES);
        assert_eq!(entries.last().unwrap().command, "newest");
        assert!(entries.iter().all(|e| e.command != "oldest"));
    }
}
