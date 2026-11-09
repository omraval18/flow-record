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
