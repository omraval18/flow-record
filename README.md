# fr

[![CI](https://github.com/omraval18/flow-record/actions/workflows/ci.yml/badge.svg)](https://github.com/omraval18/flow-record/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/omraval18/flow-record)](https://github.com/omraval18/flow-record/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Record terminal commands once, replay them as parameterized workflows.**

Stop writing throwaway shell scripts. Run `fr`, do the thing once, run `fr` again — and
`fr <name>` repeats it anywhere, with the bits that change as arguments. Forgot to start
recording? `fr save` turns what you just ran into a workflow, and `fr suggest` spots the
sequences you keep typing. Works in **zsh, bash and fish**.

```console
$ fr                                    # start recording
$ pnpm dlx create-next-app dashboard
$ cd dashboard
$ pnpm add zustand
$ fr                                    # stop, review, save

  ? Make dynamic  ↑↓ move · space select · a all · enter confirm
    ❯ [1] dashboard  used in 2 steps
  ? Name (next) ›
  ? Save to (p)roject = this repo · (g)lobal = everywhere  [global] ›

  ✓ Saved next (global)
    Run   fr next <name>

$ fr next my-new-app                    # later, in any terminal
```

`zustand` appeared once, so it stays constant. `dashboard` appeared twice, so you were offered
it as an argument. You answered two questions; there is no script to edit.

## Why fr

- **Replay happens in your shell, not a subprocess.** `cd`, `export` and `source` persist
  after the workflow finishes, which is exactly what a script can't do. Replayed steps land
  in your shell history, so ↑ and Ctrl-R find them.
- **No need to plan ahead.** `fr save` builds a workflow from commands you already ran;
  `fr suggest` finds sequences you've typed three or more times and offers the values that
  changed between them as arguments.
- **Workflows that adapt.** Arguments can be limited to a set of choices (with a picker and
  tab completion), defaults can be computed (`$(git branch --show-current)`), steps can be
  allowed to fail, and workflows can call other workflows.
- **Failures are resumable.** When step 3 of 5 fails, fix the problem and run `fr resume`:
  it continues from step 3 with the same arguments.
- **Arguments are quoted for where they land.** A value like `it's $HOME` is escaped
  correctly inside bare words, `'single'` and `"double"` quotes. Shell pipes, redirects and
  heredocs are preserved, never string-replaced.
- **Project-aware.** Workflows live per repository (`.fr/`, shared with your team) or
  globally. The same name can mean different things in different projects.
- **Safe by default.** Failed commands are excluded, destructive steps need a typed
  confirmation, and a workflow arriving via `git pull` must be approved before it runs.
- **Small and fast.** One 700–800 KB binary, no runtime, startup in about a millisecond.
- **macOS and Linux**, on both arm64 and x86_64.

## Install

### 1. Install fr

**Prebuilt binary** (recommended):

```sh
curl -fsSL https://raw.githubusercontent.com/omraval18/flow-record/main/install.sh | sh
```

The script picks the right build for your machine, verifies its SHA-256 checksum and installs a
single file to `~/.local/bin` (make sure that's on your `PATH`). Options:
`FR_INSTALL_DIR=/usr/local/bin` to install elsewhere, `FR_VERSION=v0.1.1` to pin a version.

| Platform | Build | Requirements |
|---|---|---|
| macOS, Apple silicon | `aarch64-apple-darwin` | |
| macOS, Intel | `x86_64-apple-darwin` | |
| Linux, x86_64 | `x86_64-unknown-linux-gnu` | glibc 2.34+ (Ubuntu 22.04+, Debian 12+, Fedora 35+, RHEL 9+) |
| Linux, arm64 | `aarch64-unknown-linux-gnu` | glibc 2.34+ |

Binaries are also on the [releases page](https://github.com/omraval18/flow-record/releases/latest).
On older distributions, Alpine (musl), or anything else, build from source:

```sh
cargo install --git https://github.com/omraval18/flow-record
# or, from a clone:
cargo install --path .
```

### 2. Enable the shell integration

| Shell | Add to | Line |
|---|---|---|
| zsh | `~/.zshrc` (after `compinit`) | `eval "$(fr init zsh)"` |
| bash 3.2+ | `~/.bashrc` | `eval "$(fr init bash)"` |
| fish 3.1+ | `~/.config/fish/config.fish` | `fr init fish \| source` |

Open a new terminal and run `fr --help`. The integration defines the `fr` function (replay
has to run inside your shell), completion, the recording indicator and the history hooks.

## Usage

### Create

| | |
|---|---|
| `fr` | Start recording. A red `● rec` appears in your prompt. Run `fr` again to stop. |
| `fr record` / `fr stop` | The same, explicitly. `fr status` says which state you're in. |
| `fr save 3` | Save the last 3 commands you ran in this shell as a workflow. |
| `fr save` | Pick steps from your recent commands in a checkbox list. |
| `fr suggest` | Show command sequences you keep repeating here, and save one. `fr list` tells you when there are some. |

Every path ends in the same short review: failed commands are already excluded, values that
repeat (or that changed each time, for `fr suggest`) are offered as arguments in a checkbox
list — the order you pick them is the argument order — and you choose a name and a scope.
Enter accepts the defaults.

### Run

```sh
fr next my-new-app          # run; a unique prefix works too:  fr ne my-new-app
fr next                     # missing arguments are prompted for
fr -n next my-new-app       # dry run: preview only
```

Press `fr <TAB>` for a list ranked for the current folder: project workflows first, then by
how recently and how often you used them there. An ambiguous prefix opens the same ranking
as a picker.

| Flag | Meaning |
|---|---|
| `-n`, `--dry-run` | Show the commands that would run; run nothing, prompt for nothing |
| `-y`, `--yes` | Skip the destructive-step confirmation (never the untrusted-workflow one) |
| `-g`, `--global` / `-p`, `--project` | Pick the workflow from one scope when both define the name |
| `--from <n>` | Start at step *n* |

If a step fails, fr stops and tells you which one. Fix the problem and run **`fr resume`**:
it continues from the failed step, in the same shell, with the same argument values (and
without asking again for the ones you typed in).

A step can run another workflow — record `fr build` like any other command, or write it in a
step — so small workflows compose into bigger ones.

**Shims.** `fr shim next` lets you type `next my-app` instead of `fr next my-app`, in this
shell and every new one. `fr shim` lists them, `fr unshim next` removes one. fr warns you
when a shim hides a real command.

### Manage

| | |
|---|---|
| `fr list` (`ls`) | Workflows visible here, ranked; shows which global ones a project hides |
| `fr show <name>` | Steps and parameters |
| `fr edit <name>` | Rename, move between project and global, set defaults and choices, add/rename parameters, reorder or edit steps, mark steps as allowed to fail. Nothing is written until you save. |
| `fr edit -e <name>` | Open the raw TOML in `$VISUAL` / `$EDITOR` |
| `fr rm <name>` | Delete |
| `fr export <name>` | Print a standalone bash script; parameters become positional arguments |
| `fr shim [name…]` / `fr unshim <name…>` | Run workflows without the `fr` prefix |

## Scopes

| Scope | Stored in | Visible |
|---|---|---|
| **project** | `<repo>/.fr/workflows/` | from that directory and everything below it; the nearest `.fr/` wins |
| **global** | `~/.config/fr/workflows/` | everywhere |

When a recording starts inside a git repository the default is *project*, otherwise *global*.
A project workflow hides a global one with the same name (`fr list` says so; `fr -g name`
reaches the hidden one). Commit `.fr/` to share workflows with your team.

## Safety

- **Trust on first use.** A project workflow you didn't create is untrusted until you
  approve it, and again whenever its content changes. Approvals are bound to a SHA-256 of
  the file *and* its path. Workflows you record or edit yourself are trusted automatically.
- **Destructive steps** (`rm -rf`, `git push --force`, `git reset --hard`, `sudo`,
  `DROP TABLE`, …) require typing the workflow's name, and are never run via a prefix
  match. This is a speed bump for your own recordings, not a sandbox.
- **No partial-success claims.** If step 2 of 3 fails, replay stops and says so; earlier
  steps are not rolled back.
- **Computed defaults run only after approval.** A project workflow's `$(…)` defaults are
  not executed until you've trusted it, and never during `fr -n`.
- **Secrets are your responsibility** for now: values typed into commands are stored as
  typed. Pass secrets as arguments or read them from the environment.
- **Command history.** To power `fr save` and `fr suggest`, the shell hooks append each
  command you type to `~/.local/state/fr/history` (yours only, trimmed to the newest 10 000
  lines). It's like your shell's own history file, plus the exit status and directory. Set
  `FR_NO_HISTORY=1` to turn it off. In bash, commands kept out of bash's history
  (`HISTCONTROL=ignorespace`) are not logged either.

## Workflow files

Plain TOML, easy to review and diff:

```toml
version = 1
description = "deploy {{env}} (+3 more)"
steps = [
    "fr build",                         # another workflow
    "-pnpm lint",                       # leading "-": may fail, the workflow carries on
    "docker build -t app:{{tag}} .",
    "./deploy.sh {{env}} app:{{tag}}",
]

[[params]]
name = "env"
choices = ["staging", "prod"]           # a picker when omitted; anything else is refused

[[params]]
name = "tag"
default = "$(git rev-parse --short HEAD)" # computed each run unless you pass a value
example = "a1b2c3d"                     # shown as a hint
```

Steps are the raw shell text you typed, with `{{name}}` marking each argument. Templates
that aren't valid parameter names (Go templates like `{{.Names}}`) are left alone.
Workflows recorded in fish carry `shell = "fish"` and run only in fish; zsh and bash share
workflows.

## How it works

1. `fr init <shell>` defines an `fr` shell function and hooks that run after each command
   (zsh `preexec`/`precmd`, bash `PROMPT_COMMAND`, fish `fish_preexec`/`fish_postexec`).
   They capture the command as typed — nothing is scraped from the screen — and append it
   to the history log with shell builtins only, so a prompt costs no extra process. While
   recording, commands also go to a per-shell session log, so two terminals never
   interleave.
2. On stop, the log becomes a workflow: repeated whole-word literals are suggested as
   parameters and replaced in a single pass.
3. On replay the binary renders the steps and prints **one script** to stdout, quoted for
   your shell's language; the shell function `eval`s it in your current shell. All prompts
   and previews use stderr, so the script is the only thing captured. When a step fails,
   the script leaves the failed step and the argument values in shell variables, which is
   all `fr resume` needs.

## Development

```sh
cargo test                                   # unit + end-to-end (real binary, real zsh/bash/fish when installed)
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

The crate is a library plus a thin binary. Logic is kept free of globals: the filesystem is
reached through a `Store` (directories and cwd passed in), the terminal through the `Ui`
trait, and output through a `Write` — so flows like recording review and `fr edit` are
tested with a scripted `Ui` and temp directories, not a TTY. See `src/lib.rs` for the module
map.

## Status

Early: v0.1.1. Runs on macOS and Linux (arm64 and x86_64) in zsh, bash and fish; CI tests
every change on both, driving the real shells. Not yet: a static musl Linux build, workflow
sharing/import, secret redaction.

## License

[MIT](LICENSE)
