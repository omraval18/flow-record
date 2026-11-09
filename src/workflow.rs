//! The workflow data model and its invariants.

use crate::{cli, render, Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Version written to new files; files from a newer version are rejected on load.
pub const FORMAT_VERSION: u32 = 1;

const MAX_NAME_LEN: usize = 48;

/// Where a workflow lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// `<repo>/.fr/workflows`, shared with the repository.
    Project,
    /// The user's config directory, available everywhere.
    Global,
}

impl Scope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Global => "global",
        }
    }

    pub const fn other(self) -> Self {
        match self {
            Self::Project => Self::Global,
            Self::Global => Self::Project,
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// An argument slot a workflow takes at replay time.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// Used when the argument is omitted; without one, replay prompts for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The value seen while recording, shown as a hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

impl Param {
    pub fn new(name: impl Into<String>, example: Option<String>) -> Self {
        Self {
            name: name.into(),
            default: None,
            example,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub description: String,
    /// Raw shell text, possibly containing `{{param}}` placeholders.
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
}

const fn current_version() -> u32 {
    FORMAT_VERSION
}

impl Workflow {
    pub fn new(description: String, steps: Vec<String>, params: Vec<Param>) -> Self {
        Self {
            version: FORMAT_VERSION,
            description,
            steps,
            params,
        }
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// `fr <name> <a> [b=default]`
    pub fn usage(&self, name: &str) -> String {
        let mut usage = format!("fr {name}");
        for p in &self.params {
            usage.push(' ');
            match &p.default {
                Some(d) => usage.push_str(&format!("[{}={d}]", p.name)),
                None => usage.push_str(&format!("<{}>", p.name)),
            }
        }
        usage
    }

    /// Check the invariants every stored workflow must hold.
    pub fn validate(&self) -> Result<()> {
        if self.steps.is_empty() {
            return Err(Error::invalid("a workflow needs at least one step"));
        }
        for (i, p) in self.params.iter().enumerate() {
            if !render::is_ident(&p.name) {
                return Err(Error::invalid(format!(
                    "`{}` is not a valid parameter name (letters, digits and `_`)",
                    p.name
                )));
            }
            if self.params[..i].iter().any(|q| q.name == p.name) {
                return Err(Error::invalid(format!(
                    "parameter `{}` is declared twice",
                    p.name
                )));
            }
        }
        for step in &self.steps {
            if let Some(name) = render::placeholders(step).find(|n| self.param(n).is_none()) {
                return Err(Error::invalid(format!(
                    "a step uses {{{{{name}}}}} but no parameter has that name"
                )));
            }
        }
        Ok(())
    }
}

/// Validate a workflow name: lowercase letters, digits, `-` and `_`, not a command name.
pub fn validate_name(name: &str) -> Result<()> {
    let well_formed = name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !well_formed {
        return Err(Error::invalid(format!(
            "`{name}` is not a valid name — use lowercase letters, digits, `-` and `_` (max {MAX_NAME_LEN})"
        )));
    }
    if cli::is_reserved(name) {
        return Err(Error::invalid(format!(
            "`{name}` is a built-in command; choose another name"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(steps: &[&str], params: &[&str]) -> Workflow {
        Workflow::new(
            String::new(),
            steps.iter().map(|s| (*s).to_owned()).collect(),
            params.iter().map(|n| Param::new(*n, None)).collect(),
        )
    }

    #[test]
    fn toml_round_trip_preserves_everything() {
        let original = Workflow {
            version: FORMAT_VERSION,
            description: "demo".into(),
            steps: vec!["echo {{a}}\nsecond line".into(), "echo 'quote'".into()],
            params: vec![Param {
                name: "a".into(),
                default: Some("x y".into()),
                example: Some("z".into()),
            }],
        };
        let text = toml::to_string_pretty(&original).unwrap();
        assert_eq!(toml::from_str::<Workflow>(&text).unwrap(), original);
    }

    #[test]
    fn a_minimal_file_gets_defaults() {
        let parsed: Workflow = toml::from_str("steps = [\"ls\"]").unwrap();
        assert_eq!(parsed.version, FORMAT_VERSION);
        assert!(parsed.params.is_empty() && parsed.description.is_empty());
    }

    #[test]
    fn validate_rejects_undefined_placeholders_and_bad_params() {
        assert!(wf(&["echo {{x}}"], &["x"]).validate().is_ok());
        assert!(
            wf(&["echo {{y}}"], &["x"]).validate().is_err(),
            "undefined placeholder"
        );
        assert!(
            wf(&["echo {{.Names}}"], &[]).validate().is_ok(),
            "foreign templates are not placeholders"
        );
        assert!(wf(&[], &[]).validate().is_err(), "no steps");
        assert!(
            wf(&["a"], &["x", "x"]).validate().is_err(),
            "duplicate parameter"
        );
        assert!(
            wf(&["a"], &["not valid"]).validate().is_err(),
            "bad parameter name"
        );
    }

    #[test]
    fn usage_marks_optional_parameters() {
        let mut w = wf(&["echo {{a}} {{b}}"], &["a", "b"]);
        w.params[1].default = Some("d".into());
        assert_eq!(w.usage("go"), "fr go <a> [b=d]");
    }

    #[test]
    fn names_are_restricted() {
        for ok in ["next", "a1", "deploy-prod", "x_y", "9lives"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "Next",
            "-x",
            "_x",
            "has space",
            "dot.dot",
            "list",
            "run",
            &"a".repeat(49),
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }
}
