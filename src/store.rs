//! Workflow files on disk: project/global scoping, name resolution, usage ranking and
//! trust for repository-supplied workflows.
//!
//! ```text
//! project  <repo>/.fr/workflows/<name>.toml     nearest ancestor of the cwd holding `.fr`
//! global   $XDG_CONFIG_HOME/fr/workflows/<name>.toml
//! state    $XDG_STATE_HOME/fr/{usage,trusted,sessions/}
//! ```

use crate::workflow::{Scope, Workflow, FORMAT_VERSION};
use crate::{Error, Result};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE_LOG_MAX_BYTES: u64 = 64 * 1024;
const USAGE_LOG_KEEP_LINES: usize = 500;

/// Where `fr` keeps its configuration and state.
#[derive(Debug, Clone)]
pub struct Dirs {
    config: PathBuf,
    state: PathBuf,
}

impl Dirs {
    pub fn new(config: impl Into<PathBuf>, state: impl Into<PathBuf>) -> Self {
        Self {
            config: config.into(),
            state: state.into(),
        }
    }

    /// `$XDG_CONFIG_HOME/fr` and `$XDG_STATE_HOME/fr`, defaulting to `~/.config` and `~/.local/state`.
    pub fn from_env() -> Self {
        use std::env::var_os;
        let home = var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let base = |var: &str, fallback: &str| {
            var_os(var)
                .filter(|v| !v.is_empty())
                .map_or_else(|| home.join(fallback), PathBuf::from)
        };
        Self::new(
            base("XDG_CONFIG_HOME", ".config").join("fr"),
            base("XDG_STATE_HOME", ".local/state").join("fr"),
        )
    }

    pub fn global_workflows(&self) -> PathBuf {
        self.config.join("workflows")
    }

    pub fn sessions(&self) -> PathBuf {
        self.state.join("sessions")
    }

    fn usage_log(&self) -> PathBuf {
        self.state.join("usage")
    }

    fn trust_file(&self) -> PathBuf {
        self.state.join("trusted")
    }
}

/// A workflow file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub scope: Scope,
    pub path: PathBuf,
    pub description: String,
}

/// Workflows visible from a directory.
#[derive(Debug, Default)]
pub struct Listing {
    /// Most relevant first: project before global, then most recently and most often used.
    pub ranked: Vec<Entry>,
    /// Global workflows hidden by a project workflow of the same name.
    pub shadowed: Vec<Entry>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Exact(Entry),
    /// Matched as the only workflow whose name starts with the input.
    Prefix(Entry),
    /// Several names start with the input; ranked best first.
    Ambiguous(Vec<Entry>),
    Missing {
        similar: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Store {
    dirs: Dirs,
    cwd: PathBuf,
}

impl Store {
    pub fn new(dirs: Dirs, cwd: impl Into<PathBuf>) -> Self {
        Self {
            dirs,
            cwd: cwd.into(),
        }
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self::new(Dirs::from_env(), std::env::current_dir()?))
    }

    pub fn dirs(&self) -> &Dirs {
        &self.dirs
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn project_root(&self) -> Option<PathBuf> {
        find_project_root(&self.cwd)
    }

    /// The directory a workflow of `scope` is stored in. `project_root` is required for
    /// [`Scope::Project`].
    pub fn workflows_dir(&self, scope: Scope, project_root: Option<&Path>) -> PathBuf {
        match (scope, project_root) {
            (Scope::Project, Some(root)) => project_dir(root),
            _ => self.dirs.global_workflows(),
        }
    }

    pub fn list(&self) -> Listing {
        let project = self
            .project_root()
            .map(|root| read_entries(&project_dir(&root), Scope::Project))
            .unwrap_or_default();
        let (shadowed, global): (Vec<_>, Vec<_>) =
            read_entries(&self.dirs.global_workflows(), Scope::Global)
                .into_iter()
                .partition(|g| project.iter().any(|p| p.name == g.name));

        let usage = self.usage();
        let mut ranked: Vec<Entry> = project.into_iter().chain(global).collect();
        ranked.sort_by(|a, b| rank(a, &usage).cmp(&rank(b, &usage)));
        Listing { ranked, shadowed }
    }

    /// Exact name, else unique prefix, else the ranked candidates. `only` restricts the
    /// search to one scope (and makes shadowed global workflows reachable).
    pub fn resolve(&self, input: &str, only: Option<Scope>) -> Resolution {
        let Listing {
            mut ranked,
            shadowed,
        } = self.list();
        if let Some(scope) = only {
            ranked.extend(shadowed);
            ranked.retain(|e| e.scope == scope);
        }
        if let Some(i) = ranked.iter().position(|e| e.name == input) {
            return Resolution::Exact(ranked.swap_remove(i));
        }
        let (mut prefixed, rest): (Vec<_>, Vec<_>) =
            ranked.into_iter().partition(|e| e.name.starts_with(input));
        match prefixed.len() {
            0 => Resolution::Missing {
                similar: rest
                    .into_iter()
                    .filter(|e| e.name.contains(input))
                    .map(|e| e.name)
                    .collect(),
            },
            1 => Resolution::Prefix(prefixed.remove(0)),
            _ => Resolution::Ambiguous(prefixed),
        }
    }

    // ---- usage ranking ---------------------------------------------------------------

    fn context(&self) -> String {
        self.project_root()
            .map_or_else(|| "-".to_owned(), |root| root.display().to_string())
    }

    /// Note that `name` was run here. Ranking is best-effort; callers may ignore failure.
    pub fn record_use(&self, name: &str) -> Result<()> {
        let path = self.dirs.usage_log();
        let io_err = |e| Error::io(&path, e);
        fs::create_dir_all(&self.dirs.state).map_err(|e| Error::io(&self.dirs.state, e))?;
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_err)?;
        writeln!(log, "{}\t{}\t{name}", unix_now(), self.context()).map_err(io_err)?;
        compact_log(&path).map_err(io_err)
    }

    fn usage(&self) -> HashMap<String, Use> {
        let mut usage: HashMap<String, Use> = HashMap::new();
        let Ok(file) = File::open(self.dirs.usage_log()) else {
            return usage;
        };
        let context = self.context();
        for line in BufReader::new(file).lines().map_while(io::Result::ok) {
            let mut fields = line.splitn(3, '\t');
            if let (Some(at), Some(ctx), Some(name)) = (fields.next(), fields.next(), fields.next())
            {
                if ctx == context {
                    let entry = usage.entry(name.to_owned()).or_default();
                    entry.last = entry.last.max(at.parse().unwrap_or(0));
                    entry.count += 1;
                }
            }
        }
        usage
    }

    // ---- trust -----------------------------------------------------------------------

    /// Whether `path` has exactly the content the user last approved (or wrote).
    pub fn is_trusted(&self, path: &Path) -> bool {
        let Ok(line) = trust_line(path) else {
            return false;
        };
        File::open(self.dirs.trust_file()).is_ok_and(|f| {
            BufReader::new(f)
                .lines()
                .map_while(io::Result::ok)
                .any(|l| l == line)
        })
    }

    /// Approve the current content of `path`. Editing the file revokes the approval.
    pub fn trust(&self, path: &Path) -> Result<()> {
        if self.is_trusted(path) {
            return Ok(());
        }
        let line = trust_line(path).map_err(|e| Error::io(path, e))?;
        let file = self.dirs.trust_file();
        fs::create_dir_all(&self.dirs.state).map_err(|e| Error::io(&self.dirs.state, e))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .map_err(|e| Error::io(&file, e))?;
        writeln!(f, "{line}").map_err(|e| Error::io(&file, e))
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Use {
    last: u64,
    count: u32,
}

/// Sort key: project first, then most recent, then most used, then alphabetical.
fn rank<'a>(
    entry: &'a Entry,
    usage: &HashMap<String, Use>,
) -> (bool, Reverse<u64>, Reverse<u32>, &'a str) {
    let used = usage.get(&entry.name).copied().unwrap_or_default();
    (
        entry.scope == Scope::Global,
        Reverse(used.last),
        Reverse(used.count),
        &entry.name,
    )
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Keep the usage log bounded: once it grows past the limit, retain only recent lines.
fn compact_log(path: &Path) -> io::Result<()> {
    if fs::metadata(path)?.len() <= USAGE_LOG_MAX_BYTES {
        return Ok(());
    }
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let kept = lines[lines.len().saturating_sub(USAGE_LOG_KEEP_LINES)..].join("\n");
    fs::write(path, kept + "\n")
}

/// `<sha256 of content>\t<canonical path>`: approval is bound to both.
fn trust_line(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    let canonical = fs::canonicalize(path)?;
    Ok(format!("{:x}\t{}", hasher.finalize(), canonical.display()))
}

// ---- directory layout ------------------------------------------------------------------

pub fn project_dir(root: &Path) -> PathBuf {
    root.join(".fr").join("workflows")
}

/// Nearest ancestor of `from` (inclusive) that holds a `.fr` directory.
pub fn find_project_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|dir| dir.join(".fr").is_dir())
        .map(Path::to_path_buf)
}

/// Where a new recording started in `from` should be saved: the nearest `.fr` or git root.
pub fn find_save_root(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|dir| dir.join(".fr").is_dir() || dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn read_entries(dir: &Path, scope: Scope) -> Vec<Entry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    read_dir
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_owned();
            // A file that fails to parse is still listed so `fr edit` can fix it.
            let description = load(&path).map(|w| w.description).unwrap_or_default();
            Some(Entry {
                name,
                scope,
                path,
                description,
            })
        })
        .collect()
}

// ---- reading and writing workflow files ---------------------------------------------------

/// Read a workflow file. The content is not validated, so broken files can still be edited.
pub fn load(path: &Path) -> Result<Workflow> {
    let text = fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let workflow: Workflow = toml::from_str(&text).map_err(|source| Error::Parse {
        path: path.to_owned(),
        source,
    })?;
    if workflow.version > FORMAT_VERSION {
        return Err(Error::invalid(format!(
            "{} was written by a newer fr (format {}, this version reads up to {FORMAT_VERSION})",
            path.display(),
            workflow.version
        )));
    }
    Ok(workflow)
}

/// Validate and write `<dir>/<name>.toml`, replacing any existing file atomically.
pub fn write(dir: &Path, name: &str, workflow: &Workflow) -> Result<PathBuf> {
    workflow.validate()?;
    let text = toml::to_string_pretty(workflow).map_err(Error::Serialize)?;
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let path = dir.join(format!("{name}.toml"));
    let staging = dir.join(format!(".{name}.toml.tmp"));
    fs::write(&staging, text).map_err(|e| Error::io(&staging, e))?;
    fs::rename(&staging, &path).map_err(|e| Error::io(&path, e))?;
    Ok(path)
}

pub fn delete(path: &Path) -> Result<()> {
    fs::remove_file(path).map_err(|e| Error::io(path, e))
}

#[cfg(test)]
pub mod testing {
    //! A throwaway config/state/working tree for tests in this and other modules.

    use super::*;
    use tempfile::{tempdir, TempDir};

    #[derive(Debug)]
    pub struct Fixture {
        root: TempDir,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Fixture {
        pub fn new() -> Self {
            Self {
                root: tempdir().unwrap(),
            }
        }

        pub fn path(&self, rel: &str) -> PathBuf {
            self.root.path().join(rel)
        }

        pub fn store_in(&self, cwd: &str) -> Store {
            let cwd = self.path(cwd);
            fs::create_dir_all(&cwd).unwrap();
            Store::new(Dirs::new(self.path("config"), self.path("state")), cwd)
        }

        /// Create a one-step workflow file and return its path.
        pub fn put(&self, dir: &str, name: &str, description: &str) -> PathBuf {
            self.put_workflow(
                dir,
                name,
                &Workflow::new(description.into(), vec!["true".into()], vec![]),
            )
        }

        pub fn put_workflow(&self, dir: &str, name: &str, workflow: &Workflow) -> PathBuf {
            write(&self.path(dir), name, workflow).unwrap()
        }

        pub fn global(&self, name: &str) -> PathBuf {
            self.put("config/workflows", name, &format!("global {name}"))
        }

        pub fn project(&self, repo: &str, name: &str) -> PathBuf {
            self.put(
                &format!("{repo}/.fr/workflows"),
                name,
                &format!("project {name}"),
            )
        }

        pub fn log_use(&self, store: &Store, at: u64, name: &str) {
            fs::create_dir_all(self.path("state")).unwrap();
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path("state/usage"))
                .unwrap();
            writeln!(log, "{at}\t{}\t{name}", store.context()).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Fixture;
    use super::*;
    use crate::workflow::Param;

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn project_workflows_rank_before_global_and_shadow_same_names() {
        let fx = Fixture::new();
        fx.global("lint");
        fx.global("deploy");
        fx.project("repo", "lint");
        fx.project("repo", "build");
        let listing = fx.store_in("repo").list();

        assert_eq!(names(&listing.ranked), ["build", "lint", "deploy"]);
        assert_eq!(
            listing.ranked[1].scope,
            Scope::Project,
            "`lint` is the project one"
        );
        assert_eq!(names(&listing.shadowed), ["lint"]);
        assert_eq!(listing.shadowed[0].scope, Scope::Global);
    }

    #[test]
    fn outside_a_project_only_global_workflows_are_visible() {
        let fx = Fixture::new();
        fx.global("a");
        fx.project("repo", "b");
        assert_eq!(names(&fx.store_in("elsewhere").list().ranked), ["a"]);
    }

    #[test]
    fn the_nearest_project_wins_and_subdirectories_inherit() {
        let fx = Fixture::new();
        fx.project("mono", "outer");
        fx.project("mono/pkg", "inner");
        assert_eq!(names(&fx.store_in("mono/pkg/src").list().ranked), ["inner"]);
        assert_eq!(names(&fx.store_in("mono/docs").list().ranked), ["outer"]);
    }

    #[test]
    fn recency_then_frequency_then_name_order_workflows_within_a_scope() {
        let fx = Fixture::new();
        for name in ["alpha", "beta", "gamma", "delta"] {
            fx.global(name);
        }
        let store = fx.store_in("anywhere");
        fx.log_use(&store, 100, "gamma");
        fx.log_use(&store, 200, "beta");
        fx.log_use(&store, 50, "delta");
        fx.log_use(&store, 60, "delta"); // used twice, but last used before gamma/beta

        assert_eq!(
            names(&store.list().ranked),
            ["beta", "gamma", "delta", "alpha"]
        );
    }

    #[test]
    fn usage_in_one_project_does_not_rank_another() {
        let fx = Fixture::new();
        fx.project("a", "x");
        fx.project("a", "y");
        fx.project("b", "x");
        fx.project("b", "y");
        let (in_a, in_b) = (fx.store_in("a"), fx.store_in("b"));
        fx.log_use(&in_a, 10, "y");
        assert_eq!(names(&in_a.list().ranked), ["y", "x"]);
        assert_eq!(names(&in_b.list().ranked), ["x", "y"], "b never used y");
    }

    #[test]
    fn record_use_feeds_the_ranking() {
        let fx = Fixture::new();
        fx.global("a");
        fx.global("b");
        let store = fx.store_in("w");
        store.record_use("b").unwrap();
        assert_eq!(names(&store.list().ranked), ["b", "a"]);
    }

    #[test]
    fn the_usage_log_stays_bounded() {
        let fx = Fixture::new();
        let store = fx.store_in("w");
        fx.log_use(&store, 1, "first");
        for _ in 0..3000 {
            fx.log_use(&store, 2, "filler-entry-to-grow-the-file");
        }
        store.record_use("latest").unwrap();

        let log = fs::read_to_string(fx.path("state/usage")).unwrap();
        assert!(log.len() < 64 * 1024, "{} bytes", log.len());
        assert!(
            log.trim_end().ends_with("latest"),
            "most recent entry survives compaction"
        );
        assert!(!log.contains("first"), "oldest entries are dropped");
    }

    #[test]
    fn resolve_prefers_exact_then_unique_prefix_then_reports_ambiguity() {
        let fx = Fixture::new();
        for name in ["next", "nextra", "nuke", "build"] {
            fx.global(name);
        }
        let store = fx.store_in("w");

        assert!(
            matches!(store.resolve("next", None), Resolution::Exact(e) if e.name == "next"),
            "exact beats prefix of `nextra`"
        );
        assert!(matches!(store.resolve("bu", None), Resolution::Prefix(e) if e.name == "build"));
        match store.resolve("n", None) {
            Resolution::Ambiguous(c) => assert_eq!(names(&c), ["next", "nextra", "nuke"]),
            other => panic!("{other:?}"),
        }
        match store.resolve("ext", None) {
            Resolution::Missing { similar } => {
                assert_eq!(similar, ["next", "nextra"], "substring matches are offered")
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(store.resolve("zzz", None), Resolution::Missing { similar } if similar.is_empty())
        );
    }

    #[test]
    fn a_scope_restriction_reaches_the_shadowed_global_workflow() {
        let fx = Fixture::new();
        fx.global("lint");
        fx.project("repo", "lint");
        let store = fx.store_in("repo");

        let scope_of = |only| match store.resolve("lint", only) {
            Resolution::Exact(e) => e.scope,
            other => panic!("{other:?}"),
        };
        assert_eq!(scope_of(None), Scope::Project);
        assert_eq!(scope_of(Some(Scope::Project)), Scope::Project);
        assert_eq!(scope_of(Some(Scope::Global)), Scope::Global);
    }

    #[test]
    fn trust_is_bound_to_content_and_location() {
        let fx = Fixture::new();
        let store = fx.store_in("repo");
        let file = fx.project("repo", "build");

        assert!(!store.is_trusted(&file), "new files are untrusted");
        store.trust(&file).unwrap();
        assert!(store.is_trusted(&file));

        let copy = fx.path("repo/.fr/workflows/copy.toml");
        fs::copy(&file, &copy).unwrap();
        assert!(
            !store.is_trusted(&copy),
            "identical content elsewhere is a different file"
        );

        fs::write(&file, "steps = [\"rm -rf ~\"]\n").unwrap();
        assert!(!store.is_trusted(&file), "editing revokes approval");
    }

    #[test]
    fn trusting_twice_does_not_duplicate_the_record() {
        let fx = Fixture::new();
        let store = fx.store_in("repo");
        let file = fx.project("repo", "build");
        store.trust(&file).unwrap();
        store.trust(&file).unwrap();
        assert_eq!(
            fs::read_to_string(fx.path("state/trusted"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn write_validates_replaces_atomically_and_round_trips() {
        let fx = Fixture::new();
        let dir = fx.path("wf");
        let wf = Workflow::new(
            "d".into(),
            vec!["echo {{a}}".into()],
            vec![Param::new("a", Some("x".into()))],
        );

        let path = write(&dir, "demo", &wf).unwrap();
        assert_eq!(load(&path).unwrap(), wf);
        let mut changed = wf.clone();
        changed.steps.push("true".into());
        write(&dir, "demo", &changed).unwrap();
        assert_eq!(load(&path).unwrap(), changed);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no staging files left behind"
        );

        let invalid = Workflow::new(String::new(), vec!["echo {{missing}}".into()], vec![]);
        assert!(write(&dir, "bad", &invalid).is_err());
        assert!(
            !dir.join("bad.toml").exists(),
            "invalid workflows are never written"
        );
    }

    #[test]
    fn load_reports_the_path_of_a_broken_file_and_rejects_newer_formats() {
        let fx = Fixture::new();
        let broken = fx.path("broken.toml");
        fs::write(&broken, "steps = nope").unwrap();
        let err = load(&broken).unwrap_err().to_string();
        assert!(err.contains("broken.toml"), "{err}");

        let future = fx.path("future.toml");
        fs::write(
            &future,
            format!("version = {}\nsteps = [\"x\"]\n", FORMAT_VERSION + 1),
        )
        .unwrap();
        assert!(load(&future).unwrap_err().to_string().contains("newer fr"));
    }

    #[test]
    fn broken_files_are_still_listed_so_they_can_be_fixed() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.path("config/workflows")).unwrap();
        fs::write(fx.path("config/workflows/oops.toml"), "not toml [").unwrap();
        assert_eq!(names(&fx.store_in("w").list().ranked), ["oops"]);
    }

    #[test]
    fn save_root_prefers_the_nearest_fr_or_git_directory() {
        let fx = Fixture::new();
        fs::create_dir_all(fx.path("repo/.git")).unwrap();
        fs::create_dir_all(fx.path("repo/sub/deep")).unwrap();
        fs::create_dir_all(fx.path("plain/dir")).unwrap();
        assert_eq!(
            find_save_root(&fx.path("repo/sub/deep")),
            Some(fx.path("repo"))
        );
        assert_eq!(find_save_root(&fx.path("plain/dir")), None);
        assert_eq!(
            find_project_root(&fx.path("repo/sub")),
            None,
            "a git repo alone is not yet a project root for lookup"
        );
    }
}
