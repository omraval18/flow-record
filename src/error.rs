//! The crate-wide error type.

use std::path::PathBuf;
use std::{fmt, io};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The user declined a confirmation or closed the input (Ctrl-D / Ctrl-C).
    Cancelled,
    /// Wrong command-line usage; the message explains the fix.
    Usage(String),
    /// A value supplied by the user was rejected.
    Invalid(String),
    /// No workflow matched `name`; `similar` lists near misses.
    NotFound { name: String, similar: Vec<String> },
    /// Reading or writing `path` failed.
    Io { path: PathBuf, source: io::Error },
    /// A terminal or process operation failed.
    Os(io::Error),
    /// `path` is not a valid workflow file.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// A workflow could not be encoded as TOML.
    Serialize(toml::ser::Error),
}

impl Error {
    pub fn usage(msg: impl Into<String>) -> Self {
        Self::Usage(msg.into())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Usage(msg) | Self::Invalid(msg) => f.write_str(msg),
            Self::NotFound { name, similar } if similar.is_empty() => {
                write!(f, "no workflow called `{name}` — see `fr list`")
            }
            Self::NotFound { name, similar } => {
                write!(
                    f,
                    "no workflow called `{name}` — did you mean: {}",
                    similar.join(", ")
                )
            }
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Os(source) => source.fmt(f),
            Self::Parse { path, source } => write!(f, "{}: {}", path.display(), source.message()),
            Self::Serialize(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Os(source) => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Serialize(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::Os(source)
    }
}
