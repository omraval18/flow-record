//! Commands that inspect or manage saved workflows: `list`, `show`, `export`, `rm`, plus
//! the data behind shell completion.

use crate::run::{locate, Located};
use crate::store::{self, Listing, Resolution, Store};
use crate::ui::{first_line, tilde, Pal, Ui};
use crate::workflow::{Param, Scope, Workflow};
use crate::{render, Result};
use std::io::Write;
use std::path::Path;

pub fn list(store: &Store, pal: Pal, out: &mut dyn Write) -> Result<()> {
    let Listing { ranked, shadowed } = store.list();
    if ranked.is_empty() {
        writeln!(
            out,
            "\n  No workflows yet. Run {} to record your first one.\n",
            pal.bold("fr")
        )?;
        return Ok(());
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
    Ok(())
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
        let line = render::substitute(&first_line(step), |n, _| {
            Some(pal.cyan(&format!("{{{{{n}}}}}")))
        });
        out.push_str(&format!(
            "    {}  {line}\n",
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
    match (&p.default, &p.example) {
        (Some(default), _) => format!("optional · default: {default}"),
        (None, Some(example)) => format!("required · e.g. {example}"),
        (None, None) => "required".to_owned(),
    }
}

/// Print a standalone bash script equivalent to the workflow; parameters become
/// positional arguments.
pub fn export(store: &Store, ui: &mut dyn Ui, name: &str, out: &mut dyn Write) -> Result<()> {
    let Located { entry, .. } = locate(store, ui, name, None)?;
    let workflow = store::load(&entry.path)?;
    workflow.validate()?;

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
            Some(default) => writeln!(
                out,
                "{}=${{{n}:-{}}}",
                p.name,
                render::quote(default, render::Quote::Bare)
            )?,
            None => writeln!(out, "{}=${{{n}:?usage: $0{usage}}}", p.name)?,
        }
    }
    for step in &workflow.steps {
        writeln!(out, "{}", render::export_step(step))?;
    }
    Ok(())
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
/// * `<name> <index>`: a one-line hint for that positional argument.
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
            }
        }
    }
    Ok(())
}

fn argument_hint(p: &Param) -> String {
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
        let out = captured(|o| list(&fx.store_in("repo"), Pal::plain(), o));

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
    fn list_points_new_users_at_recording() {
        let fx = Fixture::new();
        assert!(captured(|o| list(&fx.store_in("w"), Pal::plain(), o)).contains("No workflows yet"));
    }

    #[test]
    fn the_card_lists_steps_and_how_each_parameter_behaves() {
        let params = vec![
            Param::new("a", Some("demo".into())),
            Param {
                name: "b".into(),
                default: Some("x".into()),
                example: None,
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
                example: None,
            },
        ];
        fx.put_workflow(
            "config/workflows",
            "serve",
            &wf("", &["echo {{name}} {{port}}"], params),
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
        assert_eq!(hint(&["serve", "2"]), "", "past the last parameter");
        assert_eq!(hint(&["nothing", "0"]), "");
    }
}
