//! The review that follows `fr stop`: turn a recorded session into a saved workflow with
//! as few questions as possible (which values are arguments, a name, a scope).

use crate::history;
use crate::render::Dialect;
use crate::session::{self, Recorded, Recording};
use crate::store::{self, Store};
use crate::suggest::{self, Candidate, Habit};
use crate::ui::{first_line, tilde, Choice, Ui};
use crate::workflow::{validate_name, Param, Scope, Workflow};
use crate::{render, Error, Result};
use std::path::{Path, PathBuf};

/// How many recent commands `fr save` offers.
const SAVE_WINDOW: usize = 15;

const DESCRIPTION_LEN: usize = 60;

/// What was saved.
#[derive(Debug, PartialEq, Eq)]
pub struct Saved {
    pub name: String,
    pub scope: Scope,
    pub path: PathBuf,
    pub params: Vec<String>,
}

/// `fr stop`: review the session log at `session_file`, then delete it.
pub fn finish(ui: &mut dyn Ui, store: &Store, session_file: &Path) -> Result<()> {
    let recording = Recording::read(session_file)?;
    let outcome = review(ui, store, recording, &[]);
    // The log has served its purpose whether or not the user saved; failing to delete a
    // temp file must not mask the outcome.
    let _ = std::fs::remove_file(session_file);
    outcome.map(|_| ())
}

/// `fr save [n]`: turn commands this shell already ran into a workflow — the last `n`,
/// or the ones picked from the recent list.
pub fn save(
    ui: &mut dyn Ui,
    store: &Store,
    pid: u32,
    count: Option<usize>,
    shell: Dialect,
) -> Result<()> {
    let log = store.dirs().history_log();
    let _ = history::compact(&log);
    let entries: Vec<history::Entry> = history::read(&log)
        .into_iter()
        .filter(|e| {
            e.pid == pid && e.recorded().succeeded() && session::worth_recording(&e.command)
        })
        .collect();
    if entries.is_empty() {
        ui.say("  No commands from this shell yet. Run some, then `fr save`.");
        return Ok(());
    }
    let picked: Vec<&history::Entry> = match count {
        Some(n) => entries[entries.len().saturating_sub(n)..].iter().collect(),
        None => {
            let recent = &entries[entries.len().saturating_sub(SAVE_WINDOW)..];
            let pal = ui.pal();
            ui.say(&format!("\n  {}", pal.bold("Recent commands")));
            ui.say(&format!(
                "  {}\n",
                pal.dim("Pick the steps for the new workflow; they run in the order shown.")
            ));
            let items: Vec<Choice> = recent
                .iter()
                .map(|e| Choice {
                    title: first_line(&e.command),
                    detail: if e.dir == store.cwd() {
                        String::new()
                    } else {
                        tilde(&e.dir)
                    },
                })
                .collect();
            let mut rows = ui.select_many("Steps", &items)?;
            rows.sort_unstable();
            rows.into_iter().map(|r| &recent[r]).collect()
        }
    };
    let Some(first) = picked.first() else {
        ui.say("  Nothing picked.");
        return Ok(());
    };
    let recording = Recording {
        cwd: first.dir.clone(),
        commands: picked.iter().map(|e| e.recorded()).collect(),
        shell,
    };
    review(ui, store, recording, &[]).map(|_| ())
}

/// Habits in the history that are worth offering here: commands typed under this
/// project (anywhere, outside a project) that no workflow covers yet.
pub fn habits_here(store: &Store) -> Vec<Habit> {
    let entries: Vec<history::Entry> = history::read(&store.dirs().history_log())
        .into_iter()
        .filter(|e| e.recorded().succeeded() && session::worth_recording(&e.command))
        .collect();
    let root = store::find_save_root(store.cwd());
    let here: Vec<history::Entry> = entries
        .into_iter()
        .filter(|e| root.as_ref().is_none_or(|r| e.dir.starts_with(r)))
        .collect();
    let mut runs = history::sessions(&here);
    runs.sort_by_key(|run| run[0].at);
    let runs: Vec<Vec<&str>> = runs
        .iter()
        .map(|run| run.iter().map(|e| e.command.as_str()).collect())
        .collect();
    let known: Vec<Vec<String>> = store
        .list()
        .ranked
        .iter()
        .filter_map(|e| store::load(&e.path).ok())
        .map(|w| w.steps)
        .collect();
    suggest::habits(&runs, &known)
}

/// `fr suggest`: show repeated command sequences and save one as a workflow.
pub fn suggest(ui: &mut dyn Ui, store: &Store, shell: Dialect) -> Result<()> {
    let _ = history::compact(&store.dirs().history_log());
    let habits = habits_here(store);
    let pal = ui.pal();
    if habits.is_empty() {
        ui.say(&format!(
            "\n  No repeated sequences yet. {}\n",
            pal.dim("fr notices commands you type in the same order three or more times.")
        ));
        return Ok(());
    }
    ui.say(&format!("\n  {}\n", pal.bold("You keep typing these")));
    for (i, habit) in habits.iter().enumerate() {
        ui.say(&format!(
            "    {}  {}",
            pal.cyan(&(i + 1).to_string()),
            pal.dim(&format!("{}×", habit.times))
        ));
        for command in &habit.commands {
            ui.say(&format!("       {}", first_line(command)));
        }
    }
    ui.say("");
    let answer = ui.ask("Save which?", &format!("1-{} · Enter = none", habits.len()))?;
    let Some(habit) = answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| habits.get(i))
    else {
        return match answer.trim() {
            "" => Ok(()),
            other => Err(Error::invalid(format!(
                "`{other}` is not one of the numbers"
            ))),
        };
    };
    let recording = Recording {
        cwd: store.cwd().to_path_buf(),
        commands: habit
            .commands
            .iter()
            .map(|c| Recorded {
                status: 0,
                text: c.clone(),
            })
            .collect(),
        shell,
    };
    review(ui, store, recording, &habit.varying).map(|_| ())
}

/// Walk the user through saving `recording`. `hints` are values known to change between
/// runs; they are offered first as arguments. Returns `None` when there was nothing to save.
pub fn review(
    ui: &mut dyn Ui,
    store: &Store,
    recording: Recording,
    hints: &[String],
) -> Result<Option<Saved>> {
    if recording.commands.is_empty() {
        ui.say("  Nothing was recorded.");
        return Ok(None);
    }
    show_recorded(ui, &recording);

    let Recording {
        cwd,
        commands,
        shell,
    } = recording;
    let mut steps: Vec<String> = commands
        .into_iter()
        .filter(Recorded::succeeded)
        .map(|c| c.text)
        .collect();
    if steps.is_empty() {
        ui.say("\n  Every command failed — nothing to save.");
        return Ok(None);
    }

    let params = choose_dynamic(ui, &mut steps, hints)?;
    ui.say("");
    let (name, scope, dir) = choose_destination(ui, store, &cwd, &suggest::suggest_name(&steps))?;

    let mut workflow = Workflow::new(describe(&steps), steps, params);
    workflow.shell = shell;
    let path = store::write(&dir, &name, &workflow)?;
    if scope == Scope::Project {
        store.trust(&path)?; // the user just made it; no need to approve it again
    }

    let pal = ui.pal();
    ui.say(&format!(
        "\n  {} Saved {} {}",
        pal.green("✓"),
        pal.bold(&name),
        pal.dim(&format!("({scope})"))
    ));
    ui.say(&format!("    {}\n", pal.dim(&tilde(&path))));
    ui.say(&format!(
        "    {}  {}",
        pal.dim("Run "),
        workflow.usage(&name)
    ));
    ui.say(&format!("    {}  fr edit {name}\n", pal.dim("Edit")));
    Ok(Some(Saved {
        name,
        scope,
        path,
        params: workflow.params.into_iter().map(|p| p.name).collect(),
    }))
}

fn show_recorded(ui: &mut dyn Ui, recording: &Recording) {
    let pal = ui.pal();
    let n = recording.commands.len();
    ui.say(&format!(
        "\n  {}\n",
        pal.bold(&format!(
            "Recorded {n} command{}",
            if n == 1 { "" } else { "s" }
        ))
    ));
    let mut kept = 0;
    for command in &recording.commands {
        let line = first_line(&command.text);
        if command.succeeded() {
            kept += 1;
            ui.say(&format!("    {}  {line}", pal.dim(&format!("{kept:>2}"))));
        } else {
            let why = pal.red(&format!("exit {} · skipped", command.status));
            ui.say(&format!("    {}  {}  {why}", pal.red(" ✗"), pal.dim(&line)));
        }
    }
}

/// Offer repeated values as parameters. Rewrites `steps` in place and returns the
/// parameters in the order the user picked them (which is their argument order).
fn choose_dynamic(ui: &mut dyn Ui, steps: &mut [String], hints: &[String]) -> Result<Vec<Param>> {
    let mut candidates: Vec<Candidate> = hints
        .iter()
        .filter_map(|h| {
            let used = steps
                .iter()
                .filter(|s| render::find_words(s, h).next().is_some())
                .count();
            (used > 0).then(|| Candidate {
                value: h.clone(),
                steps: used,
            })
        })
        .collect();
    let changed = candidates.len();
    for c in suggest::candidates(steps) {
        if !candidates.iter().any(|k| k.value == c.value) {
            candidates.push(c);
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let pal = ui.pal();
    ui.say(&format!("\n  {}", pal.bold("Values that might change")));
    ui.say(&format!(
        "  {}\n",
        pal.dim("Pick the ones to pass as arguments each time you run this. The order you pick is the argument order.")
    ));
    let items: Vec<Choice> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| Choice {
            title: c.value.clone(),
            detail: if i < changed {
                "changed every time you ran this".to_owned()
            } else {
                format!("used in {} steps", c.steps)
            },
        })
        .collect();
    let picked = ui.select_many("Make dynamic", &items)?;

    let mut params: Vec<Param> = Vec::with_capacity(picked.len());
    for (rank, &row) in picked.iter().enumerate() {
        let Candidate { value, .. } = &candidates[row];
        let default = if rank == 0 {
            "name".to_owned()
        } else {
            format!("arg{}", rank + 1)
        };
        // A single pick needs no question; several get a short naming prompt each.
        let name = if picked.len() == 1 {
            default
        } else {
            ask_param_name(ui, &default, value, &params)?
        };
        params.push(Param::new(name, Some(value.clone())));
    }

    let pairs: Vec<(&str, &str)> = picked
        .iter()
        .zip(&params)
        .map(|(&row, p)| (candidates[row].value.as_str(), p.name.as_str()))
        .collect();
    for step in steps.iter_mut() {
        *step = render::replace_words(step, &pairs);
    }
    Ok(params)
}

fn ask_param_name(ui: &mut dyn Ui, default: &str, value: &str, taken: &[Param]) -> Result<String> {
    let pal = ui.pal();
    loop {
        let answer = ui.ask(&format!("Name for \"{value}\""), &format!("({default})"))?;
        let name = non_empty(&answer).unwrap_or(default);
        if !render::is_ident(name) {
            ui.say(&format!("  {}", pal.yellow("letters, digits and _ only")));
        } else if taken.iter().any(|p| p.name == name) {
            ui.say(&format!(
                "  {}",
                pal.yellow(&format!("`{name}` is already used"))
            ));
        } else {
            return Ok(name.to_owned());
        }
    }
}

/// Ask for a name and scope. Project scope is the default inside a git repository or an
/// existing `.fr/`, global elsewhere; Enter accepts both defaults.
fn choose_destination(
    ui: &mut dyn Ui,
    store: &Store,
    start_dir: &Path,
    suggested: &str,
) -> Result<(String, Scope, PathBuf)> {
    let root = store::find_save_root(start_dir);
    let default_scope = if root.is_some() {
        Scope::Project
    } else {
        Scope::Global
    };
    loop {
        let name = ask_name(ui, suggested)?;
        let scope = ask_scope(ui, default_scope)?;
        let dir = store.workflows_dir(scope, Some(root.as_deref().unwrap_or(start_dir)));
        let path = dir.join(format!("{name}.toml"));
        if path.exists() && !ui.confirm(&format!("{} already exists. Overwrite?", tilde(&path)))? {
            continue;
        }
        return Ok((name, scope, dir));
    }
}

fn ask_name(ui: &mut dyn Ui, suggested: &str) -> Result<String> {
    let pal = ui.pal();
    loop {
        let answer = ui.ask("Name", &format!("({suggested})"))?;
        let name = non_empty(&answer).unwrap_or(suggested);
        match validate_name(name) {
            Ok(()) => return Ok(name.to_owned()),
            Err(e) => ui.say(&format!("  {}", pal.red(&e.to_string()))),
        }
    }
}

fn ask_scope(ui: &mut dyn Ui, default: Scope) -> Result<Scope> {
    let pal = ui.pal();
    let hint = format!("(p)roject = this repo · (g)lobal = everywhere  [{default}]");
    loop {
        match ui
            .ask("Save to", &hint)?
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" => return Ok(default),
            "p" | "project" => return Ok(Scope::Project),
            "g" | "global" => return Ok(Scope::Global),
            _ => ui.say(&format!("  {}", pal.yellow("type p or g"))),
        }
    }
}

fn non_empty(answer: &str) -> Option<&str> {
    Some(answer.trim()).filter(|a| !a.is_empty())
}

fn describe(steps: &[String]) -> String {
    let head: String = first_line(&steps[0])
        .chars()
        .take(DESCRIPTION_LEN)
        .collect();
    match steps.len() {
        1 => head,
        n => format!("{head} (+{} more)", n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::Fixture;
    use crate::ui::testing::{pick, text, Scripted};
    use crate::Error;
    use std::fs;

    fn recording(cwd: PathBuf, commands: &[(i32, &str)]) -> Recording {
        Recording {
            cwd,
            shell: Dialect::Posix,
            commands: commands
                .iter()
                .map(|&(status, t)| Recorded {
                    status,
                    text: t.to_owned(),
                })
                .collect(),
        }
    }

    fn files_under(dir: &Path) -> usize {
        fs::read_dir(dir).map_or(0, |rd| {
            rd.flatten()
                .map(|e| {
                    if e.path().is_dir() {
                        files_under(&e.path())
                    } else {
                        1
                    }
                })
                .sum()
        })
    }

    #[test]
    fn the_nextjs_flow_saves_a_parameterized_global_workflow_without_the_failed_command() {
        let fx = Fixture::new();
        let store = fx.store_in("work");
        let rec = recording(
            fx.path("work"),
            &[
                (0, "pnpm dlx create-next-app dashboard"),
                (0, "cd dashboard"),
                (1, "ls nope"),
                (0, "pnpm add zustand"),
            ],
        );
        let mut ui = Scripted::new([pick(&[0]), text(""), text("")]);

        let saved = review(&mut ui, &store, rec, &[]).unwrap().unwrap();

        assert_eq!(
            (saved.name.as_str(), saved.scope),
            ("next", Scope::Global),
            "no repo here, so global"
        );
        let wf = store::load(&saved.path).unwrap();
        assert_eq!(
            wf.steps,
            [
                "pnpm dlx create-next-app {{name}}",
                "cd {{name}}",
                "pnpm add zustand"
            ]
        );
        assert_eq!(wf.params, [Param::new("name", Some("dashboard".into()))]);
        assert!(ui.transcript().contains("exit 1 · skipped"));
        assert!(ui.exhausted());
    }

    #[test]
    fn inside_a_git_repo_the_default_scope_is_project_and_the_workflow_is_trusted() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.path("repo/.git")).unwrap();
        let store = fx.store_in("repo");
        let mut ui = Scripted::new([text(""), text("")]);

        let saved = review(
            &mut ui,
            &store,
            recording(fx.path("repo"), &[(0, "cargo test")]),
            &[],
        )
        .unwrap()
        .unwrap();

        assert_eq!(saved.scope, Scope::Project);
        assert_eq!(saved.path, fx.path("repo/.fr/workflows/cargo-test.toml"));
        assert!(
            store.is_trusted(&saved.path),
            "no approval prompt for what you just recorded"
        );
    }

    #[test]
    fn scope_can_be_overridden_either_way() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.path("repo/.git")).unwrap();
        let store = fx.store_in("repo");
        let mut ui = Scripted::new([text("mine"), text("global")]);
        let saved = review(
            &mut ui,
            &store,
            recording(fx.path("repo"), &[(0, "make")]),
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(saved.scope, Scope::Global);
        assert_eq!(saved.path, fx.path("config/workflows/mine.toml"));

        let plain = fx.store_in("plain");
        let mut ui = Scripted::new([text("local"), text("p")]);
        let saved = review(
            &mut ui,
            &plain,
            recording(fx.path("plain"), &[(0, "make")]),
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            saved.path,
            fx.path("plain/.fr/workflows/local.toml"),
            "project scope without a repo creates .fr in the start dir"
        );
    }

    #[test]
    fn several_picks_are_named_in_pick_order() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let rec = recording(
            fx.path("w"),
            &[
                (0, "mkdir demo"),
                (0, "cd demo"),
                (0, "serve demo 3000"),
                (0, "open localhost:3000"),
            ],
        );
        // candidates are [demo (3 steps), 3000 (2 steps)]; pick 3000 first, then demo
        let mut ui = Scripted::new([
            pick(&[1, 0]),
            text("port"),
            text(""),
            text("app"),
            text("g"),
        ]);

        let saved = review(&mut ui, &store, rec, &[]).unwrap().unwrap();

        assert_eq!(saved.params, ["port", "arg2"]);
        let wf = store::load(&saved.path).unwrap();
        assert_eq!(
            wf.steps,
            [
                "mkdir {{arg2}}",
                "cd {{arg2}}",
                "serve {{arg2}} {{port}}",
                "open localhost:{{port}}"
            ]
        );
        assert_eq!(wf.usage("app"), "fr app <port> <arg2>");
    }

    #[test]
    fn declining_every_suggestion_keeps_the_steps_literal() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let rec = recording(fx.path("w"), &[(0, "mkdir demo"), (0, "cd demo")]);
        let mut ui = Scripted::new([pick(&[]), text("keep"), text("")]);
        let saved = review(&mut ui, &store, rec, &[]).unwrap().unwrap();
        let wf = store::load(&saved.path).unwrap();
        assert_eq!(wf.steps, ["mkdir demo", "cd demo"]);
        assert!(wf.params.is_empty());
    }

    #[test]
    fn a_duplicate_or_invalid_parameter_name_is_asked_again() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let rec = recording(
            fx.path("w"),
            &[
                (0, "mkdir demo"),
                (0, "cd demo"),
                (0, "serve demo 3000"),
                (0, "open :3000"),
            ],
        );
        let mut ui = Scripted::new([
            pick(&[0, 1]),
            text("x"),
            text("x"),
            text("bad name"),
            text("y"),
            text("n"),
            text(""),
        ]);
        let saved = review(&mut ui, &store, rec, &[]).unwrap().unwrap();
        assert_eq!(saved.params, ["x", "y"]);
        assert!(
            ui.transcript().contains("`x` is already used")
                && ui.transcript().contains("letters, digits and _ only")
        );
    }

    #[test]
    fn invalid_names_are_rejected_and_asked_again() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let mut ui = Scripted::new([text("list"), text("Bad Name"), text("good"), text("")]);
        let saved = review(
            &mut ui,
            &store,
            recording(fx.path("w"), &[(0, "make")]),
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(saved.name, "good");
        assert!(ui.transcript().contains("built-in command"));
    }

    #[test]
    fn declining_to_overwrite_returns_to_the_name_prompt_and_keeps_the_original() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let original = fx.global("a");
        let before = fs::read_to_string(&original).unwrap();
        let mut ui = Scripted::new([text("a"), text(""), text("n"), text("b"), text("")]);

        let saved = review(
            &mut ui,
            &store,
            recording(fx.path("w"), &[(0, "make")]),
            &[],
        )
        .unwrap()
        .unwrap();

        assert_eq!(saved.name, "b");
        assert_eq!(fs::read_to_string(&original).unwrap(), before);
    }

    #[test]
    fn closing_input_at_any_prompt_cancels_without_writing_anything() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let rec = || recording(fx.path("w"), &[(0, "mkdir demo"), (0, "cd demo")]);
        for answers in [vec![], vec![pick(&[])], vec![pick(&[]), text("x")]] {
            let mut ui = Scripted::new(answers);
            assert!(matches!(
                review(&mut ui, &store, rec(), &[]),
                Err(Error::Cancelled)
            ));
        }
        assert_eq!(files_under(&fx.path("config")), 0);
    }

    #[test]
    fn empty_and_all_failed_recordings_ask_nothing() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        for rec in [
            recording(fx.path("w"), &[]),
            recording(fx.path("w"), &[(1, "false"), (127, "nope")]),
        ] {
            let mut ui = Scripted::new([]);
            assert_eq!(review(&mut ui, &store, rec, &[]).unwrap(), None);
            assert!(ui.asked.is_empty());
        }
    }

    // ---- after the fact ------------------------------------------------------------------

    fn write_history(fx: &Fixture, lines: &[(u32, i32, &str)]) {
        std::fs::create_dir_all(fx.path("state")).unwrap();
        let dir = fx.path("w");
        let text: String = lines
            .iter()
            .enumerate()
            .map(|(i, (pid, status, cmd))| {
                format!("{i}\t{pid}\t{status}\t{}\t{cmd}\n", dir.display())
            })
            .collect();
        std::fs::write(fx.path("state/history"), text).unwrap();
    }

    #[test]
    fn save_with_a_count_takes_this_shells_last_successful_commands() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        write_history(
            &fx,
            &[
                (7, 0, "mkdir demo"),
                (8, 0, "echo other shell"),
                (7, 0, "cd demo"),
                (7, 1, "npm tset"),
                (7, 0, "fr list"),
                (7, 0, "npm test"),
            ],
        );
        let mut ui = Scripted::new([text("t"), text("")]);
        save(&mut ui, &store, 7, Some(2), Dialect::Posix).unwrap();
        let wf = store::load(&fx.path("config/workflows/t.toml")).unwrap();
        assert_eq!(
            wf.steps,
            ["cd demo", "npm test"],
            "failed and fr commands are skipped"
        );
    }

    #[test]
    fn save_without_a_count_offers_recent_commands_and_keeps_their_order() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        write_history(&fx, &[(7, 0, "a1"), (7, 0, "b2"), (7, 0, "c3")]);
        let mut ui = Scripted::new([pick(&[2, 0]), text("abc"), text("")]);
        save(&mut ui, &store, 7, None, Dialect::Posix).unwrap();
        let wf = store::load(&fx.path("config/workflows/abc.toml")).unwrap();
        assert_eq!(
            wf.steps,
            ["a1", "c3"],
            "picked out of order, saved in run order"
        );
    }

    #[test]
    fn suggest_offers_a_habit_with_the_changing_value_preselectable() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let mut lines = Vec::new();
        for (pid, commit) in [
            (1, "git commit -m one"),
            (2, "git commit -m two"),
            (3, "git commit -m three"),
        ] {
            lines.extend([
                (pid, 0, "git add -A"),
                (pid, 0, commit),
                (pid, 0, "git push"),
            ]);
        }
        write_history(&fx, &lines);
        let mut ui = Scripted::new([text("1"), pick(&[0]), text("ship"), text("")]);
        suggest(&mut ui, &store, Dialect::Posix).unwrap();
        assert!(ui.transcript().contains("3×"));
        let wf = store::load(&fx.path("config/workflows/ship.toml")).unwrap();
        assert_eq!(
            wf.steps,
            ["git add -A", "git commit -m {{name}}", "git push"]
        );
        assert!(habits_here(&store).is_empty(), "now covered by a workflow");
    }

    #[test]
    fn finish_reads_the_log_and_always_removes_it() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        let log = crate::session::create(&fx.path("state/sessions"), &fx.path("w"), Dialect::Posix)
            .unwrap();
        crate::session::append(&log, 0, "make build").unwrap();

        finish(&mut Scripted::new([text("build"), text("")]), &store, &log).unwrap();
        assert!(fx.path("config/workflows/build.toml").exists());
        assert!(!log.exists());

        let log = crate::session::create(&fx.path("state/sessions"), &fx.path("w"), Dialect::Posix)
            .unwrap();
        crate::session::append(&log, 0, "make again").unwrap();
        assert!(finish(&mut Scripted::new([]), &store, &log).is_err());
        assert!(!log.exists(), "removed even when the user cancels");
    }
}
