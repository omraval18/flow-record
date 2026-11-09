//! Replaying a workflow: resolve the name, bind arguments, show a preview, ask for the
//! confirmations that apply, and emit one script for the shell function to `eval`.
//!
//! The script goes to `out` (stdout); everything for the human goes through the [`Ui`]
//! (stderr), so `$(fr _run …)` captures only the script.

use crate::destructive::is_destructive;
use crate::store::{self, Entry, Resolution, Store};
use crate::ui::{tilde, Ui};
use crate::workflow::{Param, Scope, Workflow};
use crate::{render, Error, Result};
use std::collections::HashMap;
use std::io::Write;

const USAGE: &str = "usage: fr [-n] [-y] [-g|-p] <workflow> [args…]";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Preview only; nothing is run and no argument is prompted for.
    pub dry_run: bool,
    /// Skip the destructive-step confirmation (never the untrusted-workflow one).
    pub yes: bool,
    pub scope: Option<Scope>,
    pub name: String,
    pub args: Vec<String>,
}

impl Options {
    /// Flags come first; everything after the workflow name is a positional argument,
    /// even if it starts with `-`.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self::default();
        let mut rest = args;
        while let Some((flag, tail)) = rest.split_first() {
            let scope = match flag.as_str() {
                "-n" | "--dry-run" => {
                    options.dry_run = true;
                    None
                }
                "-y" | "--yes" => {
                    options.yes = true;
                    None
                }
                "-g" | "--global" => Some(Scope::Global),
                "-p" | "--project" => Some(Scope::Project),
                f if f.len() > 1 && f.starts_with('-') => {
                    return Err(Error::usage(format!("unknown flag `{f}`\n{USAGE}")))
                }
                _ => break,
            };
            if let Some(scope) = scope {
                if options.scope.is_some_and(|s| s != scope) {
                    return Err(Error::usage(
                        "--global and --project are mutually exclusive",
                    ));
                }
                options.scope = Some(scope);
            }
            rest = tail;
        }
        let (name, args) = rest.split_first().ok_or_else(|| Error::usage(USAGE))?;
        options.name.clone_from(name);
        options.args = args.to_vec();
        Ok(options)
    }
}

/// A resolved workflow and how it was found.
#[derive(Debug)]
pub struct Located {
    pub entry: Entry,
    pub by_prefix: bool,
}

/// Resolve `input` (exact name, unique prefix, or a pick from the ranked candidates).
pub fn locate(store: &Store, ui: &mut dyn Ui, input: &str, only: Option<Scope>) -> Result<Located> {
    match store.resolve(input, only) {
        Resolution::Exact(entry) => Ok(Located {
            entry,
            by_prefix: false,
        }),
        Resolution::Prefix(entry) => Ok(Located {
            entry,
            by_prefix: true,
        }),
        Resolution::Ambiguous(candidates) => Ok(Located {
            entry: pick_one(ui, candidates, input)?,
            by_prefix: false,
        }),
        Resolution::Missing { similar } => Err(Error::NotFound {
            name: input.to_owned(),
            similar,
        }),
    }
}

fn pick_one(ui: &mut dyn Ui, mut candidates: Vec<Entry>, input: &str) -> Result<Entry> {
    let pal = ui.pal();
    let width = candidates.iter().map(|e| e.name.len()).max().unwrap_or(0);
    ui.say(&format!(
        "\n  {}\n",
        pal.bold(&format!("`{input}` matches {} workflows", candidates.len()))
    ));
    for (i, e) in candidates.iter().enumerate() {
        let (num, scope) = (
            pal.cyan(&(i + 1).to_string()),
            pal.dim(&format!("{:<7}", e.scope.label())),
        );
        ui.say(&format!(
            "    {num}  {:<width$}  {scope}  {}",
            e.name,
            pal.dim(&e.description)
        ));
    }
    ui.say("");
    let answer = ui.ask("Which one?", "[1]")?;
    let choice = match answer.trim() {
        "" => 1,
        n => n
            .parse::<usize>()
            .map_err(|_| Error::invalid(format!("`{n}` is not a number")))?,
    };
    match choice.checked_sub(1).filter(|&i| i < candidates.len()) {
        Some(i) => Ok(candidates.swap_remove(i)),
        None => Err(Error::invalid(format!(
            "choose a number from 1 to {}",
            candidates.len()
        ))),
    }
}

/// Argument values by parameter name; `None` marks a value still unknown (dry run only).
pub type Bound = HashMap<String, Option<String>>;

/// Match positional `args` to the workflow's parameters. For a required parameter with
/// no argument, `missing` decides: prompt (`Some`) or leave it pending (`None`).
pub fn bind(
    workflow: &Workflow,
    name: &str,
    args: &[String],
    mut missing: impl FnMut(&Param) -> Result<Option<String>>,
) -> Result<Bound> {
    if args.len() > workflow.params.len() {
        return Err(Error::usage(format!(
            "`{name}` takes {} argument(s), got {} — {}",
            workflow.params.len(),
            args.len(),
            workflow.usage(name)
        )));
    }
    let mut bound = Bound::with_capacity(workflow.params.len());
    for (i, param) in workflow.params.iter().enumerate() {
        let value = match (args.get(i), &param.default) {
            (Some(arg), _) => Some(arg.clone()),
            (None, Some(default)) => Some(default.clone()),
            (None, None) => missing(param)?,
        };
        bound.insert(param.name.clone(), value);
    }
    Ok(bound)
}

/// Render every step. Pending values appear as `<name>` for previews.
fn render_steps(workflow: &Workflow, bound: &Bound) -> Vec<String> {
    workflow
        .steps
        .iter()
        .map(|step| {
            render::substitute(step, |name, ctx| {
                Some(match bound.get(name)? {
                    Some(value) => render::quote(value, ctx).into_owned(),
                    None => format!("<{name}>"),
                })
            })
        })
        .collect()
}

fn ask_required(ui: &mut dyn Ui, param: &Param) -> Result<String> {
    let hint = param
        .example
        .as_ref()
        .map(|e| format!("e.g. {e}"))
        .unwrap_or_default();
    loop {
        let answer = ui.ask(&param.name, &hint)?;
        if !answer.is_empty() {
            return Ok(answer);
        }
    }
}

fn preview(ui: &mut dyn Ui, entry: &Entry, by_prefix: bool, steps: &[String]) {
    let pal = ui.pal();
    let how = if by_prefix {
        pal.dim("  · matched by prefix")
    } else {
        String::new()
    };
    ui.say(&format!(
        "\n  {} {}  {}{how}",
        pal.green("▶"),
        pal.bold(&entry.name),
        pal.dim(entry.scope.label())
    ));
    for (i, step) in steps.iter().enumerate() {
        for (line_no, line) in step.lines().enumerate() {
            let num = if line_no == 0 {
                format!("{:>2}", i + 1)
            } else {
                "  ".to_owned()
            };
            ui.say(&format!("    {}  {line}", pal.dim(&num)));
        }
    }
    ui.say("");
}

/// Run a workflow: write its script to `out` once everything has been confirmed.
pub fn run(store: &Store, ui: &mut dyn Ui, options: &Options, out: &mut dyn Write) -> Result<()> {
    let Located { entry, by_prefix } = locate(store, ui, &options.name, options.scope)?;
    let workflow = store::load(&entry.path)?;
    workflow
        .validate()
        .map_err(|e| Error::invalid(format!("{e} — try `fr edit {}`", entry.name)))?;

    let destructive = workflow.steps.iter().any(|s| is_destructive(s));
    if by_prefix && destructive {
        return Err(Error::invalid(format!(
            "`{}` is destructive — type its full name: fr {}",
            options.name, entry.name
        )));
    }

    let bound = bind(&workflow, &entry.name, &options.args, |param| {
        if options.dry_run {
            Ok(None)
        } else {
            ask_required(ui, param).map(Some)
        }
    })?;
    let steps = render_steps(&workflow, &bound);
    preview(ui, &entry, by_prefix, &steps);
    if options.dry_run {
        return Ok(());
    }

    let pal = ui.pal();
    if entry.scope == Scope::Project && !store.is_trusted(&entry.path) {
        ui.say(&format!(
            "  {} New or changed project workflow",
            pal.yellow("!")
        ));
        ui.say(&format!("    {}", pal.dim(&tilde(&entry.path))));
        if !ui.confirm("Run it?")? {
            return Err(Error::Cancelled);
        }
        store.trust(&entry.path)?;
    }
    if destructive && !options.yes {
        ui.say(&format!("  {} Destructive steps detected", pal.yellow("!")));
        if ui
            .ask(&format!("Type \"{}\" to continue", entry.name), "")?
            .trim()
            != entry.name
        {
            return Err(Error::Cancelled);
        }
    }

    // Ranking is a convenience; an unwritable state directory must not block the run.
    let _ = store.record_use(&entry.name);
    out.write_all(render::chain(&steps).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::Fixture;
    use crate::ui::testing::{text, Scripted};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn workflow(steps: &[&str], params: &[(&str, Option<&str>)]) -> Workflow {
        Workflow::new(
            String::new(),
            strings(steps),
            params
                .iter()
                .map(|&(name, default)| Param {
                    name: name.into(),
                    default: default.map(Into::into),
                    example: None,
                })
                .collect(),
        )
    }

    fn run_script(store: &Store, ui: &mut Scripted, flags: &[&str]) -> Result<String> {
        let mut out = Vec::new();
        run(store, ui, &Options::parse(&strings(flags))?, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    // ---- option parsing ---------------------------------------------------------------

    #[test]
    fn flags_precede_the_name_and_later_arguments_are_positional() {
        let o = Options::parse(&strings(&["-n", "--global", "deploy", "-x", "--force"])).unwrap();
        assert_eq!(
            o,
            Options {
                dry_run: true,
                yes: false,
                scope: Some(Scope::Global),
                name: "deploy".into(),
                args: strings(&["-x", "--force"])
            }
        );
    }

    #[test]
    fn bad_option_sets_are_rejected() {
        assert!(Options::parse(&[]).is_err(), "no name");
        assert!(
            Options::parse(&strings(&["-n"])).is_err(),
            "flags but no name"
        );
        assert!(Options::parse(&strings(&["--bogus", "x"]))
            .unwrap_err()
            .to_string()
            .contains("--bogus"));
        assert!(
            Options::parse(&strings(&["-g", "-p", "x"])).is_err(),
            "conflicting scopes"
        );
        assert!(
            Options::parse(&strings(&["-g", "-g", "x"])).is_ok(),
            "repeating a flag is harmless"
        );
    }

    // ---- binding ----------------------------------------------------------------------

    #[test]
    fn bind_uses_arguments_then_defaults_and_asks_only_for_the_rest() {
        let wf = workflow(&["x"], &[("a", None), ("b", Some("dflt")), ("c", None)]);
        let mut asked = Vec::new();
        let bound = bind(&wf, "w", &strings(&["one"]), |p| {
            asked.push(p.name.clone());
            Ok(Some(format!("asked-{}", p.name)))
        })
        .unwrap();

        assert_eq!(bound["a"].as_deref(), Some("one"));
        assert_eq!(bound["b"].as_deref(), Some("dflt"));
        assert_eq!(bound["c"].as_deref(), Some("asked-c"));
        assert_eq!(asked, ["c"]);
    }

    #[test]
    fn an_explicit_argument_overrides_the_default() {
        let wf = workflow(&["x"], &[("a", Some("dflt"))]);
        let bound = bind(&wf, "w", &strings(&["given"]), |_| unreachable!()).unwrap();
        assert_eq!(bound["a"].as_deref(), Some("given"));
    }

    #[test]
    fn too_many_arguments_report_the_usage() {
        let wf = workflow(&["x"], &[("a", None)]);
        let err = bind(&wf, "w", &strings(&["1", "2"]), |_| Ok(None))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("takes 1 argument(s), got 2") && err.contains("fr w <a>"),
            "{err}"
        );
    }

    #[test]
    fn pending_values_preview_as_angle_brackets_without_quoting() {
        let wf = workflow(&["mkdir {{name}}", "echo \"{{name}}\""], &[("name", None)]);
        let bound = bind(&wf, "w", &[], |_| Ok(None)).unwrap();
        assert_eq!(
            render_steps(&wf, &bound),
            ["mkdir <name>", "echo \"<name>\""]
        );
    }

    // ---- the full run -----------------------------------------------------------------

    fn global_workflow(fx: &Fixture, name: &str, steps: &[&str], params: &[(&str, Option<&str>)]) {
        fx.put_workflow("config/workflows", name, &workflow(steps, params));
    }

    #[test]
    fn a_run_quotes_arguments_prints_a_preview_and_records_usage() {
        let fx = Fixture::new();
        global_workflow(
            &fx,
            "scaffold",
            &["mkdir {{name}}", "cd {{name}}"],
            &[("name", None)],
        );
        global_workflow(&fx, "other", &["true"], &[]);
        let store = fx.store_in("w");
        let mut ui = Scripted::new([]);

        let script = run_script(&store, &mut ui, &["scaffold", "my app"]).unwrap();

        assert!(
            script.contains("{\nmkdir 'my app'\n}") && script.contains("cd 'my app'"),
            "{script}"
        );
        assert!(
            ui.transcript().contains("▶ scaffold") && ui.transcript().contains("mkdir 'my app'")
        );
        assert_eq!(
            store.list().ranked[0].name,
            "scaffold",
            "usage moves it to the front"
        );
    }

    #[test]
    fn a_missing_required_argument_is_prompted_for_and_empty_answers_are_rejected() {
        let fx = Fixture::new();
        global_workflow(&fx, "go", &["echo {{who}}"], &[("who", None)]);
        let store = fx.store_in("w");
        let mut ui = Scripted::new([text(""), text("Om")]);

        let script = run_script(&store, &mut ui, &["go"]).unwrap();

        assert!(script.contains("echo Om"));
        assert_eq!(ui.asked, ["who", "who"]);
    }

    #[test]
    fn closing_input_while_prompting_cancels_without_a_script() {
        let fx = Fixture::new();
        global_workflow(&fx, "go", &["echo {{who}}"], &[("who", None)]);
        assert!(matches!(
            run_script(&fx.store_in("w"), &mut Scripted::new([]), &["go"]),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn dry_run_previews_pending_values_and_emits_nothing() {
        let fx = Fixture::new();
        global_workflow(&fx, "go", &["echo {{who}}"], &[("who", None)]);
        let mut ui = Scripted::new([]);

        let script = run_script(&fx.store_in("w"), &mut ui, &["-n", "go"]).unwrap();

        assert_eq!(script, "");
        assert!(ui.transcript().contains("echo <who>"));
        assert!(ui.asked.is_empty(), "a dry run never prompts");
    }

    #[test]
    fn a_unique_prefix_runs_and_says_so() {
        let fx = Fixture::new();
        global_workflow(&fx, "scaffold", &["true"], &[]);
        let mut ui = Scripted::new([]);
        assert!(!run_script(&fx.store_in("w"), &mut ui, &["scaf"])
            .unwrap()
            .is_empty());
        assert!(ui.transcript().contains("matched by prefix"));
    }

    #[test]
    fn an_ambiguous_prefix_opens_a_ranked_picker() {
        let fx = Fixture::new();
        global_workflow(&fx, "build", &["echo build"], &[]);
        global_workflow(&fx, "bench", &["echo bench"], &[]);
        let store = fx.store_in("w");

        let second = run_script(&store, &mut Scripted::new([text("2")]), &["b"]).unwrap();
        assert!(
            second.contains("echo build"),
            "ranked alphabetically: bench, build"
        );
        let default = run_script(&store, &mut Scripted::new([text("")]), &["b"]).unwrap();
        assert!(
            default.contains("echo build"),
            "build is now most recently used, so it is first and the default"
        );
        assert!(run_script(&store, &mut Scripted::new([text("9")]), &["b"]).is_err());
        assert!(run_script(&store, &mut Scripted::new([text("x")]), &["b"]).is_err());
    }

    #[test]
    fn an_unknown_name_suggests_near_matches() {
        let fx = Fixture::new();
        global_workflow(&fx, "deploy-prod", &["true"], &[]);
        let err = run_script(&fx.store_in("w"), &mut Scripted::new([]), &["prod"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("did you mean: deploy-prod"), "{err}");
    }

    #[test]
    fn destructive_workflows_refuse_prefix_matches_and_demand_the_full_name_typed() {
        let fx = Fixture::new();
        global_workflow(&fx, "nuke", &["rm -rf ./build"], &[]);
        let store = fx.store_in("w");

        let err = run_script(&store, &mut Scripted::new([]), &["nu"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("type its full name"), "{err}");

        assert!(matches!(
            run_script(&store, &mut Scripted::new([text("wrong")]), &["nuke"]),
            Err(Error::Cancelled)
        ));
        assert!(
            run_script(&store, &mut Scripted::new([text("nuke")]), &["nuke"])
                .unwrap()
                .contains("rm -rf ./build")
        );
        assert!(
            run_script(&store, &mut Scripted::new([]), &["-y", "nuke"]).is_ok(),
            "--yes skips the prompt"
        );
    }

    #[test]
    fn an_unapproved_project_workflow_needs_a_yes_once_and_again_after_it_changes() {
        let fx = Fixture::new();
        let file = fx.put_workflow("repo/.fr/workflows", "build", &workflow(&["echo hi"], &[]));
        let store = fx.store_in("repo");

        assert!(matches!(
            run_script(&store, &mut Scripted::new([text("n")]), &["build"]),
            Err(Error::Cancelled)
        ));
        assert!(!store.is_trusted(&file));

        assert!(run_script(&store, &mut Scripted::new([text("y")]), &["build"]).is_ok());
        assert!(
            run_script(&store, &mut Scripted::new([]), &["build"]).is_ok(),
            "approved: no prompt"
        );

        std::fs::write(&file, "steps = [\"echo changed\"]\n").unwrap();
        assert!(
            matches!(
                run_script(&store, &mut Scripted::new([]), &["build"]),
                Err(Error::Cancelled)
            ),
            "changed: asks again"
        );
    }

    #[test]
    fn yes_does_not_bypass_the_untrusted_workflow_prompt() {
        let fx = Fixture::new();
        fx.put_workflow("repo/.fr/workflows", "build", &workflow(&["echo hi"], &[]));
        let result = run_script(
            &fx.store_in("repo"),
            &mut Scripted::new([]),
            &["-y", "build"],
        );
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn a_workflow_with_an_undefined_placeholder_points_at_edit() {
        let fx = Fixture::new();
        std::fs::create_dir_all(fx.path("config/workflows")).unwrap();
        std::fs::write(
            fx.path("config/workflows/broken.toml"),
            "steps = [\"echo {{oops}}\"]\n",
        )
        .unwrap();
        let err = run_script(&fx.store_in("w"), &mut Scripted::new([]), &["broken"])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("{{oops}}") && err.contains("fr edit broken"),
            "{err}"
        );
    }
}
