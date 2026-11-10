//! Argument parsing and dispatch.
//!
//! Parsing is hand-written on purpose: the grammar is tiny (`fr <command> …` or
//! `fr [flags] <workflow> [args…]`), and `fr` runs on every prompt while recording, so a
//! sub-millisecond start and a small binary matter more than derive-generated help.

use crate::run::Options;
use crate::shell::Shell;
use crate::store::Store;
use crate::ui::{Pal, Ui};
use crate::{edit, history, manage, record, run, session, Error, Result};
use std::io::Write;
use std::path::PathBuf;

pub const USAGE: &str = "fr — record terminal workflows once, replay them anywhere

USAGE
  fr                          Start recording; run again to stop and save
  fr <name> [args…]           Run a workflow (a unique prefix works: fr ne)

CREATE
  record, stop, status        Control recording
  save [n]                    Save commands you already ran (the last n, or pick)
  suggest                     Turn a sequence you keep typing into a workflow

RUN
  run <name> [args…]          Same as fr <name>
  resume                      Continue the last failed workflow from the failed step

MANAGE
  list, ls                    Workflows for this folder, most relevant first
  show <name>                 Print a workflow
  edit <name>                 Change name, scope, parameters, choices and steps
  rm <name>                   Delete a workflow
  export <name>               Print a portable bash script
  shim [name…]                Run workflows without typing fr (next = fr next)
  unshim <name…>              Undo shim
  init zsh|bash|fish          Print the shell integration

RUN FLAGS
  -n, --dry-run               Preview only
  -y, --yes                   Skip the destructive-step confirmation
  -g, --global                Use the global workflow
  -p, --project               Use the project workflow
  --from <step>               Start at this step

EDIT FLAGS
  -e, --file                  Open the raw TOML in $EDITOR

SETUP
  zsh   ~/.zshrc (after compinit)    eval \"$(fr init zsh)\"
  bash  ~/.bashrc                    eval \"$(fr init bash)\"
  fish  ~/.config/fish/config.fish   fr init fish | source

DOCS
  https://github.com/omraval18/flow-record/blob/main/docs/reference.md
";

/// Built-in command names; workflows may not use them.
const COMMANDS: &[&str] = &[
    "record", "start", "stop", "status", "run", "list", "ls", "show", "edit", "rm", "delete",
    "remove", "export", "init", "version", "help", "save", "suggest", "resume", "shim", "unshim",
];

pub fn is_reserved(name: &str) -> bool {
    COMMANDS.contains(&name)
}

/// Whether `fr <args>` replays a workflow (as opposed to controlling fr itself).
pub fn is_replay(args: &[String]) -> bool {
    let args = match args.split_first() {
        Some((first, rest)) if first == "run" => rest,
        _ => args,
    };
    matches!(
        Options::parse(args),
        Ok(o) if !o.dry_run && !is_reserved(&o.name) && !o.name.starts_with('_')
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Record {
        shell: Shell,
    },
    Stop {
        session: PathBuf,
    },
    Run(Options),
    Save {
        shell: Shell,
        pid: u32,
        count: Option<usize>,
    },
    Suggest {
        shell: Shell,
    },
    List,
    Show {
        name: String,
    },
    Edit {
        name: String,
        raw_file: bool,
    },
    Remove {
        name: String,
    },
    Export {
        name: String,
    },
    Shim {
        names: Vec<String>,
    },
    Unshim {
        names: Vec<String>,
    },
    Init(Shell),
    Help,
    Version,
    /// Internal: called by the shell hook after every recorded command.
    Event {
        session: PathBuf,
        status: i32,
        command: String,
    },
    /// Internal: completion data.
    Complete {
        args: Vec<String>,
    },
}

fn shell_arg(name: &str) -> Result<Shell> {
    Shell::parse(name).ok_or_else(|| {
        Error::usage(format!(
            "`{name}` is not a supported shell — use zsh, bash or fish"
        ))
    })
}

fn needs_integration() -> Error {
    Error::usage(
        "this needs the shell integration — add `eval \"$(fr init zsh)\"` to ~/.zshrc \
         (bash: `eval \"$(fr init bash)\"` in ~/.bashrc; fish: `fr init fish | source` in config.fish)",
    )
}

impl Command {
    pub fn parse(args: &[String]) -> Result<Self> {
        let Some((first, rest)) = args.split_first() else {
            return Ok(Self::Help);
        };
        let one = |what: &str| -> Result<String> {
            match rest {
                [name] => Ok(name.clone()),
                _ => Err(Error::usage(format!("usage: fr {first} <{what}>"))),
            }
        };
        if matches!(rest, [flag] if flag == "-h" || flag == "--help") && is_reserved(first) {
            return Ok(Self::Help);
        }
        Ok(match first.as_str() {
            "help" | "-h" | "--help" => Self::Help,
            "version" | "-V" | "--version" => Self::Version,
            "record" | "start" => Self::Record {
                shell: match rest {
                    [] => Shell::Zsh,
                    [shell] => shell_arg(shell)?,
                    _ => return Err(Error::usage("usage: fr record")),
                },
            },
            "stop" => Self::Stop {
                session: PathBuf::from(one("session file")?),
            },
            "run" | "_run" => Self::Run(Options::parse(rest)?),
            // `_exec <shell> <depth> [flags] <name> [args…]`, from the shell function.
            "_exec" => match rest {
                [shell, depth, run @ ..] => Self::Run(Options {
                    shell: shell_arg(shell)?,
                    depth: depth.parse().unwrap_or(0),
                    remember: true,
                    ..Options::parse(run)?
                }),
                _ => {
                    return Err(Error::usage(
                        "usage: fr _exec <shell> <depth> <workflow> [args…]",
                    ))
                }
            },
            "_save" => match rest {
                [shell, pid, count @ ..] => Self::Save {
                    shell: shell_arg(shell)?,
                    pid: pid
                        .parse()
                        .map_err(|_| Error::usage(format!("bad shell pid `{pid}`")))?,
                    count: match count {
                        [] => None,
                        [n] => {
                            Some(n.parse::<usize>().ok().filter(|&n| n > 0).ok_or_else(|| {
                                Error::usage(format!(
                                    "`{n}` is not a number of commands — usage: fr save [n]"
                                ))
                            })?)
                        }
                        _ => return Err(Error::usage("usage: fr save [n]")),
                    },
                },
                _ => return Err(Error::usage("usage: fr _save <shell> <pid> [n]")),
            },
            "suggest" | "_suggest" => Self::Suggest {
                shell: match rest {
                    [] => Shell::Zsh,
                    [shell] if first == "_suggest" => shell_arg(shell)?,
                    _ => return Err(Error::usage("usage: fr suggest")),
                },
            },
            "list" | "ls" => Self::List,
            "show" => Self::Show {
                name: one("workflow")?,
            },
            "rm" | "delete" | "remove" => Self::Remove {
                name: one("workflow")?,
            },
            "export" => Self::Export {
                name: one("workflow")?,
            },
            "shim" => Self::Shim {
                names: rest.to_vec(),
            },
            "unshim" if !rest.is_empty() => Self::Unshim {
                names: rest.to_vec(),
            },
            "unshim" => return Err(Error::usage("usage: fr unshim <name…>")),
            "edit" => parse_edit(rest)?,
            "init" => match rest {
                [shell] => Self::Init(shell_arg(shell)?),
                _ => return Err(Error::usage("usage: fr init zsh|bash|fish")),
            },
            "_ev" => match rest {
                [session, status, command] => Self::Event {
                    session: PathBuf::from(session),
                    status: status
                        .parse()
                        .map_err(|_| Error::usage(format!("bad exit status `{status}`")))?,
                    command: command.clone(),
                },
                _ => return Err(Error::usage("usage: fr _ev <session> <status> <command>")),
            },
            "_complete" => Self::Complete {
                args: rest.to_vec(),
            },
            _ => return Err(needs_integration()),
        })
    }
}

fn parse_edit(args: &[String]) -> Result<Command> {
    let (mut raw_file, mut name) = (false, None);
    for arg in args {
        match arg.as_str() {
            "-e" | "--file" => raw_file = true,
            flag if flag.starts_with('-') => {
                return Err(Error::usage(format!("unknown flag `{flag}`")))
            }
            _ if name.is_none() => name = Some(arg.clone()),
            _ => return Err(Error::usage("usage: fr edit [-e|--file] <workflow>")),
        }
    }
    Ok(Command::Edit {
        name: name.ok_or_else(|| Error::usage("usage: fr edit [-e|--file] <workflow>"))?,
        raw_file,
    })
}

/// Execute a parsed command. Machine-readable output goes to `out`; everything meant
/// for a person goes through `ui`.
pub fn execute(
    command: Command,
    store: &Store,
    ui: &mut dyn Ui,
    out: &mut dyn Write,
) -> Result<()> {
    match command {
        Command::Help => out.write_all(USAGE.as_bytes())?,
        Command::Version => writeln!(out, "fr {}", env!("CARGO_PKG_VERSION"))?,
        Command::Init(shell) => {
            let log = store.dirs().history_log();
            let _ = history::compact(&log);
            out.write_all(shell.integration(&log, &store.shims()).as_bytes())?;
        }
        Command::Record { shell } => {
            let path = session::create(&store.dirs().sessions(), store.cwd(), shell.dialect())?;
            writeln!(out, "{}", path.display())?;
        }
        Command::Stop { session } => record::finish(ui, store, &session)?,
        Command::Event {
            session,
            status,
            command,
        } => {
            session::append(&session, status, &command)?;
        }
        Command::Run(options) => run::run(store, ui, &options, out)?,
        Command::Save { shell, pid, count } => {
            record::save(ui, store, pid, count, shell.dialect())?;
        }
        Command::Suggest { shell } => record::suggest(ui, store, shell.dialect())?,
        Command::List => {
            let habits = record::habits_here(store).len();
            manage::list(store, Pal::stdout(), habits, out)?;
        }
        Command::Show { name } => manage::show(store, ui, Pal::stdout(), &name, out)?,
        Command::Edit { name, raw_file } => edit::edit(store, ui, &name, raw_file)?,
        Command::Remove { name } => manage::remove(store, ui, &name)?,
        Command::Export { name } => manage::export(store, ui, &name, out)?,
        Command::Shim { names } => manage::shim(store, ui, &names, out)?,
        Command::Unshim { names } => manage::unshim(store, ui, &names)?,
        Command::Complete { args } => manage::complete(store, &args, out)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command> {
        Command::parse(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn bare_invocation_and_help_flags_print_usage() {
        for args in [
            &[][..],
            &["help"],
            &["-h"],
            &["--help"],
            &["edit", "--help"],
            &["list", "-h"],
        ] {
            assert_eq!(parse(args).unwrap(), Command::Help, "{args:?}");
        }
    }

    #[test]
    fn commands_and_their_aliases_parse() {
        let zsh = Command::Record { shell: Shell::Zsh };
        assert_eq!(parse(&["record"]).unwrap(), zsh);
        assert_eq!(parse(&["start"]).unwrap(), zsh);
        assert_eq!(
            parse(&["record", "fish"]).unwrap(),
            Command::Record { shell: Shell::Fish }
        );
        assert_eq!(parse(&["ls"]).unwrap(), Command::List);
        assert_eq!(
            parse(&["rm", "x"]).unwrap(),
            Command::Remove { name: "x".into() }
        );
        assert_eq!(
            parse(&["delete", "x"]).unwrap(),
            Command::Remove { name: "x".into() }
        );
        assert_eq!(parse(&["init", "zsh"]).unwrap(), Command::Init(Shell::Zsh));
        assert_eq!(
            parse(&["init", "fish"]).unwrap(),
            Command::Init(Shell::Fish)
        );
        assert!(parse(&["init", "tcsh"])
            .unwrap_err()
            .to_string()
            .contains("zsh, bash or fish"));
        assert_eq!(parse(&["--version"]).unwrap(), Command::Version);
    }

    #[test]
    fn edit_takes_a_name_and_an_optional_raw_flag_in_either_order() {
        let expected = Command::Edit {
            name: "x".into(),
            raw_file: true,
        };
        assert_eq!(parse(&["edit", "-e", "x"]).unwrap(), expected);
        assert_eq!(parse(&["edit", "x", "--file"]).unwrap(), expected);
        assert_eq!(
            parse(&["edit", "x"]).unwrap(),
            Command::Edit {
                name: "x".into(),
                raw_file: false
            }
        );
        assert!(
            parse(&["edit"]).is_err()
                && parse(&["edit", "a", "b"]).is_err()
                && parse(&["edit", "--bogus", "a"]).is_err()
        );
    }

    #[test]
    fn commands_that_take_one_name_reject_anything_else() {
        for command in ["show", "rm", "export", "stop"] {
            assert!(parse(&[command]).is_err(), "{command} without argument");
            assert!(
                parse(&[command, "a", "b"]).is_err(),
                "{command} with two arguments"
            );
        }
    }

    #[test]
    fn run_and_the_internal_run_alias_share_option_parsing() {
        let expected = Command::Run(Options {
            dry_run: true,
            name: "go".into(),
            args: vec!["a".into()],
            ..Options::default()
        });
        assert_eq!(parse(&["run", "-n", "go", "a"]).unwrap(), expected);
        assert_eq!(parse(&["_run", "-n", "go", "a"]).unwrap(), expected);
    }

    #[test]
    fn the_shell_function_passes_its_shell_and_nesting_depth() {
        assert_eq!(
            parse(&["_exec", "bash", "2", "--from", "3", "go", "a"]).unwrap(),
            Command::Run(Options {
                shell: Shell::Bash,
                depth: 2,
                remember: true,
                from: Some(3),
                name: "go".into(),
                args: vec!["a".into()],
                ..Options::default()
            })
        );
    }

    #[test]
    fn save_takes_an_optional_positive_count() {
        assert_eq!(
            parse(&["_save", "zsh", "42", "3"]).unwrap(),
            Command::Save {
                shell: Shell::Zsh,
                pid: 42,
                count: Some(3)
            }
        );
        assert!(matches!(
            parse(&["_save", "zsh", "42"]).unwrap(),
            Command::Save { count: None, .. }
        ));
        assert!(parse(&["_save", "zsh", "42", "0"]).is_err());
        assert!(parse(&["_save", "zsh", "42", "x"]).is_err());
        assert!(parse(&["save"])
            .unwrap_err()
            .to_string()
            .contains("shell integration"));
    }

    #[test]
    fn only_workflow_runs_count_as_replays() {
        let replay =
            |args: &[&str]| is_replay(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert!(
            replay(&["build"]) && replay(&["-g", "deploy", "prod"]) && replay(&["run", "build"])
        );
        assert!(
            !replay(&[]) && !replay(&["list"]) && !replay(&["-n", "build"]) && !replay(&["stop"])
        );
        assert!(!replay(&["_ev", "x"]) && !replay(&["--help"]));
    }

    #[test]
    fn the_hook_event_carries_status_and_the_full_command() {
        assert_eq!(
            parse(&["_ev", "/s", "130", "sleep 5 && echo done"]).unwrap(),
            Command::Event {
                session: "/s".into(),
                status: 130,
                command: "sleep 5 && echo done".into()
            }
        );
        assert!(parse(&["_ev", "/s", "abc", "x"]).is_err());
        assert!(parse(&["_ev", "/s", "0"]).is_err());
    }

    #[test]
    fn anything_else_is_a_workflow_name_that_needs_the_shell_function() {
        let err = parse(&["deploy", "prod"]).unwrap_err().to_string();
        assert!(err.contains("fr init zsh"), "{err}");
    }

    #[test]
    fn every_command_name_is_reserved_so_workflows_cannot_shadow_it() {
        for name in [
            "list", "edit", "rm", "run", "record", "status", "export", "save", "suggest", "resume",
            "shim",
        ] {
            assert!(is_reserved(name), "{name}");
        }
        assert!(!is_reserved("deploy"));
    }
}
