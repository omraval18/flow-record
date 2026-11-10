# Changelog

All notable changes to fr. The format follows [Keep a Changelog](https://keepachangelog.com),
and versions follow [Semantic Versioning](https://semver.org).

## [Unreleased]

## [0.2.0] — 2026-11-11

### Added

- **bash and fish support.** `fr init bash` (bash 3.2+) and `fr init fish` (fish 3.1+)
  alongside zsh, with recording, replay, completion and the recording indicator. Workflows
  recorded in fish are marked `shell = "fish"` and quoted with fish rules.
- **`fr save [n]`** turns commands you already ran in this shell into a workflow — the last
  *n*, or picked from the last 15.
- **`fr suggest`** finds command sequences you typed three or more times and saves one; the
  values that changed between runs are offered as arguments. `fr list` mentions them.
- **Command history** written by the shell hooks with builtins only, powering `save` and
  `suggest`. Trimmed automatically; `FR_NO_HISTORY=1` turns it off.
- **`fr resume`** continues the last failed workflow from the failed step with the same
  argument values; **`--from <n>`** starts any workflow at step *n*.
- **Choices**: `choices = [...]` restricts a parameter, with a numbered picker, tab
  completion and validation (also in exported scripts).
- **Computed defaults**: a default containing `$(…)` is expanded each run — only after a
  project workflow is approved, and never in a dry run. A failing command stops the run.
- **Steps allowed to fail**: a leading `-` (as in make); `fr edit` → steps → `fail <n>`.
- **Workflows calling workflows**: recordings keep `fr <workflow>` steps; nesting deeper
  than 16 is refused.
- **`fr shim` / `fr unshim`**: run a workflow without the `fr` prefix.
- Replayed steps are added to the shell's history.
- `fr edit` → parameters → `choices <name> [a b …]`.
- [Reference documentation](docs/reference.md).

### Changed

- The installer's last hint shows the setup line for your shell.
- The shell integration calls `fr _exec`; open a new terminal after upgrading.

## [0.1.1] — 2026-11-09

First public release.

### Added

- Record with `fr` … `fr`, replay with `fr <name>` in the current shell.
- Repeated values offered as arguments in a checkbox list; shell-aware quoting.
- Project (`.fr/`) and global scopes, prefix matching, ranked completion.
- `list`, `show`, `edit` (interactive and `-e`), `rm`, `export`.
- Trust on first use for project workflows; typed confirmation for destructive steps.
- Prebuilt binaries for macOS and Linux (x86_64 and arm64, glibc 2.34+) and `install.sh`.

[Unreleased]: https://github.com/omraval18/flow-record/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/omraval18/flow-record/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/omraval18/flow-record/releases/tag/v0.1.1
