# fr reference

Everything fr does, in one place. For a quick start see the [README](../README.md).

- [Commands](#commands)
- [Creating workflows](#creating-workflows)
- [Running workflows](#running-workflows)
- [Parameters](#parameters)
- [Steps](#steps)
- [Editing](#editing)
- [Scopes and name resolution](#scopes-and-name-resolution)
- [Shims](#shims)
- [Completion](#completion)
- [Safety](#safety)
- [Workflow file format](#workflow-file-format)
- [Shells](#shells)
- [Files and directories](#files-and-directories)
- [Environment variables](#environment-variables)
- [Exit status](#exit-status)

## Commands

| Command | What it does |
|---|---|
| `fr` | Start recording; run it again to stop and save. |
| `fr record` (`start`) | Start recording. |
| `fr stop` | Stop recording and review it. |
| `fr status` | Print `recording` or `not recording`. |
| `fr save [n]` | Save commands already run in this shell: the last *n*, or pick from the last 15. |
| `fr suggest` | List command sequences you keep repeating here and save one. |
| `fr <name> [args…]` | Run a workflow. A unique prefix of the name works. |
| `fr run <name> [args…]` | Same as `fr <name>`; useful in scripts for clarity. |
| `fr resume` | Continue the last failed workflow in this shell from the step that failed. |
| `fr list` (`ls`) | Workflows visible from here, most relevant first. |
| `fr show <name>` | Print a workflow's steps and parameters. |
| `fr edit <name>` | Change a workflow interactively. |
| `fr edit -e <name>` (`--file`) | Open the workflow file in `$VISUAL` / `$EDITOR` (default `vi`). |
| `fr rm <name>` (`delete`, `remove`) | Delete a workflow, after confirmation. |
| `fr export <name>` | Print a standalone bash script equivalent to the workflow. |
| `fr shim [name…]` | Let workflows run without the `fr` prefix; no names lists current shims. |
| `fr unshim <name…>` | Remove shims. |
| `fr init zsh\|bash\|fish` | Print the shell integration (see [Shells](#shells)). |
| `fr help` (`-h`, `--help`) | Usage summary. `fr <command> --help` prints the same. |
| `fr version` (`-V`, `--version`) | Print the version. |

Recording (`fr`, `record`, `stop`, `status`), replay, `save` and `resume` need the shell
integration, because they change or read the state of your running shell; a new shim takes
effect in a shell that loaded it. `list`, `show`, `edit`, `rm`, `export`
and `suggest` also work from the plain binary.

## Creating workflows

### Recording

Run `fr`, do the work, run `fr` again. While recording, a red `● rec` shows in your prompt
(zsh: right prompt; bash: start of `PS1`; fish: right prompt). Each terminal records on its
own, so two recordings never mix.

What is recorded:

- every command as you typed it (multi-line commands, heredocs and pipes included);
- its exit status: failed commands are listed and **left out**; status `130` (stopped
  with Ctrl-C, like a dev server) counts as success;
- `fr <workflow>` and `fr run <workflow>` are kept, so a workflow can call another.

What is not: `exit`, `logout`, blank lines, and commands that control fr itself (`fr stop`,
`fr list`, `fr -n …`, …).

### After the fact: `fr save`

`fr save 3` takes the last three successful commands typed in **this** shell. `fr save`
alone shows the last 15 in a checkbox list; the ones you tick become the steps, in the order
they ran. This reads the [command history](#command-history), so it works even if you never
started a recording.

### From habits: `fr suggest`

fr looks through the command history for sequences of 2–5 commands that you typed in the
same order at least three times. "Same" means same programs, subcommands and flags — the
values may differ, so `git commit -m "fix"` and `git commit -m "docs"` match. It ignores:

- sequences that only move around (`cd`, `ls`, `pwd`, `cat`, `git status`, `git diff`,
  `git log`, …);
- sequences an existing workflow already contains;
- a short sequence that is mostly part of a longer one;
- commands that failed.

Inside a repository only commands typed under that repository count; elsewhere, all of
them. Commands from one shell form a run, broken after an hour of inactivity. The five best
are shown with how many times you typed them; pick one to save. Values that differed
between occurrences are offered first as arguments ("changed every time you ran this").
`fr list` mentions when there are habits to save.

### The review

`fr stop`, `fr save` and `fr suggest` end with the same questions:

1. **Values that might change** — a checkbox list of candidate arguments: whole words or
   `--flag=value` values of at least three characters that appear in two or more steps
   (up to eight), plus, for `fr suggest`, values that changed between runs. The order you
   tick them is the argument order. With several picks you are asked to name each one;
   the defaults are `name`, `arg2`, `arg3`…
2. **Name** — suggested from the steps (`create-next-app` → `next`, `cargo new` →
   `cargo-new`).
3. **Save to** — `p`roject or `g`lobal. The default is project inside a git repository or
   a folder with `.fr/`, global elsewhere.

If the name is taken you are asked whether to overwrite. Enter accepts every default.
Ctrl-D or Ctrl-C cancels without writing anything.

Checkbox keys: `↑`/`↓` or `k`/`j` move, `space` toggles, `1`–`9` toggle that row, `a`
toggles all, `enter` confirms, `esc`/`q` picks nothing, `Ctrl-C` cancels. Without a
terminal (piped input) you type row numbers instead (`1 3`, `a` for all, Enter for none).

## Running workflows

```sh
fr deploy prod              # run
fr dep prod                 # a unique prefix works
fr deploy                   # missing required arguments are asked for
fr -n deploy prod           # dry run: show the steps, run nothing, ask nothing
fr --from 3 deploy prod     # start at step 3
```

| Flag | Meaning |
|---|---|
| `-n`, `--dry-run` | Preview only. Computed defaults are shown, not run. |
| `-y`, `--yes` | Skip the destructive-step confirmation. Never skips the untrusted-workflow one. |
| `-g`, `--global` | Use the global workflow of that name (reaches one hidden by a project workflow). |
| `-p`, `--project` | Use the project workflow of that name. |
| `--from <n>`, `--from=<n>` | Start at step *n*. Step numbers stay as in the full workflow. |

Flags go **before** the name; everything after the name is an argument, even if it starts
with `-`.

Before running, fr prints a preview of the exact commands with arguments filled in. The
steps then run **in your current shell**: `cd`, `export`, `source`, aliases and functions
behave as if you had typed them. Each step is also added to your shell's history, so ↑ and
Ctrl-R find it.

### When a step fails

Steps run in order and replay stops at the first failure (unless the step is
[allowed to fail](#steps)):

```text
fr: step 3/5 failed (earlier steps already ran; fix it, then run: fr resume)
```

The shell's `$?` is the failed command's status. Earlier steps are **not** undone.

`fr resume` runs the same workflow again from the failed step, with the same argument
values — including ones you were prompted for and computed defaults — so nothing is asked
twice. It works in the shell where the failure happened (the state is kept in the shell
variables `FR_RESUME` and `FR_RESUME_STEP`, never on disk). After a successful run there is
nothing to resume.

### Workflows that call workflows

A step may be `fr <other> [args]`. It runs through the same shell function, with its own
preview, prompts and confirmations, and a failure inside it fails the calling step.
Workflows nested more than 16 deep are refused, which stops a workflow that calls itself.

## Parameters

A parameter is a slot filled at run time; steps refer to it as `{{name}}`. Arguments are
matched to parameters in order. For each parameter fr uses, in this order:

1. the argument you passed;
2. its `default` — a fixed value, or a **computed** one (below);
3. otherwise it asks you. A parameter with `choices` shows a numbered list; answer with a
   number or the value.

**Choices.** `choices = ["staging", "prod"]` accepts only those values, from arguments,
prompts, defaults and completion alike. Anything else is an error that lists the choices.

**Computed defaults.** A default containing `$(…)` is expanded by a shell each time the
workflow runs without that argument: `default = "$(git branch --show-current)"` or
`default = "release-$(date +%F)"`. It runs in the current directory with `sh` (fish
workflows: `fish`), sees exported environment variables only, and its output is used as the
value (and checked against `choices`). If the command fails, the run stops and names the
parameter. Computed defaults are never run during `fr -n`, and for a project workflow not
until you have approved it.

**Quoting.** Values are escaped for the position they land in — a bare word, inside
`'single'` quotes or inside `"double"` quotes — in the workflow's shell language. A value
like `it's $HOME` arrives intact and never runs as code.

Parameter names use letters, digits and `_`, starting with a letter or `_`. Text like
`{{.Names}}` (Go/Helm templates) is not a placeholder and is left alone.

## Steps

A step is shell text exactly as you would type it; it can span several lines.

**Allowed to fail.** A step starting with `-` (as in make) may fail: replay reports
`step 2/5 failed with status 1 (allowed to fail, continuing)` and goes on. Toggle it in
`fr edit` → steps → `fail <n>`, or add/remove the `-` in the file.

## Editing

`fr edit <name>` shows the workflow and a menu. Changes are kept in memory until you save
(`s` or Enter); `q` asks before discarding them.

| Key | Menu | Commands |
|---|---|---|
| `1` | name | Type a new name. |
| `2` | scope | Move between project and global (offers to create `.fr/` if needed). |
| `3` | description | Type a new description. |
| `4` | parameters | `add <text> [name]` — turn text found in the steps into a parameter · `rename <old> <new>` · `default <name> [value]` — set, or clear without a value; `$(…)` is computed · `choices <name> [a b …]` — restrict values, or lift the restriction without values · `constant <name> [value]` — put fixed text back (defaults to the default or example) |
| `5` | steps | `rm <n>` · `mv <from> <to>` · `set <n> <command…>` · `fail <n>` — toggle "allowed to fail" |
| `6` | editor | Save, then open the file in your editor. |

On save, parameters no step uses are dropped, every placeholder must have a parameter, and
a default must be one of the choices. Renaming or moving refuses to overwrite another
workflow. `fr edit -e` skips the menu and opens the file; it is checked for mistakes when
the editor closes.

## Scopes and name resolution

| Scope | Stored in | Visible |
|---|---|---|
| project | `<repo>/.fr/workflows/<name>.toml` | in that folder and below; the nearest `.fr/` wins |
| global | `$XDG_CONFIG_HOME/fr/workflows/<name>.toml` | everywhere |

A project workflow hides a global one with the same name; `fr list` shows the hidden one
and `fr -g <name>` runs it. Commit `.fr/` to share a project's workflows.

A name given to `fr <name>`, `show`, `edit`, `rm` and `export` resolves as: exact name →
the only name starting with it → if several start with it, a ranked picker. If nothing
matches, names containing it are suggested.

**Ranking** (`fr list`, completion, the picker): project workflows first, then most recently
used here, then most used here, then alphabetical. "Here" is the project, or "outside any
project", so usage in one repository does not reorder another.

**Names** are lowercase letters, digits, `-` and `_`, start with a letter or digit, are at
most 48 characters, and cannot be a command name (`list`, `run`, `save`, …).

## Shims

`fr shim next` defines a shell function `next` that runs `fr next "$@"`, in the current
shell and every new one (`fr init` includes it). It resolves the name when run, so a shim
for a project workflow works inside that project. fr warns when a shim hides a program on
your `PATH`; that program is still reachable as `command next`. `fr shim` lists shims,
`fr unshim next` removes one (also from the current shell).

## Completion

`fr <TAB>` offers workflows ranked for this folder (with description and scope), then
commands. After `fr <workflow>` it shows which argument comes next (`<env> one of: dev,
prod`, `[port=3000]`, `<name> e.g. demo`) and completes the allowed values of a parameter
with choices. `fr show|edit|rm|export|run|shim <TAB>` completes workflow names; `fr unshim
<TAB>` completes shims. Available in zsh (menu selection), bash and fish.

## Safety

- **Trust on first use.** A project workflow you didn't record or edit (it arrived with
  `git pull`, say) is shown and must be approved before it runs, and again whenever its file
  changes. Approval is bound to a SHA-256 of the content *and* the file's path. `-y` does
  not skip this. Global workflows are yours and need no approval.
- **Destructive steps** need you to type the workflow's name, and are never run through a
  prefix match. Flagged: `rm -r`/`-f`, `sudo`, `doas`, `dd of=`, `mkfs*`, `shred`, `wipefs`,
  any `--force` outside git, `git push --force`/`-f`/`--delete`, `git reset --hard`,
  `git clean -f`, `git branch -D`, `git checkout`/`restore` with `--force`, `-f` or `.`,
  and SQL `DROP TABLE|DATABASE|SCHEMA` (also inside `psql -c '…'` and other SQL clients).
  This is a speed bump for your own recordings, not a sandbox.
- **Computed defaults** wait for approval and never run in a dry run.
- **Secrets** typed into commands are stored as typed. Pass them as arguments or read them
  from the environment.

### Command history

To power `fr save` and `fr suggest`, the shell hooks append every command you type to
`$XDG_STATE_HOME/fr/history`: time, shell process id, exit status, directory and the
command. It is written with shell builtins only (no extra process per prompt), stays on
your machine like your shell's own history file, and is trimmed to the newest 10 000 lines
once it passes 2 MiB. Set `FR_NO_HISTORY=1` to stop logging; delete the file
to forget it. Replayed steps are not logged as typed commands.

## Workflow file format

```toml
version = 1                       # format version; files from a newer fr are refused
description = "deploy {{env}}"    # shown in list and completion
shell = "fish"                    # only for workflows recorded in fish
steps = [
    "fr build",                   # runs another workflow
    "-pnpm lint",                 # may fail
    "./deploy.sh {{env}} {{tag}}",
]

[[params]]                        # in argument order
name = "env"
choices = ["staging", "prod"]     # optional: the only accepted values
example = "staging"               # optional: shown as a hint

[[params]]
name = "tag"
default = "$(git rev-parse --short HEAD)"   # optional; $(…) is computed each run
```

| Field | Required | Meaning |
|---|---|---|
| `version` | no (1) | File format version. |
| `description` | no | One line for listings. |
| `shell` | no (zsh/bash) | `"fish"` for fish workflows. |
| `steps` | yes, ≥ 1 | Shell text; `{{param}}` placeholders; a leading `-` allows failure. |
| `params[].name` | yes | Letters, digits, `_`. Unique. |
| `params[].default` | no | Makes the argument optional. Contains `$(…)` → computed. |
| `params[].choices` | no | Allowed values, no duplicates; a fixed default must be one of them. |
| `params[].example` | no | Hint shown when asking and in completion. |

`fr export` turns a zsh/bash workflow into a bash script with `set -euo pipefail`:
parameters become positional arguments (`$1`, `$2`, …), defaults apply when an argument is
missing (computed ones run when the script does), choices are checked, and steps allowed to
fail are wrapped so they don't stop the script. A step that runs another workflow (`fr x`)
needs fr and its shell integration where the script runs. fish workflows can't be exported.

## Shells

| | zsh | bash | fish |
|---|---|---|---|
| Version | any recent | 3.2+ | 3.1+ |
| Setup | `eval "$(fr init zsh)"` in `~/.zshrc`, after `compinit` | `eval "$(fr init bash)"` in `~/.bashrc` | `fr init fish \| source` in `~/.config/fish/config.fish` |
| Hooks | `preexec` + `precmd` | `PROMPT_COMMAND` (reads the command back from history) | `fish_preexec` + `fish_postexec` |
| Recording indicator | right prompt | start of `PS1` | right prompt |
| Steps added to history | yes | yes | fish 4+ |
| Timestamps in the history log | yes | bash 5+ | no |

zsh and bash share workflows; workflows recorded in fish are marked `shell = "fish"`, are
quoted with fish rules, and run only in fish (and the other way round).

bash notes: commands bash keeps out of its history (`HISTCONTROL=ignorespace`,
`ignoredups` repeats, `set +o history`) are not seen by fr either. The integration prepends
itself to `PROMPT_COMMAND` and does not need bash-preexec.

After upgrading fr, open a new terminal so the shell loads the matching integration.

## Files and directories

| Path | Contents |
|---|---|
| `<repo>/.fr/workflows/*.toml` | Project workflows (commit these). |
| `$XDG_CONFIG_HOME/fr/workflows/*.toml` | Global workflows. Default `~/.config/fr/…`. |
| `$XDG_CONFIG_HOME/fr/shims` | Shim names, one per line. |
| `$XDG_STATE_HOME/fr/history` | Command history (see above). Default `~/.local/state/fr/…`. |
| `$XDG_STATE_HOME/fr/usage` | When and where each workflow ran, for ranking (trimmed to the latest 500 runs past 64 KiB). |
| `$XDG_STATE_HOME/fr/trusted` | Approved project workflows (hash and path). |
| `$XDG_STATE_HOME/fr/sessions/` | Logs of recordings in progress; removed on stop. |

## Environment variables

| Variable | Effect |
|---|---|
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | Where config and state live (defaults `~/.config`, `~/.local/state`). |
| `FR_NO_HISTORY` | When set, the hooks stop writing the command history. |
| `VISUAL`, `EDITOR` | Editor for `fr edit -e` (default `vi`). |
| `NO_COLOR` | Disable colour. Colour is also off when output is not a terminal. |
| `FR_RESUME`, `FR_RESUME_STEP` | Set by a failed run, read by `fr resume` (shell variables, per shell). |
| `FR_INSTALL_DIR`, `FR_VERSION`, `FR_REPO`, `FR_BASE_URL` | `install.sh` only: install location, version to pin, source repository, download mirror. |

## Exit status

`fr` exits `0` on success and `1` on an error or when you cancel a prompt (`Cancelled.`).
A replayed workflow leaves the status of the step that failed. Closing a pipe early
(`fr list | head`) is not an error.
