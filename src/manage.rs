//! Commands that inspect or manage saved workflows: `list`, `show`, `export`, `rm`, plus
//! the data behind shell completion.

use crate::render::Dialect;
use crate::run::{locate, Located};
use crate::store::{self, Listing, Resolution, Store};
use crate::ui::{first_line, tilde, Pal, Ui};
use crate::workflow::{split_optional, validate_name, Param, Scope, Workflow};
use crate::{render, Error, Result};
use std::io::Write;
use std::path::Path;

/// `fr list`. `habits` is how many repeated command sequences `fr suggest` could offer.
pub fn list(store: &Store, pal: Pal, habits: usize, out: &mut dyn Write) -> Result<()> {
    let tip = |out: &mut dyn Write| -> Result<()> {
        if habits > 0 {
            let what = if habits == 1 { "sequence" } else { "sequences" };
            writeln!(
                out,
                "  {}\n",
                pal.dim(&format!(
                    "You've repeated {habits} command {what} here — `fr suggest` turns one into a workflow."
                ))
            )?;
        }
        Ok(())
    };
    let Listing { ranked, shadowed } = store.list();
    if ranked.is_empty() {
        writeln!(
            out,
            "\n  No workflows yet. Run {} to record your first one, or {} to save what you just ran.\n",
            pal.bold("fr"),
            pal.bold("fr save")
        )?;
        return tip(out);
    }
    let width = ranked
        .iter()
        .chain(&shadowed)
        .map(|e| e.name.len())
        .max()
        .unwrap_or(0)
        .max("NAME".len());
    writeln!(
        out,
        "\n  {}",
        pal.dim(&format!("{:<width$}  {:<7}  DESCRIPTION", "NAME", "SCOPE"))
    )?;
    for e in &ranked {
        let (name, scope) = (
            pal.bold(&format!("{:<width$}", e.name)),
            pal.cyan(&format!("{:<7}", e.scope.label())),
        );
        writeln!(out, "  {name}  {scope}  {}", pal.dim(&e.description))?;
    }
    for e in &shadowed {
        let note = format!(
            "{:<width$}  {:<7}  hidden by the project workflow of the same name (fr -g {})",
            e.name,
            e.scope.label(),
            e.name
        );
        writeln!(out, "  {}", pal.dim(&note))?;
    }
    writeln!(out)?;
    tip(out)
}

pub fn show(
    store: &Store,
    ui: &mut dyn Ui,
    pal: Pal,
    name: &str,
    out: &mut dyn Write,
) -> Result<()> {
    let Located { entry, .. } = locate(store, ui, name, None)?;
    let workflow = store::load(&entry.path)?;
    writeln!(
        out,
        "{}",
        card(pal, &entry.name, entry.scope, &entry.path, &workflow, false)
    )?;
    Ok(())
}

/// A workflow summary: header, numbered steps with placeholders highlighted, parameters.
pub fn card(
    pal: Pal,
    name: &str,
    scope: Scope,
    path: &Path,
    workflow: &Workflow,
    unsaved: bool,
) -> String {
    let flag = if unsaved {
        format!("  {}", pal.yellow("● unsaved changes"))
    } else {
        String::new()
    };
    let mut out = format!(
        "\n  {}  {}  {}{flag}\n",
        pal.bold(name),
        pal.cyan(scope.label()),
        pal.dim(&tilde(path))
    );
    if !workflow.description.is_empty() {
        out.push_str(&format!("  {}\n", pal.dim(&workflow.description)));
    }
    out.push_str(&format!("\n  {}\n", pal.bold("Steps")));
    for (i, step) in workflow.steps.iter().enumerate() {
        let (optional, command) = split_optional(step);
        let line = render::substitute_in(workflow.shell, &first_line(command), |n, _| {
            Some(pal.cyan(&format!("{{{{{n}}}}}")))
        });
        let tag = if optional {
            format!("  {}", pal.dim("may fail"))
        } else {
            String::new()
        };
        out.push_str(&format!(
            "    {}  {line}{tag}\n",
            pal.dim(&format!("{:>2}", i + 1))
        ));
    }
    out.push_str(&format!("\n  {}\n", pal.bold("Parameters")));
    if workflow.params.is_empty() {
        out.push_str(&format!(
            "    {}\n",
            pal.dim("none — every value is constant")
        ));
    }
    for p in &workflow.params {
        out.push_str(&format!("    {:<12} {}\n", p.name, pal.dim(&param_note(p))));
    }
    out
}

pub fn param_note(p: &Param) -> String {
    let mut note = match (&p.default, &p.example) {
        (Some(default), _) if p.computed_default().is_some() => {
            format!("optional · default: {default} (computed each run)")
        }
        (Some(default), _) => format!("optional · default: {default}"),
        (None, Some(example)) => format!("required · e.g. {example}"),
        (None, None) => "required".to_owned(),
    };
    if !p.choices.is_empty() {
        note.push_str(&format!(" · one of: {}", p.choices.join(", ")));
    }
    note
}

/// Print a standalone bash script equivalent to the workflow; parameters become
/// positional arguments.
pub fn export(store: &Store, ui: &mut dyn Ui, name: &str, out: &mut dyn Write) -> Result<()> {
    let Located { entry, .. } = locate(store, ui, name, None)?;
    let workflow = store::load(&entry.path)?;
    workflow.validate()?;
    if workflow.shell == Dialect::Fish {
        return Err(Error::invalid(format!(
            "`{}` is a fish workflow; export writes bash scripts",
            entry.name
        )));
    }

    writeln!(
        out,
        "#!/usr/bin/env bash\n# Exported by fr from `{}` ({}).\nset -euo pipefail",
        entry.name, entry.scope
    )?;
    let usage: String = workflow
        .params
        .iter()
        .map(|p| format!(" {}", p.name))
        .collect();
    for (i, p) in workflow.params.iter().enumerate() {
        let n = i + 1;
        match &p.default {
            // A computed default is shell code by design; it runs when the script does.
            Some(default) if p.computed_default().is_some() => {
                writeln!(out, "{}=${{{n}:-\"{default}\"}}", p.name)?;
            }
            Some(default) => writeln!(
                out,
                "{}=${{{n}:-{}}}",
                p.name,
                render::quote(default, render::Quote::Bare)
            )?,
            None => writeln!(out, "{}=${{{n}:?usage: $0{usage}}}", p.name)?,
        }
        if !p.choices.is_empty() {
            let pattern: Vec<String> = p
                .choices
                .iter()
                .map(|c| render::quote(c, render::Quote::Bare).into_owned())
                .collect();
            writeln!(
                out,
                "case \"${}\" in {}) ;; *) echo {} >&2; exit 2 ;; esac",
                p.name,
                pattern.join("|"),
                render::quote(
                    &format!("{} must be one of: {}", p.name, p.choices.join(", ")),
                    render::Quote::Bare
                )
            )?;
        }
    }
    for step in &workflow.steps {
        let (optional, command) = split_optional(step);
        let command = render::export_step(command);
        if optional {
            writeln!(
                out,
                "{{\n{command}\n}} || echo \"step failed with status $? (allowed to fail)\" >&2"
            )?;
        } else {
            writeln!(out, "{command}")?;
        }
    }
    Ok(())
}

/// `fr shim [name…]`: let workflows run without the `fr` prefix (`next` = `fr next`).
/// With no names, list the current shims.
pub fn shim(store: &Store, ui: &mut dyn Ui, names: &[String], out: &mut dyn Write) -> Result<()> {
    let mut shims = store.shims();
    if names.is_empty() {
        if shims.is_empty() {
            ui.say("  No shims. `fr shim <workflow>` lets you run it without typing fr.");
        }
        for name in &shims {
            writeln!(out, "{name}")?;
        }
        return Ok(());
    }
    let pal = ui.pal();
    for name in names {
        validate_name(name)?;
        if matches!(store.resolve(name, None), Resolution::Missing { .. }) {
            ui.say(&format!(
                "  {} no workflow called `{name}` here yet; the shim works wherever one exists",
                pal.yellow("!")
            ));
        }
        if let Some(found) = on_path(name) {
            ui.say(&format!(
                "  {} `{name}` now hides {} — run that with `command {name}`",
                pal.yellow("!"),
                tilde(&found)
            ));
        }
        if !shims.contains(name) {
            shims.push(name.clone());
        }
    }
    shims.sort_unstable();
    store.set_shims(&shims)?;
    ui.say(&format!(
        "  {} {} now run{} without `fr`",
        pal.green("✓"),
        names.join(", "),
        if names.len() == 1 { "s" } else { "" }
    ));
    Ok(())
}

pub fn unshim(store: &Store, ui: &mut dyn Ui, names: &[String]) -> Result<()> {
    let mut shims = store.shims();
    for name in names {
        let Some(at) = shims.iter().position(|s| s == name) else {
            return Err(Error::invalid(format!("`{name}` is not a shim")));
        };
        shims.remove(at);
    }
    store.set_shims(&shims)?;
    let pal = ui.pal();
    ui.say(&format!(
        "  {} Removed {}",
        pal.green("✓"),
        names.join(", ")
    ));
    Ok(())
}

/// The first executable called `name` on `$PATH`.
fn on_path(name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|p| {
                p.metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    })
}

pub fn remove(store: &Store, ui: &mut dyn Ui, name: &str) -> Result<()> {
    let Located { entry, .. } = locate(store, ui, name, None)?;
    if !ui.confirm(&format!(
        "Delete `{}` ({})?",
        entry.name,
        tilde(&entry.path)
    ))? {
        return Err(crate::Error::Cancelled);
    }
    store::delete(&entry.path)?;
    let pal = ui.pal();
    ui.say(&format!(
        "  {} Deleted {}",
        pal.green("✓"),
        pal.bold(&entry.name)
    ));
    Ok(())
}

/// Data for the zsh completion function (`fr _complete`).
///
/// * no arguments: one `name:description [scope]` line per workflow, best first;
/// * `<name> <index>`: a one-line hint for that positional argument, followed by one
///   line per allowed value when the parameter has choices.
pub fn complete(store: &Store, args: &[String], out: &mut dyn Write) -> Result<()> {
    match args {
        [] => {
            for e in store.list().ranked {
                writeln!(
                    out,
                    "{}:{} [{}]",
                    e.name,
                    e.description.replace('\n', " "),
                    e.scope
                )?;
            }
        }
        [name, rest @ ..] => {
            let index = rest
                .first()
                .and_then(|i| i.parse::<usize>().ok())
                .unwrap_or(0);
            let entry = match store.resolve(name, None) {
                Resolution::Exact(e) | Resolution::Prefix(e) => e,
                _ => return Ok(()),
            };
            if let Some(p) = store::load(&entry.path)
                .ok()
                .and_then(|w| w.params.into_iter().nth(index))
            {
                writeln!(out, "{}", argument_hint(&p))?;
                for choice in &p.choices {
                    writeln!(out, "{choice}")?;
                }
            }
        }
    }
    Ok(())
}

fn argument_hint(p: &Param) -> String {
    if !p.choices.is_empty() {
        return format!("<{}> one of: {}", p.name, p.choices.join(", "));
    }
    match (&p.default, &p.example) {
        (Some(default), _) => format!("[{}={default}]", p.name),
        (None, Some(example)) => format!("<{}> e.g. {example}", p.name),
        (None, None) => format!("<{}>", p.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::Fixture;
    use crate::ui::testing::{text, Scripted};
    use crate::Error;
    use std::process::Command;

    fn captured(f: impl FnOnce(&mut Vec<u8>) -> Result<()>) -> String {
        let mut out = Vec::new();
        f(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn wf(description: &str, steps: &[&str], params: Vec<Param>) -> Workflow {
        Workflow::new(
            description.into(),
            steps.iter().map(|s| (*s).to_owned()).collect(),
            params,
        )
    }

    #[test]
    fn list_shows_ranked_rows_and_explains_shadowed_workflows() {
        let fx = Fixture::new();
        fx.global("lint");
        fx.global("deploy");
        fx.project("repo", "lint");
        let out = captured(|o| list(&fx.store_in("repo"), Pal::plain(), 0, o));

        let rows: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert!(rows[0].starts_with("NAME") && rows[0].contains("SCOPE"));
        assert!(
            rows[1].starts_with("lint")
                && rows[1].contains("project")
                && rows[1].ends_with("project lint")
        );
        assert!(rows[2].starts_with("deploy") && rows[2].contains("global"));
        assert!(
            rows[3].starts_with("lint")
                && rows[3].contains("hidden by the project workflow")
                && rows[3].contains("fr -g lint")
        );
    }

    #[test]
    fn list_points_new_users_at_recording_and_at_their_habits() {
        let fx = Fixture::new();
        let out = captured(|o| list(&fx.store_in("w"), Pal::plain(), 2, o));
        assert!(
            out.contains("No workflows yet") && out.contains("fr save"),
            "{out}"
        );
        assert!(out.contains("repeated 2 command sequences"), "{out}");
    }

    #[test]
    fn the_card_lists_steps_and_how_each_parameter_behaves() {
        let params = vec![
            Param::new("a", Some("demo".into())),
            Param {
                name: "b".into(),
                default: Some("x".into()),
                ..Param::default()
            },
            Param::new("c", None),
        ];
        let card = card(
            Pal::plain(),
            "go",
            Scope::Global,
            Path::new("/p/go.toml"),
            &wf("desc", &["echo {{a}}\nmore"], params),
            true,
        );

        for expected in [
            "go  global  /p/go.toml",
            "● unsaved changes",
            "desc",
            " 1  echo {{a}} …",
            "required · e.g. demo",
            "optional · default: x",
        ] {
            assert!(card.contains(expected), "missing {expected:?} in\n{card}");
        }
        assert!(
            card.lines()
                .any(|l| l.trim() == format!("c{}required", " ".repeat(12))),
            "{card}"
        );
    }

    #[test]
    fn show_prints_the_card_for_a_prefix_match() {
        let fx = Fixture::new();
        fx.global("deploy");
        let out = captured(|o| {
            show(
                &fx.store_in("w"),
                &mut Scripted::new([]),
                Pal::plain(),
                "dep",
                o,
            )
        });
        assert!(out.contains("deploy") && out.contains("Steps"));
    }

    // ---- export, validated by running the script in a real shell --------------------------

    fn run_exported(fx: &Fixture, workflow: &Workflow, args: &[&str]) -> std::process::Output {
        fx.put_workflow("config/workflows", "demo", workflow);
        let script = captured(|o| export(&fx.store_in("w"), &mut Scripted::new([]), "demo", o));
        let (script_path, work) = (fx.path("demo.sh"), fx.path("work"));
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(&script_path, script).unwrap();
        Command::new("bash")
            .arg(&script_path)
            .args(args)
            .current_dir(work)
            .output()
            .unwrap()
    }

    #[test]
    fn an_exported_script_behaves_like_the_workflow_for_awkward_arguments() {
        let fx = Fixture::new();
        let w = wf(
            "",
            &[
                "mkdir {{dir}}",
                "cd {{dir}}",
                "touch {{dir}}.txt",
                "echo \"made {{dir}}\"",
            ],
            vec![Param::new("dir", None)],
        );

        let out = run_exported(&fx, &w, &["it's a 'dir'"]);

        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(fx.path("work/it's a 'dir'/it's a 'dir'.txt").exists());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "made it's a 'dir'\n");
    }

    #[test]
    fn an_exported_script_applies_defaults_and_demands_required_arguments() {
        let fx = Fixture::new();
        let mut greeting = Param::new("greeting", None);
        greeting.default = Some("hello there".into());
        let w = wf(
            "",
            &["echo {{greeting}} {{who}}"],
            vec![greeting, Param::new("who", None)],
        );

        let missing = run_exported(&fx, &w, &["ignored-because-first-is-greeting"]);
        assert!(!missing.status.success());
        assert!(
            String::from_utf8_lossy(&missing.stderr).contains("usage:"),
            "{}",
            String::from_utf8_lossy(&missing.stderr)
        );

        let ok = run_exported(&fx, &w, &["hi", "Om"]);
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "hi Om\n");
    }

    #[test]
    fn an_exported_script_checks_choices_computes_defaults_and_tolerates_optional_steps() {
        let fx = Fixture::new();
        let w = wf(
            "",
            &["-false", "echo {{env}} {{tag}}"],
            vec![
                Param {
                    name: "env".into(),
                    choices: vec!["dev".into(), "prod".into()],
                    ..Param::default()
                },
                Param {
                    name: "tag".into(),
                    default: Some("v$(echo 1)".into()),
                    ..Param::default()
                },
            ],
        );
        let ok = run_exported(&fx, &w, &["prod"]);
        assert_eq!(
            String::from_utf8_lossy(&ok.stdout),
            "prod v1\n",
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        assert!(String::from_utf8_lossy(&ok.stderr).contains("allowed to fail"));

        let bad = run_exported(&fx, &w, &["staging"]);
        assert_eq!(bad.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&bad.stderr).contains("env must be one of: dev, prod"));
    }

    #[test]
    fn shims_are_added_listed_and_removed() {
        let fx = Fixture::new();
        fx.global("next");
        let store = fx.store_in("w");
        let mut ui = Scripted::new([]);
        captured(|o| shim(&store, &mut ui, &["next".into()], o));
        assert_eq!(
            captured(|o| shim(&store, &mut Scripted::new([]), &[], o)),
            "next\n"
        );
        assert!(
            shim(&store, &mut ui, &["list".into()], &mut Vec::new()).is_err(),
            "reserved"
        );
        unshim(&store, &mut ui, &["next".into()]).unwrap();
        assert!(store.shims().is_empty());
        assert!(unshim(&store, &mut ui, &["next".into()]).is_err());
    }

    #[test]
    fn export_refuses_an_invalid_workflow() {
        let fx = Fixture::new();
        std::fs::create_dir_all(fx.path("config/workflows")).unwrap();
        std::fs::write(
            fx.path("config/workflows/bad.toml"),
            "steps = [\"echo {{nope}}\"]\n",
        )
        .unwrap();
        let mut out = Vec::new();
        assert!(export(&fx.store_in("w"), &mut Scripted::new([]), "bad", &mut out).is_err());
        assert!(out.is_empty(), "no partial script is written");
    }

    // ---- remove ----------------------------------------------------------------------------

    #[test]
    fn remove_deletes_only_after_confirmation() {
        let fx = Fixture::new();
        let file = fx.global("old");
        let store = fx.store_in("w");

        assert!(matches!(
            remove(&store, &mut Scripted::new([text("n")]), "old"),
            Err(Error::Cancelled)
        ));
        assert!(file.exists());

        remove(&store, &mut Scripted::new([text("y")]), "old").unwrap();
        assert!(!file.exists());
    }

    // ---- completion data -------------------------------------------------------------------

    #[test]
    fn completion_lists_workflows_best_first_with_scope_and_description() {
        let fx = Fixture::new();
        fx.global("deploy");
        fx.project("repo", "build");
        let out = captured(|o| complete(&fx.store_in("repo"), &[], o));
        assert_eq!(
            out,
            "build:project build [project]\ndeploy:global deploy [global]\n"
        );
    }

    #[test]
    fn completion_hints_the_argument_being_typed() {
        let fx = Fixture::new();
        let params = vec![
            Param::new("name", Some("demo".into())),
            Param {
                name: "port".into(),
                default: Some("3000".into()),
                ..Param::default()
            },
            Param {
                name: "env".into(),
                choices: vec!["dev".into(), "prod".into()],
                ..Param::default()
            },
        ];
        fx.put_workflow(
            "config/workflows",
            "serve",
            &wf("", &["echo {{name}} {{port}} {{env}}"], params),
        );
        let store = fx.store_in("w");
        let hint = |args: &[&str]| {
            captured(|o| {
                complete(
                    &store,
                    &args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>(),
                    o,
                )
            })
        };

        assert_eq!(hint(&["serve", "0"]), "<name> e.g. demo\n");
        assert_eq!(hint(&["ser", "1"]), "[port=3000]\n", "prefixes resolve too");
        assert_eq!(
            hint(&["serve", "2"]),
            "<env> one of: dev, prod\ndev\nprod\n"
        );
        assert_eq!(hint(&["serve", "3"]), "", "past the last parameter");
        assert_eq!(hint(&["nothing", "0"]), "");
    }
}
