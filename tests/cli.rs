//! End-to-end tests: the real binary, an isolated HOME, and (when installed) real shells.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            root: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(sandbox.work()).unwrap();
        sandbox
    }

    fn work(&self) -> PathBuf {
        self.root.path().join("work")
    }

    fn bin_dir() -> PathBuf {
        Path::new(env!("CARGO_BIN_EXE_fr"))
            .parent()
            .unwrap()
            .to_owned()
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
        let root = self.root.path();
        let path = format!(
            "{}:{}",
            Self::bin_dir().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.current_dir(self.work())
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("VISUAL")
            .env_remove("EDITOR");
        cmd
    }

    /// Run `fr` with `stdin` as the answers to any prompts.
    fn fr(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = self
            .command(env!("CARGO_BIN_EXE_fr"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// Record `commands` (status, text) as a finished session and answer the review prompts.
    fn record(&self, commands: &[(i32, &str)], answers: &str) -> Output {
        let started = self.fr(&["record"], "");
        assert!(started.status.success());
        let session = text(&started.stdout).trim().to_owned();
        for (status, command) in commands {
            let event = self.fr(&["_ev", &session, &status.to_string(), command], "");
            assert!(event.status.success(), "{}", text(&event.stderr));
        }
        let stopped = self.fr(&["stop", &session], answers);
        assert!(!Path::new(&session).exists(), "the session log is removed");
        stopped
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn have(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

/// Record the canonical example: a repeated project name, a failed command, a constant package.
fn record_scaffold(sandbox: &Sandbox) {
    let out = sandbox.record(
        &[
            (0, "mkdir demo"),
            (0, "cd demo"),
            (1, "ls /definitely/missing"),
            (0, "touch demo.txt"),
            (0, "echo installing zustand"),
        ],
        "1\nscaffold\n\n",
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
}

#[test]
fn recording_saves_a_parameterized_workflow_and_skips_failed_commands() {
    let sandbox = Sandbox::new();
    record_scaffold(&sandbox);

    let saved = std::fs::read_to_string(
        sandbox
            .root
            .path()
            .join("config/fr/workflows/scaffold.toml"),
    )
    .unwrap();
    assert!(
        saved.contains("mkdir {{name}}") && saved.contains("touch {{name}}.txt"),
        "{saved}"
    );
    assert!(
        saved.contains("echo installing zustand"),
        "one-off values stay constant"
    );
    assert!(
        !saved.contains("definitely/missing"),
        "failed command excluded"
    );

    let listed = text(&sandbox.fr(&["list"], "").stdout);
    assert!(
        listed.contains("scaffold") && listed.contains("global"),
        "{listed}"
    );
    let shown = text(&sandbox.fr(&["show", "scaf"], "").stdout);
    assert!(
        shown.contains("name") && shown.contains("required · e.g. demo"),
        "{shown}"
    );
}

#[test]
fn an_exported_script_replays_the_recording_with_a_new_argument() {
    let sandbox = Sandbox::new();
    record_scaffold(&sandbox);

    let exported = sandbox.fr(&["export", "scaffold"], "");
    assert!(exported.status.success(), "{}", text(&exported.stderr));
    let script = sandbox.root.path().join("scaffold.sh");
    std::fs::write(&script, &exported.stdout).unwrap();

    let run = sandbox
        .command("bash")
        .arg(&script)
        .arg("my new app")
        .output()
        .unwrap();
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(sandbox.work().join("my new app/my new app.txt").exists());
    assert!(text(&run.stdout).contains("installing zustand"));
}

/// The headline guarantee: replay runs in the *current* shell, so `cd` persists.
#[test]
fn the_zsh_function_replays_in_the_current_shell_so_cd_persists() {
    if !have("zsh") {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let sandbox = Sandbox::new();
    record_scaffold(&sandbox);

    let out = sandbox
        .command("zsh")
        .args([
            "-c",
            r#"eval "$(fr init zsh)"; fr scaf "my app"; echo "cwd=$PWD"; fr -n scaffold | cat"#,
        ])
        .output()
        .unwrap();

    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stderr}");
    assert!(stdout.contains("installing zustand"), "{stdout}");
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("cwd=") && l.ends_with("work/my app")),
        "{stdout}"
    );
    assert!(
        stderr.contains("matched by prefix") && stderr.contains("mkdir 'my app'"),
        "{stderr}"
    );
    assert!(sandbox.work().join("my app/my app.txt").exists());
}

#[test]
fn a_failing_step_stops_the_chain_and_sets_the_shell_status() {
    if !have("zsh") {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let sandbox = Sandbox::new();
    let out = sandbox.record(
        &[(0, "echo one"), (0, "false"), (0, "echo three")],
        "flaky\n\n",
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    // `false` recorded with status 0 is not a failure at record time; replay is what fails.

    let run = sandbox
        .command("zsh")
        .args(["-c", r#"eval "$(fr init zsh)"; fr flaky; echo "rc=$?""#])
        .output()
        .unwrap();
    let stdout = text(&run.stdout);
    assert!(
        stdout.contains("one") && !stdout.contains("three") && stdout.contains("rc=1"),
        "{stdout}"
    );
    assert!(text(&run.stderr).contains("step 2/3 failed (earlier steps already ran"));
}

#[test]
fn every_shell_integration_parses() {
    let sandbox = Sandbox::new();
    for (shell, check) in [("zsh", "-n"), ("bash", "-n"), ("fish", "--no-execute")] {
        if !have(shell) {
            eprintln!("skipped: {shell} is not installed");
            continue;
        }
        let script = sandbox.root.path().join(format!("init.{shell}"));
        let init = sandbox.fr(&["init", shell], "");
        assert!(init.status.success(), "{}", text(&init.stderr));
        std::fs::write(&script, init.stdout).unwrap();
        let out = sandbox
            .command(shell)
            .arg(check)
            .arg(&script)
            .output()
            .unwrap();
        assert!(out.status.success(), "{shell}: {}", text(&out.stderr));
    }
}

// ---- the same behaviour in every supported shell ------------------------------------------

/// Each shell that is installed, with how to load the integration and read `$?`.
fn shells() -> Vec<(&'static str, &'static str, &'static str)> {
    [
        ("zsh", r#"eval "$(fr init zsh)""#, "$?"),
        ("bash", r#"eval "$(fr init bash)""#, "$?"),
        ("fish", "fr init fish | source", "$status"),
    ]
    .into_iter()
    .filter(|(shell, ..)| {
        let found = have(shell);
        if !found {
            eprintln!("skipped: {shell} is not installed");
        }
        found
    })
    .collect()
}

impl Sandbox {
    fn workflow(&self, name: &str, toml: &str) {
        let dir = self.root.path().join("config/fr/workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toml")), toml).unwrap();
    }

    /// Run `script` in `shell` after loading the integration; `{rc}` expands to the
    /// shell's status variable.
    fn shell(
        &self,
        shell: &str,
        init: &str,
        rc: &str,
        script: &str,
        stdin: &str,
    ) -> (String, String) {
        let body = format!("{init}\n{}", script.replace("{rc}", rc));
        let mut child = self
            .command(shell)
            .args(["-c", &body])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        (text(&out.stdout), text(&out.stderr))
    }
}

fn dialect_line(shell: &str) -> &'static str {
    if shell == "fish" {
        "shell = \"fish\"\n"
    } else {
        ""
    }
}

/// Nested workflows, a step allowed to fail, and resuming after a failure — in one run.
#[test]
fn workflows_nest_tolerate_optional_failures_and_resume_where_they_failed() {
    for (shell, init, rc) in shells() {
        let sandbox = Sandbox::new();
        let d = dialect_line(shell);
        sandbox.workflow(
            "greet",
            &format!("{d}steps = [\"echo hello {{{{who}}}}\"]\n[[params]]\nname = \"who\"\n"),
        );
        sandbox.workflow(
            "outer",
            &format!(
                "{d}steps = [\"-false\", \"fr greet {{{{who}}}}\", \"cd sub\", \"test -f marker\", \"echo finished\"]\n[[params]]\nname = \"who\"\n"
            ),
        );
        std::fs::create_dir_all(sandbox.work().join("sub")).unwrap();

        let (out, err) = sandbox.shell(
            shell,
            init,
            rc,
            "fr outer 'Om R'; echo \"rc={rc}\"; touch marker; fr resume; echo \"rc={rc}\"; echo \"cwd=$PWD\"; fr resume; echo \"again={rc}\"",
            "",
        );

        assert!(
            out.contains("hello Om R"),
            "{shell}: nested workflow ran\n{out}\n{err}"
        );
        assert!(
            err.contains("step 1/5 failed with status 1 (allowed to fail"),
            "{shell}: {err}"
        );
        assert!(
            err.contains("step 4/5 failed") && err.contains("fr resume"),
            "{shell}: {err}"
        );
        let lines: Vec<&str> = out.lines().collect();
        let first_rc = lines.iter().position(|l| l.starts_with("rc=")).unwrap();
        assert_eq!(lines[first_rc], "rc=1", "{shell}: {out}");
        assert_eq!(
            lines[first_rc + 1],
            "finished",
            "{shell}: resumed at step 4, not from the top\n{out}"
        );
        assert_eq!(lines[first_rc + 2], "rc=0", "{shell}: {out}");
        assert!(
            lines[first_rc + 3].ends_with("/work/sub"),
            "{shell}: cd persisted\n{out}"
        );
        assert_eq!(
            out.matches("hello Om R").count(),
            1,
            "{shell}: step 2 was not re-run\n{out}"
        );
        assert!(
            lines.last().unwrap().starts_with("again=") && !lines.last().unwrap().ends_with("=0"),
            "{shell}: nothing left to resume\n{out}"
        );
    }
}

#[test]
fn choices_are_enforced_and_computed_defaults_run_in_the_shell() {
    for (shell, init, rc) in shells() {
        let sandbox = Sandbox::new();
        sandbox.workflow(
            "deploy",
            &format!(
                "{}steps = [\"echo deploying {{{{env}}}} {{{{tag}}}}\"]\n\
                 [[params]]\nname = \"env\"\nchoices = [\"dev\", \"prod\"]\n\
                 [[params]]\nname = \"tag\"\ndefault = \"v$(echo 7)\"\n",
                dialect_line(shell)
            ),
        );
        let (out, err) = sandbox.shell(
            shell,
            init,
            rc,
            "fr deploy staging; echo \"rc={rc}\"; fr deploy prod; fr deploy; fr deploy dev v8",
            "2\n",
        );
        assert!(
            err.contains("not a valid env — choose one of: dev, prod"),
            "{shell}: {err}"
        );
        assert!(out.contains("rc=1"), "{shell}: {out}");
        assert!(
            out.contains("deploying prod v7"),
            "{shell}: computed default\n{out}\n{err}"
        );
        assert_eq!(
            out.matches("deploying prod v7").count(),
            2,
            "{shell}: picked prod as choice 2\n{out}"
        );
        assert!(
            out.contains("deploying dev v8"),
            "{shell}: an argument beats the default\n{out}"
        );
    }
}

#[test]
fn a_shim_runs_a_workflow_without_the_fr_prefix_now_and_in_new_shells() {
    for (shell, init, rc) in shells() {
        let sandbox = Sandbox::new();
        sandbox.workflow(
            "hi",
            &format!("{}steps = [\"echo shimmed\"]\n", dialect_line(shell)),
        );
        let (out, err) = sandbox.shell(shell, init, rc, "fr shim hi; hi", "");
        assert!(out.contains("shimmed"), "{shell}: same shell\n{out}\n{err}");
        let (out, _) = sandbox.shell(
            shell,
            init,
            rc,
            "hi; fr unshim hi; hi; echo \"rc={rc}\"",
            "",
        );
        assert_eq!(out.matches("shimmed").count(), 1, "{shell}: {out}");
        assert!(
            !out.contains("rc=0"),
            "{shell}: unshimmed in the same shell\n{out}"
        );
    }
}

/// The hooks log commands as they finish; `fr save` turns the last ones into a workflow.
#[test]
fn commands_already_run_can_be_saved_as_a_workflow() {
    for (shell, init, rc) in shells() {
        let sandbox = Sandbox::new();
        // Drive the hooks the way an interactive shell would after each command.
        let script = match shell {
            "zsh" => {
                "__fr_preexec 'mkdir made-by-zsh'; mkdir made-by-zsh; __fr_precmd\n\
                      __fr_preexec 'fr list'; __fr_precmd\n\
                      fr save 1"
            }
            "bash" => {
                "set -o history; __fr_precmd\n\
                       history -s 'mkdir made-by-bash'; mkdir made-by-bash; __fr_precmd\n\
                       history -s 'fr list'; __fr_precmd\n\
                       fr save 1"
            }
            _ => {
                "mkdir made-by-fish; __fr_postexec 'mkdir made-by-fish'\n\
                  __fr_postexec 'fr list'\n\
                  fr save 1"
            }
        };
        let (_, err) = sandbox.shell(shell, init, rc, script, "made\n\n");
        let saved =
            std::fs::read_to_string(sandbox.root.path().join("config/fr/workflows/made.toml"))
                .unwrap_or_else(|e| panic!("{shell}: {e}\n{err}"));
        assert!(
            saved.contains(&format!("mkdir made-by-{shell}")),
            "{shell}: {saved}"
        );
    }
}

#[test]
fn project_workflows_are_scoped_to_their_repository() {
    let sandbox = Sandbox::new();
    let repo = sandbox.work().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    let started = sandbox
        .command(env!("CARGO_BIN_EXE_fr"))
        .arg("record")
        .current_dir(&repo)
        .output()
        .unwrap();
    let session = text(&started.stdout).trim().to_owned();
    assert!(sandbox
        .fr(&["_ev", &session, "0", "echo from-repo"], "")
        .status
        .success());
    let mut stop = sandbox
        .command(env!("CARGO_BIN_EXE_fr"))
        .args(["stop", &session])
        .current_dir(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    stop.stdin.take().unwrap().write_all(b"hello\n\n").unwrap();
    assert!(stop.wait_with_output().unwrap().status.success());

    assert!(
        repo.join(".fr/workflows/hello.toml").exists(),
        "inside a git repo the default scope is project"
    );
    std::fs::create_dir_all(repo.join("src/deep")).unwrap();
    let subdir = sandbox
        .command(env!("CARGO_BIN_EXE_fr"))
        .arg("list")
        .current_dir(repo.join("src/deep"))
        .output()
        .unwrap();
    assert!(
        text(&subdir.stdout).contains("hello"),
        "subdirectories inherit the project's workflows"
    );
    let from_repo = sandbox
        .command(env!("CARGO_BIN_EXE_fr"))
        .arg("list")
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        text(&from_repo.stdout).contains("hello") && text(&from_repo.stdout).contains("project")
    );
    assert!(
        text(&sandbox.fr(&["list"], "").stdout).contains("No workflows yet"),
        "invisible outside the repo"
    );
}

#[test]
fn errors_are_reported_on_stderr_with_a_failing_status() {
    let sandbox = Sandbox::new();

    let missing = sandbox.fr(&["_run", "nope"], "");
    assert!(!missing.status.success());
    assert!(
        text(&missing.stderr).starts_with("error: no workflow called `nope`"),
        "{}",
        text(&missing.stderr)
    );
    assert!(
        missing.stdout.is_empty(),
        "stdout carries only the script, so $(…) captures nothing"
    );

    let bare = sandbox.fr(&["deploy"], "");
    assert!(!bare.status.success() && text(&bare.stderr).contains("fr init zsh"));
}

#[test]
fn help_and_version_succeed() {
    let sandbox = Sandbox::new();
    let help = sandbox.fr(&["--help"], "");
    assert!(help.status.success() && text(&help.stdout).contains("MANAGE"));
    let version = sandbox.fr(&["--version"], "");
    assert_eq!(
        text(&version.stdout).trim(),
        format!("fr {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cancelling_a_prompt_exits_nonzero_and_leaves_nothing_behind() {
    let sandbox = Sandbox::new();
    let out = sandbox.record(&[(0, "make build")], ""); // stdin closes at the name prompt
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("Cancelled"));
    assert!(!sandbox.root.path().join("config/fr/workflows").exists());
}
