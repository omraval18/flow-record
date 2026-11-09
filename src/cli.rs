//! Argument parsing and dispatch.
//!
//! Parsing is hand-written on purpose: the grammar is tiny (`fr <command> …` or
//! `fr [flags] <workflow> [args…]`), and `fr` runs on every prompt while recording, so a
//! sub-millisecond start and a small binary matter more than derive-generated help.

use crate::run::Options;
use crate::store::Store;
use crate::ui::{Pal, Ui};
use crate::{edit, manage, record, run, session, Error, Result};
use std::io::Write;
use std::path::PathBuf;

const ZSH_INTEGRATION: &str = include_str!("../shell/fr.zsh");

pub const USAGE: &str = "fr — record terminal workflows once, replay them anywhere

USAGE
  fr                          Start recording; run again to stop and save
  fr <name> [args…]           Run a workflow (a unique prefix works: fr ne)

COMMANDS
  record, stop, status        Control recording
  run <name> [args…]          Run a workflow (same as fr <name>)
  list, ls                    Workflows for this folder, most relevant first
  show <name>                 Print a workflow
  edit <name>                 Change its name, scope, parameters and steps
  rm <name>                   Delete a workflow
  export <name>               Print a portable bash script
  init zsh                    Print the shell integration

RUN FLAGS
  -n, --dry-run               Preview only
  -y, --yes                   Skip the destructive-step confirmation
  -g, --global                Use the global workflow
  -p, --project               Use the project workflow

EDIT FLAGS
  -e, --file                  Open the raw TOML in $EDITOR

SETUP
  Add to ~/.zshrc (after compinit):   eval \"$(fr init zsh)\"
";

/// Built-in command names; workflows may not use them.
const COMMANDS: &[&str] = &[
    "record", "start", "stop", "status", "run", "list", "ls", "show", "edit", "rm", "delete",
    "remove", "export", "init", "version", "help",
];

pub fn is_reserved(name: &str) -> bool {
    COMMANDS.contains(&name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Record,
    Stop {
        session: PathBuf,
    },
    Run(Options),
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
            "record" | "start" => Self::Record,
            "stop" => Self::Stop {
                session: PathBuf::from(one("session file")?),
            },
            "run" | "_run" => Self::Run(Options::parse(rest)?),
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
            "edit" => parse_edit(rest)?,
            "init" => match rest {
                [shell] if shell == "zsh" => Self::Init(Shell::Zsh),
                _ => return Err(Error::usage("usage: fr init zsh")),
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
            _ => {
                return Err(Error::usage(
                    "this needs the shell integration — add `eval \"$(fr init zsh)\"` to ~/.zshrc",
                ))
            }
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
        Command::Init(Shell::Zsh) => out.write_all(ZSH_INTEGRATION.as_bytes())?,
        Command::Record => {
            let path = session::create(&store.dirs().sessions(), store.cwd())?;
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
        Command::List => manage::list(store, Pal::stdout(), out)?,
        Command::Show { name } => manage::show(store, ui, Pal::stdout(), &name, out)?,
        Command::Edit { name, raw_file } => edit::edit(store, ui, &name, raw_file)?,
        Command::Remove { name } => manage::remove(store, ui, &name)?,
        Command::Export { name } => manage::export(store, ui, &name, out)?,
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
        assert_eq!(parse(&["record"]).unwrap(), Command::Record);
        assert_eq!(parse(&["start"]).unwrap(), Command::Record);
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
        for name in ["list", "edit", "rm", "run", "record", "status", "export"] {
            assert!(is_reserved(name), "{name}");
        }
        assert!(!is_reserved("deploy"));
    }
}
