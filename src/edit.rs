//! `fr edit <name>`: change a saved workflow through the same choices as recording
//! (name, scope, parameters) plus steps. Changes accumulate in a [`Draft`] and nothing
//! reaches disk until the user saves. `fr edit -e <name>` opens the raw file instead.

use crate::manage::{card, param_note};
use crate::run::{locate, Located};
use crate::store::{self, Entry, Store};
use crate::ui::{first_line, tilde, Ui};
use crate::workflow::{set_optional, split_optional, validate_name, Param, Scope, Workflow};
use crate::{render, Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// An in-memory copy of a workflow being edited.
#[derive(Debug)]
struct Draft {
    name: String,
    scope: Scope,
    /// Project root when `scope` is [`Scope::Project`].
    root: Option<PathBuf>,
    /// Where the workflow currently lives on disk.
    path: PathBuf,
    workflow: Workflow,
    dirty: bool,
}

impl Draft {
    fn open(entry: Entry, workflow: Workflow) -> Self {
        // <root>/.fr/workflows/<name>.toml
        let root = (entry.scope == Scope::Project)
            .then(|| entry.path.ancestors().nth(3).map(Path::to_path_buf))
            .flatten();
        Self {
            name: entry.name,
            scope: entry.scope,
            root,
            path: entry.path,
            workflow,
            dirty: false,
        }
    }

    fn changed(&mut self) {
        self.dirty = true;
    }

    fn target_path(&self, store: &Store, scope: Scope, root: Option<&Path>, name: &str) -> PathBuf {
        store
            .workflows_dir(scope, root)
            .join(format!("{name}.toml"))
    }

    fn param_index(&self, name: &str) -> Result<usize> {
        self.workflow
            .params
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| Error::invalid(format!("no parameter called `{name}`")))
    }

    fn rewrite_steps(&mut self, mut f: impl FnMut(&str) -> String) {
        for step in &mut self.workflow.steps {
            *step = f(step);
        }
    }

    // ---- identity ----------------------------------------------------------------------

    fn rename(&mut self, store: &Store, new: &str) -> Result<()> {
        validate_name(new)?;
        if new != self.name
            && self
                .target_path(store, self.scope, self.root.as_deref(), new)
                .exists()
        {
            return Err(Error::invalid(format!(
                "`{new}` already exists in {}",
                self.scope
            )));
        }
        self.name = new.to_owned();
        self.changed();
        Ok(())
    }

    /// Move to the other scope. Moving to project scope needs `project_root`.
    fn move_scope(&mut self, store: &Store, project_root: Option<PathBuf>) -> Result<()> {
        let to = self.scope.other();
        let root = if to == Scope::Project {
            project_root
        } else {
            None
        };
        if self
            .target_path(store, to, root.as_deref(), &self.name)
            .exists()
        {
            return Err(Error::invalid(format!(
                "`{}` already exists in {to}",
                self.name
            )));
        }
        self.scope = to;
        self.root = root;
        self.changed();
        Ok(())
    }

    // ---- parameters --------------------------------------------------------------------

    fn next_param_name(&self) -> String {
        if self.workflow.params.is_empty() {
            "name".to_owned()
        } else {
            format!("arg{}", self.workflow.params.len() + 1)
        }
    }

    /// Turn the literal `text` (matched as a whole word) into a new parameter.
    fn add_param(&mut self, text: &str, name: Option<&str>) -> Result<()> {
        let name = name.map_or_else(|| self.next_param_name(), str::to_owned);
        if !render::is_ident(&name) {
            return Err(Error::invalid(format!(
                "`{name}` is not a valid parameter name (letters, digits and `_`)"
            )));
        }
        if self.workflow.param(&name).is_some() {
            return Err(Error::invalid(format!("`{name}` already exists")));
        }
        if self.workflow.param(text).is_some() {
            return Err(Error::invalid(format!(
                "`{text}` is a parameter name; pick different text"
            )));
        }
        if !self
            .workflow
            .steps
            .iter()
            .any(|s| render::find_words(s, text).next().is_some())
        {
            return Err(Error::invalid(format!(
                "`{text}` does not appear in any step as a whole word"
            )));
        }
        let pairs = [(text, name.as_str())];
        self.rewrite_steps(|s| render::replace_words(s, &pairs));
        self.workflow
            .params
            .push(Param::new(name, Some(text.to_owned())));
        self.changed();
        Ok(())
    }

    fn rename_param(&mut self, old: &str, new: &str) -> Result<()> {
        let at = self.param_index(old)?;
        if !render::is_ident(new) {
            return Err(Error::invalid(format!(
                "`{new}` is not a valid parameter name (letters, digits and `_`)"
            )));
        }
        if self.workflow.param(new).is_some() {
            return Err(Error::invalid(format!("`{new}` already exists")));
        }
        let dialect = self.workflow.shell;
        self.rewrite_steps(|s| {
            render::substitute_in(dialect, s, |n, _| {
                (n == old).then(|| format!("{{{{{new}}}}}"))
            })
        });
        self.workflow.params[at].name = new.to_owned();
        self.changed();
        Ok(())
    }

    fn set_default(&mut self, name: &str, default: Option<String>) -> Result<()> {
        let at = self.param_index(name)?;
        let p = &mut self.workflow.params[at];
        if let Some(value) = default
            .as_deref()
            .filter(|d| !crate::workflow::is_computed(d))
        {
            p.accepts(value)?;
        }
        p.default = default;
        self.changed();
        Ok(())
    }

    /// Restrict a parameter to `choices`, or lift the restriction when empty.
    fn set_choices(&mut self, name: &str, choices: Vec<String>) -> Result<()> {
        let at = self.param_index(name)?;
        let mut p = self.workflow.params[at].clone();
        p.choices = choices;
        if let Some(default) = p
            .default
            .as_deref()
            .filter(|d| !crate::workflow::is_computed(d))
        {
            if p.accepts(default).is_err() {
                return Err(Error::invalid(format!(
                    "the default `{default}` must be one of the choices — change or clear it first"
                )));
            }
        }
        self.workflow.params[at] = p;
        self.changed();
        Ok(())
    }

    /// Replace the parameter with the fixed text `value`, quoted for each position it
    /// appears in.
    fn make_constant(&mut self, name: &str, value: &str) -> Result<()> {
        let at = self.param_index(name)?;
        let dialect = self.workflow.shell;
        self.rewrite_steps(|s| {
            render::substitute_in(dialect, s, |n, ctx| {
                (n == name).then(|| render::quote_in(dialect, value, ctx).into_owned())
            })
        });
        self.workflow.params.remove(at);
        self.changed();
        Ok(())
    }

    // ---- steps -------------------------------------------------------------------------

    fn remove_step(&mut self, at: usize) -> Result<()> {
        if self.workflow.steps.len() == 1 {
            return Err(Error::invalid("a workflow needs at least one step"));
        }
        self.workflow.steps.remove(at);
        self.changed();
        Ok(())
    }

    fn move_step(&mut self, from: usize, to: usize) {
        let step = self.workflow.steps.remove(from);
        self.workflow.steps.insert(to, step);
        self.changed();
    }

    fn set_step(&mut self, at: usize, text: &str) {
        self.workflow.steps[at] = text.to_owned();
        self.changed();
    }

    /// Toggle whether a failure of step `at` stops the workflow.
    fn toggle_optional(&mut self, at: usize) -> bool {
        let step = &mut self.workflow.steps[at];
        let optional = !split_optional(step).0;
        *step = set_optional(step, optional);
        self.changed();
        optional
    }

    // ---- persistence -------------------------------------------------------------------

    /// Write the draft, dropping parameters no step uses and removing the old file when
    /// the name or scope changed. Returns how many unused parameters were dropped.
    fn save(&mut self, store: &Store) -> Result<usize> {
        let before = self.workflow.params.len();
        let used: Vec<&str> = self
            .workflow
            .steps
            .iter()
            .flat_map(|s| render::placeholders(s))
            .collect();
        let mut pruned = self.workflow.clone();
        pruned.params.retain(|p| used.contains(&p.name.as_str()));

        let dir = store.workflows_dir(self.scope, self.root.as_deref());
        let target = dir.join(format!("{}.toml", self.name));
        if target != self.path && target.exists() {
            return Err(Error::invalid(format!("{} already exists", tilde(&target))));
        }
        let saved = store::write(&dir, &self.name, &pruned)?;
        if saved != self.path {
            store::delete(&self.path)?;
        }
        if self.scope == Scope::Project {
            store.trust(&saved)?;
        }
        let dropped = before - pruned.params.len();
        self.workflow = pruned;
        self.path = saved;
        self.dirty = false;
        Ok(dropped)
    }
}

/// `fr edit [-e] <name>`.
pub fn edit(store: &Store, ui: &mut dyn Ui, name: &str, raw_file: bool) -> Result<()> {
    let Located { entry, .. } = locate(store, ui, name, None)?;
    if raw_file {
        return open_in_editor(store, &entry.path, entry.scope);
    }
    let workflow = store::load(&entry.path)?;
    session(ui, store, Draft::open(entry, workflow))
}

fn open_in_editor(store: &Store, path: &Path, scope: Scope) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_owned());
    let mut words = shlex::split(&editor)
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| vec!["vi".to_owned()]);
    let program = words.remove(0);
    let status = Command::new(&program).args(words).arg(path).status()?;
    if !status.success() {
        return Err(Error::invalid(format!("`{program}` exited with {status}")));
    }
    store::load(path)?; // surface TOML mistakes immediately
    if scope == Scope::Project {
        store.trust(path)?; // the user just read and edited it
    }
    Ok(())
}

// ---- the interactive session ---------------------------------------------------------------

/// Show an error from one menu action but keep the session alive; only cancellation ends it.
fn recover(ui: &mut dyn Ui, result: Result<()>) -> Result<()> {
    match result {
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(e) => {
            let pal = ui.pal();
            ui.say(&format!("  {}", pal.red(&e.to_string())));
            Ok(())
        }
        Ok(()) => Ok(()),
    }
}

fn session(ui: &mut dyn Ui, store: &Store, mut draft: Draft) -> Result<()> {
    loop {
        let pal = ui.pal();
        let summary = card(
            pal,
            &draft.name,
            draft.scope,
            &draft.path,
            &draft.workflow,
            draft.dirty,
        );
        ui.say(summary.trim_end());
        let key = |k: &str| pal.cyan(k);
        ui.say(&format!(
            "\n  {}",
            pal.bold("What would you like to change?")
        ));
        ui.say(&format!(
            "    {} name    {} scope    {} description    {} parameters    {} steps    {} open in editor",
            key("1"), key("2"), key("3"), key("4"), key("5"), key("6")
        ));
        ui.say(&format!("    {} save    {} quit\n", key("s"), key("q")));

        let choice = ui.ask("Choose", "1-6 · s = save · q = quit · Enter = save")?;
        let outcome = match choice.trim().to_ascii_lowercase().as_str() {
            "1" | "name" => change_name(ui, store, &mut draft),
            "2" | "scope" => change_scope(ui, store, &mut draft),
            "3" | "description" => change_description(ui, &mut draft),
            "4" | "parameters" | "params" => parameters_menu(ui, &mut draft),
            "5" | "steps" => steps_menu(ui, &mut draft),
            "6" | "editor" | "file" => open_saved_file(ui, store, &mut draft),
            "" | "s" | "save" if !draft.dirty => {
                ui.say("\n  No changes.\n");
                return Ok(());
            }
            "" | "s" | "save" => match draft.save(store) {
                Ok(dropped) => {
                    ui.say(&format!(
                        "\n  {} Saved {} {}",
                        pal.green("✓"),
                        pal.bold(&draft.name),
                        pal.dim(&format!("({})", draft.scope))
                    ));
                    if dropped > 0 {
                        ui.say(&format!(
                            "    {}",
                            pal.dim("removed parameters no step uses")
                        ));
                    }
                    ui.say(&format!("    {}\n", pal.dim(&tilde(&draft.path))));
                    return Ok(());
                }
                Err(e) => Err(e),
            },
            "q" | "quit" => {
                if !draft.dirty || ui.confirm("Discard unsaved changes?")? {
                    return Ok(());
                }
                Ok(())
            }
            other => {
                ui.say(&format!(
                    "  {}",
                    pal.yellow(&format!("not an option: {other}"))
                ));
                Ok(())
            }
        };
        recover(ui, outcome)?;
    }
}

fn change_name(ui: &mut dyn Ui, store: &Store, draft: &mut Draft) -> Result<()> {
    let answer = ui.ask("Name", &format!("({})", draft.name))?;
    match answer.trim() {
        "" => Ok(()),
        new => draft.rename(store, new),
    }
}

fn change_scope(ui: &mut dyn Ui, store: &Store, draft: &mut Draft) -> Result<()> {
    let to = draft.scope.other();
    let reach = if to == Scope::Project {
        "this repo only"
    } else {
        "available everywhere"
    };
    if !ui.confirm(&format!("Move `{}` to {to} ({reach})?", draft.name))? {
        return Ok(());
    }
    let root = if to == Scope::Project {
        match store::find_save_root(store.cwd()) {
            Some(root) => Some(root),
            None if ui.confirm(&format!(
                "No git repo or .fr/ here. Create .fr/ in {}?",
                tilde(store.cwd())
            ))? =>
            {
                Some(store.cwd().to_path_buf())
            }
            None => return Ok(()),
        }
    } else {
        None
    };
    draft.move_scope(store, root)
}

fn change_description(ui: &mut dyn Ui, draft: &mut Draft) -> Result<()> {
    let answer = ui.ask("Description", "Enter = keep")?;
    if let Some(text) = Some(answer.trim()).filter(|t| !t.is_empty()) {
        draft.workflow.description = text.to_owned();
        draft.changed();
    }
    Ok(())
}

fn parameters_menu(ui: &mut dyn Ui, draft: &mut Draft) -> Result<()> {
    let pal = ui.pal();
    loop {
        ui.say(&format!("\n  {}", pal.bold("Parameters")));
        if draft.workflow.params.is_empty() {
            ui.say(&format!("    {}", pal.dim("none")));
        }
        for p in &draft.workflow.params {
            ui.say(&format!("    {:<12} {}", p.name, pal.dim(&param_note(p))));
        }
        for help in [
            "add <text> [name]       make text found in the steps dynamic",
            "rename <old> <new>",
            "default <name> [value]  set (or clear) a default; $(command) is run each time",
            "choices <name> [a b …]  only accept these values (none = anything)",
            "constant <name> [value] turn it back into fixed text",
        ] {
            ui.say(&format!("    {}", pal.dim(help)));
        }
        ui.say("");

        let line = ui.ask("Parameters", "command · Enter = back")?;
        let words = shlex::split(&line).unwrap_or_default();
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let result = match words.as_slice() {
            [] | ["back"] => return Ok(()),
            ["add", text, rest @ ..] => draft.add_param(text, rest.first().copied()),
            ["rename", old, new] => draft.rename_param(old, new),
            ["default", name, rest @ ..] => draft.set_default(name, (!rest.is_empty()).then(|| rest.join(" "))),
            ["choices", name, rest @ ..] => draft.set_choices(name, rest.iter().map(|c| (*c).to_owned()).collect()),
            ["constant", name, rest @ ..] => make_constant(ui, draft, name, rest),
            _ => Err(Error::usage("try: add <text> [name] · rename <old> <new> · default <name> [value] · choices <name> [a b …] · constant <name>")),
        };
        recover(ui, result)?;
    }
}

/// The fixed value comes from the command, else the parameter's default or example,
/// else the user.
fn make_constant(ui: &mut dyn Ui, draft: &mut Draft, name: &str, rest: &[&str]) -> Result<()> {
    let at = draft.param_index(name)?;
    let p = &draft.workflow.params[at];
    let value = match (
        rest.is_empty(),
        p.default.clone().or_else(|| p.example.clone()),
    ) {
        (false, _) => rest.join(" "),
        (true, Some(known)) => known,
        (true, None) => ui.ask("Fixed value", "")?,
    };
    draft.make_constant(name, &value)
}

fn steps_menu(ui: &mut dyn Ui, draft: &mut Draft) -> Result<()> {
    let pal = ui.pal();
    loop {
        ui.say(&format!("\n  {}", pal.bold("Steps")));
        for (i, step) in draft.workflow.steps.iter().enumerate() {
            let (optional, command) = split_optional(step);
            let tag = if optional {
                format!("  {}", pal.dim("may fail"))
            } else {
                String::new()
            };
            ui.say(&format!(
                "    {}  {}{tag}",
                pal.dim(&format!("{:>2}", i + 1)),
                first_line(command)
            ));
        }
        for help in [
            "rm <n>               remove a step",
            "mv <from> <to>       reorder",
            "set <n> <command…>   replace a step's text",
            "fail <n>             toggle: a failure is reported but the workflow continues",
        ] {
            ui.say(&format!("    {}", pal.dim(help)));
        }
        ui.say("");

        let line = ui.ask("Steps", "command · Enter = back")?;
        let mut parts = line.trim().splitn(3, char::is_whitespace);
        let (command, a, b) = (
            parts.next().unwrap_or(""),
            parts.next().unwrap_or(""),
            parts.next().unwrap_or("").trim(),
        );
        let count = draft.workflow.steps.len();
        let row = |s: &str| {
            s.parse::<usize>()
                .ok()
                .filter(|n| (1..=count).contains(n))
                .map(|n| n - 1)
        };
        let result = match (command, row(a), row(b)) {
            ("" | "back", ..) => return Ok(()),
            ("rm", Some(n), _) => draft.remove_step(n),
            ("mv", Some(from), Some(to)) => {
                draft.move_step(from, to);
                Ok(())
            }
            ("set", Some(n), _) if !b.is_empty() => {
                draft.set_step(n, b);
                Ok(())
            }
            ("fail", Some(n), _) => {
                let note = if draft.toggle_optional(n) {
                    "may fail: replay reports it and carries on"
                } else {
                    "must succeed: replay stops if it fails"
                };
                ui.say(&format!("  {}", pal.dim(&format!("step {}: {note}", n + 1))));
                Ok(())
            }
            ("rm" | "mv" | "set" | "fail", ..) => Err(Error::usage(format!(
                "step numbers run from 1 to {count} — rm <n> · mv <from> <to> · set <n> <command…> · fail <n>"
            ))),
            _ => Err(Error::usage(
                "try: rm <n> · mv <from> <to> · set <n> <command…> · fail <n>",
            )),
        };
        recover(ui, result)?;
    }
}

fn open_saved_file(ui: &mut dyn Ui, store: &Store, draft: &mut Draft) -> Result<()> {
    if draft.dirty {
        if !ui.confirm("Save your changes first? (the editor opens the saved file)")? {
            return Ok(());
        }
        draft.save(store)?;
    }
    open_in_editor(store, &draft.path, draft.scope)?;
    draft.workflow = store::load(&draft.path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::Fixture;
    use crate::ui::testing::{text, Scripted};

    fn workflow(steps: &[&str], params: &[&str]) -> Workflow {
        Workflow::new(
            "desc".into(),
            steps.iter().map(|s| (*s).to_owned()).collect(),
            params
                .iter()
                .map(|n| Param::new(*n, Some(format!("e.g.-{n}"))))
                .collect(),
        )
    }

    /// A global workflow "demo" opened as a draft.
    fn draft_of(fx: &Fixture, wf: &Workflow) -> (Store, Draft) {
        let path = fx.put_workflow("config/workflows", "demo", wf);
        let entry = Entry {
            name: "demo".into(),
            scope: Scope::Global,
            path,
            description: wf.description.clone(),
        };
        (fx.store_in("w"), Draft::open(entry, wf.clone()))
    }

    // ---- parameter operations -----------------------------------------------------------

    #[test]
    fn add_param_replaces_whole_words_only_and_remembers_the_example() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(
            &fx,
            &workflow(
                &["mkdir app", "cd ./app/src", "echo my-app app_v2 apple"],
                &[],
            ),
        );

        d.add_param("app", None).unwrap();

        assert_eq!(
            d.workflow.steps,
            [
                "mkdir {{name}}",
                "cd ./{{name}}/src",
                "echo my-app app_v2 apple"
            ]
        );
        assert_eq!(d.workflow.params, [Param::new("name", Some("app".into()))]);
        assert!(d.dirty);
    }

    #[test]
    fn add_param_rejects_unusable_requests_without_touching_the_draft() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["echo {{a}} text"], &["a"]));
        let before = d.workflow.clone();

        assert!(d.add_param("absent", None).is_err(), "text not found");
        assert!(
            d.add_param("text", Some("a")).is_err(),
            "duplicate parameter name"
        );
        assert!(
            d.add_param("a", Some("b")).is_err(),
            "text equal to a parameter name"
        );
        assert!(d.add_param("text", Some("not valid")).is_err(), "bad name");
        assert!(d.add_param("", Some("e")).is_err(), "empty text");
        assert_eq!(d.workflow, before);
        assert!(!d.dirty);
    }

    #[test]
    fn auto_names_follow_the_existing_parameter_count() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["echo one two"], &[]));
        d.add_param("one", None).unwrap();
        d.add_param("two", None).unwrap();
        assert_eq!(
            d.workflow
                .params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["name", "arg2"]
        );
    }

    #[test]
    fn rename_param_rewrites_every_placeholder_in_every_quote_context() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(
            &fx,
            &workflow(&["echo {{a}} \"{{a}}\" '{{a}}' {{ a }} {{.Keep}}"], &["a"]),
        );

        d.rename_param("a", "project").unwrap();

        assert_eq!(
            d.workflow.steps,
            ["echo {{project}} \"{{project}}\" '{{project}}' {{project}} {{.Keep}}"]
        );
        assert!(d.workflow.param("project").is_some() && d.workflow.param("a").is_none());
        assert!(d.workflow.validate().is_ok());
    }

    #[test]
    fn rename_param_refuses_collisions_and_unknown_names() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["echo {{a}} {{b}}"], &["a", "b"]));
        assert!(d.rename_param("a", "b").is_err());
        assert!(d.rename_param("zzz", "c").is_err());
        assert!(d.rename_param("a", "1bad").is_err());
        assert!(!d.dirty);
    }

    #[test]
    fn make_constant_inlines_the_value_quoted_for_each_position() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(
            &fx,
            &workflow(&["echo {{a}} \"{{a}}\" '{{a}}' keep-{{b}}"], &["a", "b"]),
        );

        d.make_constant("a", "it's mine").unwrap();

        assert_eq!(
            d.workflow.steps,
            [r#"echo 'it'\''s mine' "it's mine" 'it'\''s mine' keep-{{b}}"#]
        );
        assert_eq!(d.workflow.params.len(), 1);
        assert!(d.workflow.validate().is_ok());
    }

    #[test]
    fn set_default_sets_and_clears() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["echo {{a}}"], &["a"]));
        d.set_default("a", Some("x".into())).unwrap();
        assert_eq!(d.workflow.params[0].default.as_deref(), Some("x"));
        d.set_default("a", None).unwrap();
        assert_eq!(d.workflow.params[0].default, None);
        assert!(d.set_default("nope", None).is_err());
    }

    // ---- step operations ----------------------------------------------------------------

    #[test]
    fn choices_are_checked_against_the_default_both_ways() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["deploy {{env}}"], &["env"]));
        let list = |items: &[&str]| items.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>();
        d.set_choices("env", list(&["dev", "prod"])).unwrap();
        assert!(d.set_default("env", Some("staging".into())).is_err());
        d.set_default("env", Some("prod".into())).unwrap();
        assert!(
            d.set_choices("env", list(&["dev"])).is_err(),
            "would orphan the default"
        );
        d.set_choices("env", vec![]).unwrap();
        assert!(d.workflow.params[0].choices.is_empty());
    }

    #[test]
    fn toggling_a_step_marks_and_unmarks_it_as_allowed_to_fail() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["pnpm lint", "pnpm build"], &[]));
        assert!(d.toggle_optional(0));
        assert_eq!(d.workflow.steps[0], "-pnpm lint");
        assert!(!d.toggle_optional(0));
        assert_eq!(d.workflow.steps[0], "pnpm lint");
    }

    #[test]
    fn steps_can_be_reordered_replaced_and_removed_but_never_emptied() {
        let fx = Fixture::new();
        let (_, mut d) = draft_of(&fx, &workflow(&["a", "b", "c"], &[]));

        d.move_step(2, 0);
        assert_eq!(d.workflow.steps, ["c", "a", "b"]);
        d.set_step(1, "A2");
        d.remove_step(0).unwrap();
        d.remove_step(1).unwrap();
        assert_eq!(d.workflow.steps, ["A2"]);
        assert!(d.remove_step(0).is_err(), "the last step stays");
    }

    // ---- identity and persistence -------------------------------------------------------

    #[test]
    fn saving_a_rename_moves_the_file_and_validates_the_new_name() {
        let fx = Fixture::new();
        let (store, mut d) = draft_of(&fx, &workflow(&["true"], &[]));
        let old = d.path.clone();

        assert!(d.rename(&store, "list").is_err(), "reserved");
        assert!(d.rename(&store, "Bad").is_err(), "malformed");
        d.rename(&store, "renamed").unwrap();
        assert!(old.exists(), "nothing is written until save");
        d.save(&store).unwrap();

        assert!(!old.exists());
        assert_eq!(d.path, fx.path("config/workflows/renamed.toml"));
        assert!(d.path.exists() && !d.dirty);
    }

    #[test]
    fn a_rename_onto_an_existing_workflow_is_refused() {
        let fx = Fixture::new();
        let (store, mut d) = draft_of(&fx, &workflow(&["true"], &[]));
        fx.global("taken");
        assert!(d.rename(&store, "taken").is_err());
        assert_eq!(d.name, "demo");
    }

    #[test]
    fn moving_scope_relocates_the_file_and_trusts_the_project_copy() {
        let fx = Fixture::new();
        let (store, mut d) = draft_of(&fx, &workflow(&["true"], &[]));
        let old = d.path.clone();

        d.move_scope(&store, Some(fx.path("repo"))).unwrap();
        d.save(&store).unwrap();

        assert_eq!(
            (d.scope, d.path.clone()),
            (Scope::Project, fx.path("repo/.fr/workflows/demo.toml"))
        );
        assert!(!old.exists() && store.is_trusted(&d.path));

        d.move_scope(&store, None).unwrap();
        d.save(&store).unwrap();
        assert_eq!((d.scope, d.path.clone()), (Scope::Global, old));
        assert!(!fx.path("repo/.fr/workflows/demo.toml").exists());
    }

    #[test]
    fn moving_onto_a_workflow_of_the_same_name_in_the_other_scope_is_refused() {
        let fx = Fixture::new();
        let (store, mut d) = draft_of(&fx, &workflow(&["true"], &[]));
        fx.project("repo", "demo");
        assert!(d.move_scope(&store, Some(fx.path("repo"))).is_err());
        assert_eq!(d.scope, Scope::Global);
    }

    #[test]
    fn save_drops_unused_parameters_and_refuses_undefined_placeholders() {
        let fx = Fixture::new();
        let (store, mut d) = draft_of(&fx, &workflow(&["echo {{a}} {{b}}"], &["a", "b"]));
        d.set_step(0, "echo {{a}}");
        assert_eq!(d.save(&store).unwrap(), 1);
        assert_eq!(store::load(&d.path).unwrap().params.len(), 1);

        d.set_step(0, "echo {{ghost}}");
        let err = d.save(&store).unwrap_err().to_string();
        assert!(err.contains("{{ghost}}"), "{err}");
        assert!(d.dirty, "a failed save keeps the draft");
        assert!(
            store::load(&d.path).unwrap().steps[0].contains("{{a}}"),
            "the file on disk is untouched"
        );
    }

    // ---- the interactive session --------------------------------------------------------

    fn session_on(
        fx: &Fixture,
        wf: &Workflow,
        answers: impl IntoIterator<Item = crate::ui::testing::Reply>,
    ) -> (Scripted, Result<()>) {
        let (store, d) = draft_of(fx, wf);
        let mut ui = Scripted::new(answers);
        let result = session(&mut ui, &store, d);
        (ui, result)
    }

    #[test]
    fn a_session_can_rename_a_parameter_and_save_with_enter() {
        let fx = Fixture::new();
        let (ui, result) = session_on(
            &fx,
            &workflow(&["mkdir {{name}}", "cd {{name}}"], &["name"]),
            [text("4"), text("rename name project"), text(""), text("")],
        );

        result.unwrap();
        let saved = store::load(&fx.path("config/workflows/demo.toml")).unwrap();
        assert_eq!(saved.steps, ["mkdir {{project}}", "cd {{project}}"]);
        assert!(ui.transcript().contains("Saved") && ui.exhausted());
    }

    #[test]
    fn a_session_survives_bad_input_and_reports_it() {
        let fx = Fixture::new();
        let (ui, result) = session_on(
            &fx,
            &workflow(&["echo hi"], &[]),
            [
                text("9"),
                text("4"),
                text("add missing"),
                text("frobnicate"),
                text(""),
                text("5"),
                text("rm 1"),
                text("rm 7"),
                text(""),
                text("q"),
            ],
        );

        result.unwrap();
        let said = ui.transcript();
        for expected in [
            "not an option: 9",
            "does not appear in any step",
            "try: add",
            "a workflow needs at least one step",
            "step numbers run from 1 to 1",
        ] {
            assert!(said.contains(expected), "missing {expected:?}");
        }
        assert!(ui.exhausted());
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_and_can_be_declined() {
        let fx = Fixture::new();
        let (_, result) = session_on(
            &fx,
            &workflow(&["echo hi"], &[]),
            [
                text("3"),
                text("changed"),
                text("q"),
                text("n"),
                text("q"),
                text("y"),
            ],
        );
        result.unwrap();
        assert_eq!(
            store::load(&fx.path("config/workflows/demo.toml"))
                .unwrap()
                .description,
            "desc",
            "discarded"
        );
    }

    #[test]
    fn enter_without_changes_just_leaves() {
        let fx = Fixture::new();
        let (ui, result) = session_on(&fx, &workflow(&["echo hi"], &[]), [text("")]);
        result.unwrap();
        assert!(ui.transcript().contains("No changes"));
    }

    #[test]
    fn closing_input_cancels_the_session_and_keeps_the_file() {
        let fx = Fixture::new();
        let (_, result) = session_on(
            &fx,
            &workflow(&["echo hi"], &[]),
            [text("3"), text("never saved")],
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert_eq!(
            store::load(&fx.path("config/workflows/demo.toml"))
                .unwrap()
                .description,
            "desc"
        );
    }

    #[test]
    fn the_constant_command_falls_back_to_the_example_then_asks() {
        let fx = Fixture::new();
        let wf = workflow(&["echo {{a}} {{b}}"], &["a", "b"]);
        let (_, mut d) = draft_of(&fx, &wf);
        make_constant(&mut Scripted::new([]), &mut d, "a", &[]).unwrap();
        assert_eq!(
            d.workflow.steps[0], "echo e.g.-a {{b}}",
            "the recorded example is inlined"
        );

        d.workflow.params[0].example = None;
        make_constant(&mut Scripted::new([text("typed value")]), &mut d, "b", &[]).unwrap();
        assert_eq!(d.workflow.steps[0], "echo e.g.-a 'typed value'");
    }
}
