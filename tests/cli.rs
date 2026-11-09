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
    assert!(text(&run.stderr).contains("step 2/3 failed (earlier steps already ran)"));
}

#[test]
fn the_shell_integration_is_valid_zsh() {
    if !have("zsh") {
        eprintln!("skipped: zsh is not installed");
        return;
    }
    let sandbox = Sandbox::new();
    let script = sandbox.root.path().join("init.zsh");
    std::fs::write(&script, sandbox.fr(&["init", "zsh"], "").stdout).unwrap();
    let check = sandbox
        .command("zsh")
        .arg("-n")
        .arg(&script)
        .output()
        .unwrap();
    assert!(check.status.success(), "{}", text(&check.stderr));
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
    assert!(help.status.success() && text(&help.stdout).contains("COMMANDS"));
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
