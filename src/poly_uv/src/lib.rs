//! In-process integration boundary between Poly and uv.
//!
//! Poly depends on uv's library crates at a pinned revision. It does not call
//! the `uv` executable or uv's process-oriented `unsafe main` entry point.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;
use uv_workspace::{DiscoveryOptions, Workspace, WorkspaceCache};

mod lock;
mod stage;
mod sync;

pub use lock::{InstallPlan, LockedPackage, plan_lock, plan_lock_contents};
pub use stage::{stage_locked_wheel, stage_wheel_archive};
pub use sync::{SyncOptions, SyncReport, sync_frozen};

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to initialize the embedded uv executor: {0}")]
    Executor(#[source] std::io::Error),
    #[error("failed to initialize the embedded uv cache: {0}")]
    Cache(#[source] std::io::Error),
    #[error("failed to acquire the project sync lock: {0}")]
    SyncLock(#[source] uv_fs::LockedFileError),
    #[error("failed to acquire the wheel cache lock: {0}")]
    CacheLock(#[source] uv_fs::LockedFileError),
    #[error(transparent)]
    Workspace(#[from] uv_workspace::WorkspaceError),
    #[error("failed to read uv lockfile `{path}`: {source}")]
    ReadLock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("uv rejected the lockfile: {0}")]
    InvalidLock(String),
    #[error("RustPython {version} does not satisfy the lockfile requirement `{requires_python}`")]
    PythonVersion {
        version: String,
        requires_python: String,
    },
    #[error("the current RustPython environment is not supported by this uv lockfile")]
    UnsupportedEnvironment,
    #[error("failed to construct the RustPython marker environment: {0}")]
    MarkerEnvironment(String),
    #[error("invalid embedded RustPython version `{version}`: {message}")]
    InvalidPythonVersion { version: String, message: String },
    #[error("failed to construct wheel compatibility tags: {0}")]
    PlatformTags(String),
    #[error("uv could not materialize the selected lockfile dependency graph: {0}")]
    LockResolution(String),
    #[error("invalid wheel filename `{filename}`: {message}")]
    InvalidWheel { filename: String, message: String },
    #[error(
        "registry package `{name}=={version}` has no RustPython-compatible pure wheel; available wheels: {available:?}"
    )]
    NoPureWheel {
        name: String,
        version: String,
        available: Vec<String>,
    },
    #[error("pure wheel `{filename}` for `{name}=={version}` has no SHA-256 hash")]
    MissingHash {
        name: String,
        version: String,
        filename: String,
    },
    #[error("locked artifact `{filename}` has an invalid URL: {message}")]
    ArtifactUrl { filename: String, message: String },
    #[error("locked artifact `{filename}` uses unsupported URL scheme `{scheme}`")]
    ArtifactScheme { filename: String, scheme: String },
    #[error("failed to initialize uv's HTTP client: {0}")]
    HttpClient(String),
    #[error("failed to download locked artifact `{filename}`: {message}")]
    Download { filename: String, message: String },
    #[error("download for locked artifact `{filename}` returned HTTP {status}")]
    HttpStatus { filename: String, status: u16 },
    #[error("failed to persist wheel in cache at `{path}`: {source}")]
    PersistCache {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "package `{name}=={version}` uses {kind}; Poly sync accepts only hashed generic pure wheels"
    )]
    UnsupportedSource {
        name: String,
        version: String,
        kind: String,
    },
    #[error("failed to access wheel or staging files: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    ExtractWheel(#[from] uv_extract::Error),
    #[error(transparent)]
    InstallWheel(#[from] uv_install_wheel::Error),
    #[error("wheel `{0}` is not a generic Python 3 none-any wheel")]
    IncompatibleWheel(String),
    #[error("locked wheel filename `{expected}` does not match archive `{actual}`")]
    WheelFilenameMismatch { expected: String, actual: String },
    #[error("SHA-256 mismatch for wheel `{filename}`: expected {expected}, got {actual}")]
    HashMismatch {
        filename: String,
        expected: String,
        actual: String,
    },
    #[error("refusing to use unsafe sync target `{0}`")]
    UnsafeTarget(PathBuf),
    #[error("failed to commit the staged Python environment at `{target}`: {source}")]
    Commit {
        target: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "failed to commit the staged Python environment at `{target}` ({commit}); restoring the previous environment also failed ({rollback}); the previous environment remains at `{backup}`"
    )]
    Rollback {
        target: PathBuf,
        commit: String,
        rollback: String,
        backup: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub workspace_root: PathBuf,
    pub members: Vec<ProjectMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMember {
    pub root: PathBuf,
    pub name: String,
    pub dependencies: Vec<String>,
    pub optional_dependencies: BTreeMap<String, Vec<String>>,
}

/// Discover and validate a uv project without launching another process.
///
/// This is the Poly-owned boundary over uv's internal crates. Lockfile,
/// distribution and installer types stay inside this crate rather than
/// leaking into the rest of the runtime.
pub fn inspect_project(path: &Path) -> Result<ProjectSnapshot, Error> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(Error::Executor)?;
    executor.block_on(inspect_project_async(path))
}

async fn inspect_project_async(path: &Path) -> Result<ProjectSnapshot, Error> {
    let cache = uv_cache::Cache::temp().map_err(Error::Cache)?;
    let workspace_cache = WorkspaceCache::default();
    let workspace =
        Workspace::discover(path, &DiscoveryOptions::default(), &cache, &workspace_cache).await?;

    let members = workspace
        .packages()
        .values()
        .map(|member| {
            let project = member.project();
            ProjectMember {
                root: member.root().clone(),
                name: project.name.to_string(),
                dependencies: project.dependencies.clone().unwrap_or_default(),
                optional_dependencies: project
                    .optional_dependencies
                    .as_ref()
                    .map(|groups| {
                        groups
                            .iter()
                            .map(|(name, dependencies)| (name.to_string(), dependencies.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        })
        .collect();

    Ok(ProjectSnapshot {
        workspace_root: workspace.install_path().clone(),
        members,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn discovers_project_through_embedded_uv() {
        let directory = tempfile::tempdir().expect("create temporary project");
        fs::write(
            directory.path().join("pyproject.toml"),
            r#"
[project]
name = "poly-embedded-uv-test"
version = "1.2.3"
requires-python = ">=3.12"
dependencies = ["httpx>=0.28"]

[project.optional-dependencies]
test = ["pytest>=8"]

[tool.uv]
package = false
"#,
        )
        .expect("write pyproject.toml");

        let snapshot = inspect_project(directory.path()).expect("inspect uv project");
        assert_eq!(snapshot.workspace_root, directory.path());
        assert_eq!(snapshot.members.len(), 1);

        let project = &snapshot.members[0];
        assert_eq!(project.name, "poly-embedded-uv-test");
        assert_eq!(project.dependencies, ["httpx>=0.28"]);
        assert_eq!(
            project.optional_dependencies.get("test"),
            Some(&vec!["pytest>=8".to_owned()])
        );
    }

    #[test]
    fn discovers_popular_packages_example() {
        let example =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../poly/examples/python-packages");
        let snapshot = inspect_project(&example).expect("inspect popular packages example");
        assert_eq!(snapshot.members.len(), 1);
        assert_eq!(
            snapshot.members[0].dependencies,
            ["click", "requests", "rich"]
        );

        let plan = plan_lock(&example.join("uv.lock")).expect("plan popular packages lockfile");
        let package_names = plan
            .packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for direct_dependency in ["click", "requests", "rich"] {
            assert!(
                package_names.contains(direct_dependency),
                "install plan should contain {direct_dependency}"
            );
        }
    }
}
