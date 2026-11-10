//! Suggestions derived from recorded steps (pure functions).

use crate::render::find_words;
use std::collections::HashSet;

/// A literal that appears in several steps and so may be an argument in disguise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub value: String,
    /// Number of steps the literal appears in.
    pub steps: usize,
}

const MAX_CANDIDATES: usize = 8;
const MIN_LEN: usize = 3;

fn tokenize(step: &str) -> Vec<String> {
    shlex::split(step).unwrap_or_default()
}

/// Whole-word literals (not flags or program names) used in two or more steps, most
/// repeated first. A literal that merely contains a better candidate (`./app` next to
/// `app`) is dropped.
pub fn candidates(steps: &[String]) -> Vec<Candidate> {
    let tokens: Vec<Vec<String>> = steps.iter().map(|s| tokenize(s)).collect();
    let programs: HashSet<&str> = tokens
        .iter()
        .filter_map(|t| t.first().map(String::as_str))
        .collect();

    let mut seen = HashSet::new();
    let mut found: Vec<Candidate> = Vec::new();
    for word in tokens.iter().flat_map(|t| t.iter().skip(1)) {
        // `--name=app` contributes `app`; a bare flag contributes nothing.
        let value = if word.starts_with('-') {
            word.split_once('=').map_or("", |(_, v)| v)
        } else {
            word
        };
        if value.len() < MIN_LEN || programs.contains(value) || !seen.insert(value) {
            continue;
        }
        let steps_with = steps
            .iter()
            .filter(|s| find_words(s, value).next().is_some())
            .count();
        if steps_with >= 2 {
            found.push(Candidate {
                value: value.to_owned(),
                steps: steps_with,
            });
        }
    }

    found.sort_by_key(|c| (std::cmp::Reverse(c.steps), c.value.len()));
    let mut kept: Vec<Candidate> = Vec::new();
    for candidate in found {
        if !kept
            .iter()
            .any(|k| find_words(&candidate.value, &k.value).next().is_some())
        {
            kept.push(candidate);
        }
    }
    kept.truncate(MAX_CANDIDATES);
    kept
}

const FILLER_COMMANDS: &[&str] = &[
    "cd", "ls", "clear", "pwd", "echo", "cat", "mkdir", "touch", "export", "source", "code", "open",
];

/// A short name for the workflow: `create-next-app …` → `next`, `cargo new x` → `cargo-new`.
pub fn suggest_name(steps: &[String]) -> String {
    let tokens: Vec<Vec<String>> = steps.iter().map(|s| tokenize(s)).collect();

    let scaffold = tokens.iter().flatten().find_map(|w| {
        let rest = w.strip_prefix("create-")?;
        Some(rest.strip_suffix("-app").unwrap_or(rest))
    });
    if let Some(name) = scaffold.map(sanitize).filter(|n| n != "workflow") {
        return name;
    }

    for words in &tokens {
        let Some(program) = words.first().map(|p| p.rsplit('/').next().unwrap_or(p)) else {
            continue;
        };
        if FILLER_COMMANDS.contains(&program) {
            continue;
        }
        let subcommand = words
            .get(1)
            .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric()));
        return sanitize(&match subcommand {
            Some(sub) => format!("{program}-{sub}"),
            None => program.to_owned(),
        });
    }
    "workflow".to_owned()
}

// ---- habits: command sequences typed again and again --------------------------------

/// A sequence of commands typed in the same order several times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Habit {
    /// The most recent occurrence, as typed.
    pub commands: Vec<String>,
    pub times: usize,
    /// Words that changed between occurrences (taken from the most recent one): the
    /// likely arguments.
    pub varying: Vec<String>,
}

/// A sequence of command shapes and where it occurs: `(session, start index)`.
type Occurrences = (Vec<String>, Vec<(usize, usize)>);

const MIN_TIMES: usize = 3;
const MAX_HABITS: usize = 5;
const SEQUENCE_LENS: std::ops::RangeInclusive<usize> = 2..=5;

/// Programs whose first argument is a value (a path, a host…), not a subcommand.
const VALUE_FIRST: &[&str] = &[
    "cd", "mkdir", "touch", "cat", "rm", "cp", "mv", "ls", "code", "open", "vim", "nvim", "vi",
    "nano", "echo", "ssh", "curl", "wget", "less", "head", "tail", "source", "z", "j",
];

/// Commands that say nothing about a workflow on their own.
const TRIVIAL: &[&str] = &[
    "ls", "ll", "la", "l", "cd", "pwd", "clear", "cls", "history", "which", "man", "cat", "less",
    "echo", "z", "j", "exit", "true",
];

fn words_of(command: &str) -> Vec<String> {
    shlex::split(command).unwrap_or_else(|| command.split_whitespace().map(str::to_owned).collect())
}

/// What a command does, with its values blanked out: `git commit -m "x"` and
/// `git commit -m "y"` share the shape `git commit -m *`.
fn shape(command: &str) -> String {
    let words = words_of(command);
    let program = words.first().map_or("", String::as_str);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let is_subcommand = i == 1
                && !VALUE_FIRST.contains(&program)
                && w.len() <= 24
                && w.chars().all(|c| c.is_ascii_lowercase() || c == '-');
            if i == 0 || is_subcommand || (w.starts_with('-') && !w.contains('=')) {
                w.clone()
            } else if let Some((flag, _)) = w.split_once('=').filter(|_| w.starts_with('-')) {
                format!("{flag}=*")
            } else {
                "*".to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_trivial(command: &str) -> bool {
    let words = words_of(command);
    match words.first().map(String::as_str) {
        None => true,
        Some("git") => matches!(
            words.get(1).map(String::as_str),
            None | Some("status" | "diff" | "log" | "show" | "branch")
        ),
        Some(program) => TRIVIAL.contains(&program),
    }
}

/// Words that differ between occurrences of the same sequence, from the latest one.
fn varying_words(occurrences: &[&[&str]]) -> Vec<String> {
    let Some(latest) = occurrences.last() else {
        return Vec::new();
    };
    let split: Vec<Vec<Vec<String>>> = occurrences
        .iter()
        .map(|o| o.iter().map(|c| words_of(c)).collect())
        .collect();
    let mut out: Vec<String> = Vec::new();
    for (step, command) in latest.iter().enumerate() {
        let words = words_of(command);
        for (i, word) in words.iter().enumerate().skip(1) {
            let differs = split
                .iter()
                .any(|o| o[step].len() != words.len() || o[step].get(i).is_some_and(|w| w != word));
            if differs && !word.starts_with('-') && !out.contains(word) {
                out.push(word.clone());
            }
        }
    }
    out
}

/// Find sequences of commands that were typed in the same order at least three times.
///
/// `sessions` are runs of successful commands, oldest first. Sequences that only move
/// around (`cd`, `ls`, `git status`…) or that an existing workflow (`known`, its steps)
/// already covers are left out, as is a short sequence explained by a longer one.
pub fn habits(sessions: &[Vec<&str>], known: &[Vec<String>]) -> Vec<Habit> {
    let shapes: Vec<Vec<String>> = sessions
        .iter()
        .map(|s| s.iter().map(|c| shape(c)).collect())
        .collect();
    let known: Vec<Vec<String>> = known
        .iter()
        .map(|steps| steps.iter().map(|s| shape(s)).collect())
        .collect();
    let contains = |haystack: &[String], needle: &[String]| {
        haystack.windows(needle.len()).any(|w| w == needle)
    };

    // shape sequence -> occurrences (session, start), non-overlapping within a session
    let mut found: Vec<Occurrences> = Vec::new();
    for len in SEQUENCE_LENS {
        let mut index: std::collections::HashMap<&[String], usize> =
            std::collections::HashMap::new();
        let mut local: Vec<Occurrences> = Vec::new();
        for (s, session_shapes) in shapes.iter().enumerate() {
            for start in 0..session_shapes.len().saturating_sub(len - 1) {
                let key = &session_shapes[start..start + len];
                let slot = *index.entry(key).or_insert_with(|| {
                    local.push((key.to_vec(), Vec::new()));
                    local.len() - 1
                });
                let hits = &mut local[slot].1;
                let overlaps = hits
                    .last()
                    .is_some_and(|&(ls, lstart)| ls == s && lstart + len > start);
                if !overlaps {
                    hits.push((s, start));
                }
            }
        }
        found.extend(local.into_iter().filter(|(key, hits)| {
            hits.len() >= MIN_TIMES
                && key.iter().any(|k| !k.is_empty())
                && key.windows(2).any(|w| w[0] != w[1])
                && !known.iter().any(|k| contains(k, key))
        }));
    }

    let commands_at =
        |&(s, start): &(usize, usize), len: usize| -> &[&str] { &sessions[s][start..start + len] };
    found.retain(|(key, hits)| {
        !commands_at(&hits[hits.len() - 1], key.len())
            .iter()
            .all(|c| is_trivial(c))
    });

    // Longest first, so a shorter sequence mostly explained by a longer one is dropped.
    found.sort_by_key(|(key, hits)| (std::cmp::Reverse(key.len()), std::cmp::Reverse(hits.len())));
    let mut kept: Vec<Occurrences> = Vec::new();
    for (key, hits) in found {
        let explained = kept
            .iter()
            .any(|(k, h)| contains(k, &key) && h.len() * 3 >= hits.len() * 2);
        if !explained {
            kept.push((key, hits));
        }
    }
    kept.sort_by_key(|(key, hits)| std::cmp::Reverse(hits.len() * key.len()));
    kept.truncate(MAX_HABITS);

    kept.into_iter()
        .map(|(key, hits)| {
            let occurrences: Vec<&[&str]> =
                hits.iter().map(|h| commands_at(h, key.len())).collect();
            Habit {
                commands: occurrences[occurrences.len() - 1]
                    .iter()
                    .map(|c| (*c).to_owned())
                    .collect(),
                times: hits.len(),
                varying: varying_words(&occurrences),
            }
        })
        .collect()
}

fn sanitize(raw: &str) -> String {
    let lowered: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = lowered.trim_matches(['-', '_']);
    if trimmed.is_empty() {
        "workflow".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn values(found: &[Candidate]) -> Vec<(&str, usize)> {
        found.iter().map(|c| (c.value.as_str(), c.steps)).collect()
    }

    #[test]
    fn finds_the_repeated_project_name_but_not_one_off_packages() {
        let s = steps(&[
            "pnpm dlx create-next-app dashboard",
            "cd dashboard",
            "pnpm add zustand",
        ]);
        assert_eq!(values(&candidates(&s)), [("dashboard", 2)]);
    }

    #[test]
    fn ignores_programs_flags_and_short_words() {
        let s = steps(&["git add -A", "git commit -m go", "git push -u origin go"]);
        assert!(
            candidates(&s).is_empty(),
            "git/-A/go/origin are programs, flags or too short or single-use"
        );
    }

    #[test]
    fn takes_the_value_of_key_value_flags() {
        let s = steps(&["serve --port=3000", "open localhost:3000"]);
        assert_eq!(values(&candidates(&s)), [("3000", 2)]);
    }

    #[test]
    fn drops_literals_that_contain_a_better_candidate() {
        let s = steps(&["mkdir app", "cd ./app", "ls ./app/src"]);
        assert_eq!(values(&candidates(&s)), [("app", 3)]);
    }

    #[test]
    fn orders_by_how_often_a_literal_repeats() {
        let s = steps(&["echo alpha beta", "echo beta gamma", "echo beta alpha"]);
        assert_eq!(values(&candidates(&s)), [("beta", 3), ("alpha", 2)]);
    }

    #[test]
    fn caps_the_list() {
        let words: Vec<String> = (0..20).map(|i| format!("word{i:02}")).collect();
        let line = format!("echo {}", words.join(" "));
        assert_eq!(candidates(&[line.clone(), line]).len(), MAX_CANDIDATES);
    }

    // ---- habits ------------------------------------------------------------------------

    fn session<'a>(commands: &[&'a str]) -> Vec<&'a str> {
        commands.to_vec()
    }

    #[test]
    fn a_sequence_typed_three_times_with_different_values_is_a_habit() {
        let sessions = [
            session(&["git add -A", "git commit -m 'first'", "git push", "ls"]),
            session(&["cd x", "git add -A", "git commit -m 'second'", "git push"]),
            session(&["git add -A", "git commit -m 'third fix'", "git push"]),
        ];
        let found = habits(&sessions, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0],
            Habit {
                commands: steps(&["git add -A", "git commit -m 'third fix'", "git push"]),
                times: 3,
                varying: steps(&["third fix"]),
            }
        );
    }

    #[test]
    fn twice_is_not_a_habit_and_moving_around_never_is() {
        let twice = [
            session(&["make", "make install"]),
            session(&["make", "make install"]),
        ];
        assert!(habits(&twice, &[]).is_empty());
        let wandering = vec![session(&["cd a", "ls", "git status"]); 4];
        assert!(habits(&wandering, &[]).is_empty());
    }

    #[test]
    fn repeats_inside_one_long_session_count_without_overlapping() {
        let one = [session(&[
            "cargo fmt",
            "cargo test",
            "cargo fmt",
            "cargo test",
            "cargo fmt",
            "cargo test",
        ])];
        let found = habits(&one, &[]);
        assert_eq!(found[0].commands, steps(&["cargo fmt", "cargo test"]));
        assert_eq!(found[0].times, 3);
    }

    #[test]
    fn a_sequence_an_existing_workflow_covers_is_not_suggested() {
        let sessions = vec![session(&["pnpm i", "pnpm build"]); 3];
        assert_eq!(habits(&sessions, &[]).len(), 1);
        let known = [steps(&["pnpm i", "pnpm build", "pnpm start"])];
        assert!(habits(&sessions, &known).is_empty());
    }

    #[test]
    fn the_shorter_part_of_a_longer_habit_is_not_listed_separately() {
        let sessions = vec![session(&["docker compose up -d", "pnpm migrate", "pnpm dev"]); 4];
        let found = habits(&sessions, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].commands.len(), 3);
    }

    #[test]
    fn shapes_keep_programs_subcommands_and_flags_but_blank_values() {
        assert_eq!(shape("git commit -m 'hello world'"), "git commit -m *");
        assert_eq!(shape("cd my-app"), "cd *");
        assert_eq!(shape("serve --port=3000 ./dist"), "serve --port=* *");
    }

    #[test]
    fn names_scaffolders_after_what_they_create() {
        assert_eq!(
            suggest_name(&steps(&["pnpm dlx create-next-app dashboard"])),
            "next"
        );
        assert_eq!(suggest_name(&steps(&["npm create-vite x"])), "vite");
        assert_eq!(
            suggest_name(&steps(&["cd x", "pnpm dlx create-t3-app@latest y"])),
            "t3-app-latest"
        );
    }

    #[test]
    fn names_other_workflows_after_their_first_meaningful_command() {
        assert_eq!(
            suggest_name(&steps(&["cd x", "cargo new demo"])),
            "cargo-new"
        );
        assert_eq!(
            suggest_name(&steps(&["./scripts/deploy.sh --prod"])),
            "deploy-sh"
        );
        assert_eq!(
            suggest_name(&steps(&["cd a", "ls"])),
            "workflow",
            "only filler commands"
        );
    }
}
