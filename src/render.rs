//! Shell-aware handling of `{{param}}` placeholders.
//!
//! Steps are stored as the raw shell text the user typed. Substitution never does a
//! blind string replace: it tracks quote state, so a value is escaped for the context it
//! lands in (bare word, `'single'` or `"double"` quotes) and cannot change the meaning of
//! the surrounding command.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;

/// The shell language steps are written in. zsh and bash share POSIX quoting and control
/// flow; fish has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    #[default]
    Posix,
    Fish,
}

impl Dialect {
    pub const fn is_posix(&self) -> bool {
        matches!(self, Self::Posix)
    }
}

/// Quoting context at a position inside a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quote {
    Bare,
    Single,
    Double,
}

/// Whether `s` is a valid parameter name: `[A-Za-z_][A-Za-z0-9_]*`.
pub fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse a `{{name}}` placeholder starting at byte `start`; returns the name and the
/// byte index just past the closing braces. Foreign templates like `{{.Names}}` (Go
/// templates, Helm, Jinja…) are not placeholders and yield `None`.
fn placeholder_at(raw: &str, start: usize) -> Option<(&str, usize)> {
    let inner = raw.get(start + 2..)?;
    let len = inner.find("}}")?;
    let name = inner[..len].trim();
    is_ident(name).then_some((name, start + 2 + len + 2))
}

/// Names of every placeholder in `raw`, in order of appearance.
pub fn placeholders(raw: &str) -> impl Iterator<Item = &str> {
    let mut at = 0;
    std::iter::from_fn(move || {
        while let Some(found) = raw[at..].find("{{") {
            let start = at + found;
            match placeholder_at(raw, start) {
                Some((name, end)) => {
                    at = end;
                    return Some(name);
                }
                None => at = start + 2,
            }
        }
        None
    })
}

/// [`substitute_in`] for POSIX shells.
pub fn substitute(raw: &str, emit: impl FnMut(&str, Quote) -> Option<String>) -> String {
    substitute_in(Dialect::Posix, raw, emit)
}

/// Copy `raw`, replacing each placeholder with `emit(name, quote_context)`.
/// A placeholder for which `emit` returns `None` is left untouched.
pub fn substitute_in(
    dialect: Dialect,
    raw: &str,
    mut emit: impl FnMut(&str, Quote) -> Option<String>,
) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let (mut quote, mut i, mut copied) = (Quote::Bare, 0, 0);
    // Only ASCII bytes are inspected, and those never occur inside a multi-byte UTF-8
    // sequence, so every index used for slicing below is a char boundary.
    while i < bytes.len() {
        if bytes[i] == b'{' && bytes.get(i + 1) == Some(&b'{') {
            if let Some((name, end)) = placeholder_at(raw, i) {
                if let Some(value) = emit(name, quote) {
                    out.push_str(&raw[copied..i]);
                    out.push_str(&value);
                    (i, copied) = (end, end);
                    continue;
                }
            }
        }
        match (quote, bytes[i]) {
            // fish lets `\'` and `\\` escape inside single quotes; POSIX does not.
            (Quote::Single, b'\\')
                if dialect == Dialect::Fish && matches!(bytes.get(i + 1), Some(b'\'' | b'\\')) =>
            {
                i += 2;
                continue;
            }
            (Quote::Single, b'\'') => quote = Quote::Bare,
            (Quote::Single, _) => {}
            (_, b'\\') => {
                i += 2; // the escaped character cannot open or close a quote
                continue;
            }
            (Quote::Bare, b'\'') => quote = Quote::Single,
            (Quote::Bare, b'"') => quote = Quote::Double,
            (Quote::Double, b'"') => quote = Quote::Bare,
            _ => {}
        }
        i += 1;
    }
    out.push_str(&raw[copied.min(raw.len())..]);
    out
}

/// [`quote_in`] for POSIX shells.
pub fn quote(value: &str, ctx: Quote) -> Cow<'_, str> {
    quote_in(Dialect::Posix, value, ctx)
}

fn escape_with_backslash<'a>(value: &'a str, special: &[char]) -> Cow<'a, str> {
    if !value.contains(special) {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len() + 4);
    for c in value.chars() {
        if special.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    Cow::Owned(out)
}

/// Escape `value` so the shell reads it back verbatim in context `ctx`.
pub fn quote_in(dialect: Dialect, value: &str, ctx: Quote) -> Cow<'_, str> {
    // `%` starts a process expansion in older fish, so it is only safe in POSIX shells.
    let safe = |c: char| {
        c.is_ascii_alphanumeric()
            || "_-./:=@+,".contains(c)
            || (c == '%' && dialect == Dialect::Posix)
    };
    match (dialect, ctx) {
        (_, Quote::Bare) if value.is_empty() => Cow::Borrowed("''"),
        (_, Quote::Bare) if value.chars().all(safe) => Cow::Borrowed(value),
        (Dialect::Posix, Quote::Bare) => Cow::Owned(format!("'{}'", value.replace('\'', r"'\''"))),
        (Dialect::Posix, Quote::Single) => Cow::Owned(value.replace('\'', r"'\''")),
        (Dialect::Posix, Quote::Double) => escape_with_backslash(value, &['\\', '"', '$', '`']),
        (Dialect::Fish, Quote::Bare) => {
            Cow::Owned(format!("'{}'", escape_with_backslash(value, &['\\', '\''])))
        }
        (Dialect::Fish, Quote::Single) => escape_with_backslash(value, &['\\', '\'']),
        (Dialect::Fish, Quote::Double) => escape_with_backslash(value, &['\\', '"', '$']),
    }
}

/// Quote each word for a bare position and join them with spaces.
pub fn words(dialect: Dialect, words: &[String]) -> String {
    words
        .iter()
        .map(|w| quote_in(dialect, w, Quote::Bare))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fill `raw` with `values`; placeholders without a value are left as written.
pub fn apply(raw: &str, values: &HashMap<String, String>) -> String {
    substitute(raw, |name, ctx| {
        values.get(name).map(|v| quote(v, ctx).into_owned())
    })
}

/// Rewrite `raw` for a standalone bash script where each parameter is a shell variable.
pub fn export_step(raw: &str) -> String {
    substitute(raw, |name, ctx| {
        Some(match ctx {
            Quote::Bare => format!("\"${name}\""),
            Quote::Double => format!("${{{name}}}"),
            Quote::Single => format!("'\"${name}\"'"),
        })
    })
}

/// A step ready to run: placeholders filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// A failure is reported and the workflow carries on.
    pub optional: bool,
}

/// Everything needed to turn rendered steps into the script the shell function `eval`s.
#[derive(Debug, Clone)]
pub struct Script<'a> {
    pub dialect: Dialect,
    pub steps: &'a [Rendered],
    /// 1-based number of `steps[0]` (greater than 1 when resuming).
    pub first: usize,
    pub total: usize,
    /// Arguments that re-run this workflow with the same values; stored on failure so
    /// `fr resume` can continue from the failed step.
    pub resume: Option<&'a [String]>,
    /// Add each step to the shell's history as it runs (via `__fr_remember`).
    pub remember: bool,
}

impl Script<'_> {
    /// Build the script. It runs **in the current shell** (so `cd`, `export` and
    /// `source` persist), stops at the first failure of a required step, says which step
    /// failed, and leaves the status in `__fr_rc`.
    ///
    /// POSIX scripts end with `return`, so the caller evaluates them inside a function
    /// with `__fr_i` and `__fr_rc` declared local. fish scripts end by calling
    /// `__fr_status`, which the fish integration defines.
    pub fn render(&self) -> String {
        match self.dialect {
            Dialect::Posix => self.posix(),
            Dialect::Fish => self.fish(),
        }
    }

    fn number(&self, i: usize) -> usize {
        self.first + i
    }

    fn posix(&self) -> String {
        let total = self.total;
        let mut s = String::new();
        for (i, step) in self.steps.iter().enumerate() {
            if i > 0 {
                s.push_str(" && ");
            }
            let n = self.number(i);
            let _ = write!(s, "__fr_i={n}");
            if self.remember {
                let _ = write!(s, " && __fr_remember {}", quote(&step.text, Quote::Bare));
            }
            if step.optional {
                let _ = write!(
                    s,
                    " && {{ {{\n{}\n}} || echo \"fr: step {n}/{total} failed with status $? (allowed to fail, continuing)\" >&2; }}",
                    step.text
                );
            } else {
                let _ = write!(s, " && {{\n{}\n}}", step.text);
            }
        }
        let mut on_failure = String::from("__fr_rc=$?; ");
        let mut hint = "earlier steps already ran";
        if let Some(args) = self.resume {
            s.push_str(" && FR_RESUME=");
            let _ = write!(
                on_failure,
                "FR_RESUME_STEP=$__fr_i; FR_RESUME={}; ",
                quote(&words(Dialect::Posix, args), Quote::Bare)
            );
            hint = "earlier steps already ran; fix it, then run: fr resume";
        }
        let _ = write!(
            s,
            " || {{ {on_failure}echo \"fr: step $__fr_i/{total} failed ({hint})\" >&2; }}\nreturn ${{__fr_rc:-0}}\n"
        );
        s
    }

    fn fish(&self) -> String {
        let total = self.total;
        let mut s = String::from("set __fr_rc 0\n");
        for (i, step) in self.steps.iter().enumerate() {
            let and = if i > 0 { "and " } else { "" };
            let n = self.number(i);
            let _ = writeln!(s, "{and}set __fr_i {n}");
            if self.remember {
                let _ = writeln!(
                    s,
                    "and __fr_remember {}",
                    quote_in(Dialect::Fish, &step.text, Quote::Bare)
                );
            }
            if step.optional {
                let _ = writeln!(
                    s,
                    "and begin\nbegin\n{}\nend\nor echo \"fr: step {n}/{total} failed with status $status (allowed to fail, continuing)\" >&2\ntrue\nend",
                    step.text
                );
            } else {
                let _ = writeln!(s, "and begin\n{}\nend", step.text);
            }
        }
        let mut on_failure = String::from("set __fr_rc $status\n");
        let mut hint = "earlier steps already ran";
        if let Some(args) = self.resume {
            s.push_str("and set -g FR_RESUME\n");
            let _ = writeln!(
                on_failure,
                "set -g FR_RESUME_STEP $__fr_i\nset -g FR_RESUME {}",
                words(Dialect::Fish, args)
            );
            hint = "earlier steps already ran; fix it, then run: fr resume";
        }
        let _ = write!(
            s,
            "or begin\n{on_failure}echo \"fr: step $__fr_i/{total} failed ({hint})\" >&2\nend\n__fr_status $__fr_rc\n"
        );
        s
    }
}

// ---- turning recorded literals into placeholders -------------------------------------

/// A word boundary for literal matching: whitespace, punctuation and path separators
/// qualify, but `-` and `_` do not, so `app` never matches inside `my-app` or `app_v2`.
fn is_boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
}

/// Byte offsets of whole-word occurrences of `needle` in `haystack`.
pub fn find_words<'a>(haystack: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    haystack.match_indices(needle).filter_map(move |(at, _)| {
        let before = haystack[..at].chars().next_back();
        let after = haystack[at + needle.len()..].chars().next();
        (!needle.is_empty() && is_boundary(before) && is_boundary(after)).then_some(at)
    })
}

/// Replace whole-word occurrences of each `(literal, param)` pair with `{{param}}` in a
/// single pass, so text inserted for one pair is never rescanned for another. When
/// matches overlap, the earlier (then longer) one wins.
pub fn replace_words(text: &str, pairs: &[(&str, &str)]) -> String {
    let mut hits: Vec<(usize, usize, &str)> = pairs
        .iter()
        .filter(|(literal, _)| !literal.is_empty())
        .flat_map(|&(literal, param)| {
            find_words(text, literal).map(move |at| (at, literal.len(), param))
        })
        .collect();
    hits.sort_unstable_by_key(|&(at, len, _)| (at, std::cmp::Reverse(len)));

    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (at, len, param) in hits {
        if at < last {
            continue;
        }
        out.push_str(&text[last..at]);
        out.push_str("{{");
        out.push_str(param);
        out.push_str("}}");
        last = at + len;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn values(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    }

    /// Run `script` in bash and return its stdout, so quoting is checked against a real shell.
    fn bash(script: &str) -> String {
        let out = Command::new("bash")
            .arg("-c")
            .arg(script)
            .output()
            .expect("bash is available");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn values_survive_a_real_shell_in_every_quote_context() {
        let nasty = [
            "plain",
            "my app",
            "it's",
            "$HOME `id` \"q\" \\ *",
            "",
            "ünï cødé",
            "a;b&&c|d>e",
        ];
        for value in nasty {
            for template in [
                "printf %s {{v}}",
                "printf %s \"{{v}}\"",
                "printf %s '{{v}}'",
                "printf %s pre{{v}}post",
            ] {
                let cmd = apply(template, &values(&[("v", value)]));
                let expected = match template {
                    t if t.ends_with("pre{{v}}post") => format!("pre{value}post"),
                    _ => value.to_owned(),
                };
                assert_eq!(
                    bash(&cmd),
                    expected,
                    "template {template:?} value {value:?} rendered {cmd:?}"
                );
            }
        }
    }

    #[test]
    fn leaves_foreign_templates_and_unknown_params_alone() {
        let v = values(&[("n", "x")]);
        assert_eq!(
            apply("docker ps --format '{{.Names}}' {{n}}", &v),
            "docker ps --format '{{.Names}}' x"
        );
        assert_eq!(apply("echo {{missing}}", &v), "echo {{missing}}");
        assert_eq!(
            apply("echo {{ n }}", &v),
            "echo x",
            "inner whitespace is allowed"
        );
    }

    #[test]
    fn an_escaped_quote_does_not_change_quote_state() {
        // The `\'` is a literal quote in a bare context, so the placeholder is still bare.
        assert_eq!(
            apply(r"echo \' {{v}}", &values(&[("v", "a b")])),
            r"echo \' 'a b'"
        );
    }

    #[test]
    fn empty_values_stay_an_argument() {
        assert_eq!(
            apply("ls ./{{n}}/src", &values(&[("n", "")])),
            "ls ./''/src"
        );
    }

    #[test]
    fn placeholders_lists_names_in_order() {
        let names: Vec<_> = placeholders("a {{x}} '{{y}}' {{.Skip}} {{x}}").collect();
        assert_eq!(names, ["x", "y", "x"]);
    }

    #[test]
    fn export_step_uses_shell_variables_safely() {
        let script = format!(
            "name=\"it's a test\"\n{}",
            export_step("printf %s {{name}} \"{{name}}\" '{{name}}'")
        );
        assert_eq!(bash(&script), "it's a testit's a testit's a test");
    }

    #[test]
    fn chain_runs_steps_in_the_current_shell_and_stops_at_the_first_failure() {
        let run = |steps: &[&str]| {
            let steps: Vec<Rendered> = steps
                .iter()
                .map(|s| Rendered {
                    optional: s.starts_with('-'),
                    text: s.trim_start_matches('-').to_owned(),
                })
                .collect();
            let script = Script {
                dialect: Dialect::Posix,
                steps: &steps,
                first: 1,
                total: steps.len(),
                resume: None,
                remember: false,
            }
            .render();
            let wrapper = format!(
                "f() {{ local __fr_i __fr_rc; eval \"$1\"; }}\nf '{}'\necho \"rc=$?\"",
                script.replace('\'', r"'\''")
            );
            let out = Command::new("bash")
                .args(["-c", &wrapper])
                .output()
                .unwrap();
            (
                String::from_utf8(out.stdout).unwrap(),
                String::from_utf8(out.stderr).unwrap(),
            )
        };

        let (out, err) = run(&["cd /", "pwd", "echo done"]);
        assert_eq!(out, "/\ndone\nrc=0\n", "cd must persist into later steps");
        assert!(err.is_empty());

        let (out, err) = run(&["echo one", "sh -c 'exit 7'", "echo never"]);
        assert_eq!(out, "one\nrc=7\n");
        assert!(err.contains("step 2/3 failed"), "{err}");
    }

    /// Run a POSIX script the way the shell function does, then print what `fr resume`
    /// would use.
    fn run_script(script: &Script<'_>) -> (String, String) {
        let wrapper = format!(
            "__fr_remember() {{ :; }}\nf() {{ local __fr_i __fr_rc; eval \"$1\"; }}\nf '{}'\necho \"rc=$?\"\necho \"step=$FR_RESUME_STEP\"\neval \"set -- $FR_RESUME\"\nprintf '%s|' \"$@\"",
            script.render().replace('\'', r"'\''")
        );
        let out = Command::new("bash")
            .args(["-c", &wrapper])
            .output()
            .unwrap();
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
        )
    }

    fn rendered(steps: &[(&str, bool)]) -> Vec<Rendered> {
        steps
            .iter()
            .map(|&(text, optional)| Rendered {
                text: text.into(),
                optional,
            })
            .collect()
    }

    #[test]
    fn optional_failures_are_reported_and_a_real_failure_leaves_what_resume_needs() {
        let steps = rendered(&[
            ("echo one", false),
            ("sh -c 'exit 3'", true),
            ("false", false),
            ("echo never", false),
        ]);
        let resume = ["-g".to_owned(), "w".to_owned(), "it's a value".to_owned()];
        let (out, err) = run_script(&Script {
            dialect: Dialect::Posix,
            steps: &steps,
            first: 1,
            total: 4,
            resume: Some(&resume),
            remember: true,
        });
        assert_eq!(out, "one\nrc=1\nstep=3\n-g|w|it's a value|");
        assert!(
            err.contains("step 2/4 failed with status 3 (allowed to fail"),
            "{err}"
        );
        assert!(
            err.contains("step 3/4 failed") && err.contains("fr resume"),
            "{err}"
        );
    }

    #[test]
    fn a_resumed_script_keeps_the_original_step_numbers_and_clears_resume_on_success() {
        let steps = rendered(&[("false", false), ("echo never", false)]);
        let (_, err) = run_script(&Script {
            dialect: Dialect::Posix,
            steps: &steps,
            first: 3,
            total: 4,
            resume: Some(&[]),
            remember: false,
        });
        assert!(err.contains("step 3/4 failed"), "{err}");

        let ok = rendered(&[("true", false)]);
        let (out, _) = run_script(&Script {
            dialect: Dialect::Posix,
            steps: &ok,
            first: 1,
            total: 1,
            resume: Some(&["x".to_owned()]),
            remember: false,
        });
        assert!(out.starts_with("rc=0\nstep=\n"), "{out}");
        assert!(
            !out.contains('x'),
            "a success leaves nothing to resume: {out}"
        );
    }

    #[test]
    fn fish_quoting_uses_backslash_escapes_inside_quotes() {
        let fish = |v, ctx| quote_in(Dialect::Fish, v, ctx).into_owned();
        assert_eq!(fish(r"it's \ $x", Quote::Bare), r"'it\'s \\ $x'");
        assert_eq!(fish(r"it's \", Quote::Single), r"it\'s \\");
        assert_eq!(fish(r#"a"$b\"#, Quote::Double), r#"a\"\$b\\"#);
        assert_eq!(fish("100%", Quote::Bare), "'100%'", "% is special in fish");
        assert_eq!(quote("100%", Quote::Bare), "100%");
    }

    #[test]
    fn in_fish_an_escaped_quote_does_not_end_a_single_quoted_string() {
        let ctx = |dialect| {
            let mut seen = None;
            substitute_in(dialect, r"echo 'it\'s {{v}}'", |_, q| {
                seen = Some(q);
                None
            });
            seen.unwrap()
        };
        assert_eq!(ctx(Dialect::Fish), Quote::Single);
        assert_eq!(
            ctx(Dialect::Posix),
            Quote::Bare,
            "in POSIX, \\' ends the quote"
        );
    }

    #[test]
    fn replace_words_respects_word_boundaries() {
        let pairs = [("dashboard", "name")];
        assert_eq!(
            replace_words("cd ./dashboard/src", &pairs),
            "cd ./{{name}}/src"
        );
        assert_eq!(replace_words("--name=dashboard", &pairs), "--name={{name}}");
        assert_eq!(
            replace_words("dashboard-api dashboards my_dashboard", &pairs),
            "dashboard-api dashboards my_dashboard"
        );
    }

    #[test]
    fn replace_words_never_rescans_inserted_placeholders() {
        // "name" is also a literal to replace, but must not match inside the inserted {{name}}.
        let pairs = [("dash", "name"), ("name", "other")];
        assert_eq!(replace_words("dash name", &pairs), "{{name}} {{other}}");
    }

    #[test]
    fn replace_words_prefers_the_longer_overlapping_match() {
        let pairs = [("app", "short"), ("app.json", "long")];
        assert_eq!(replace_words("cat app.json", &pairs), "cat {{long}}");
    }
}
