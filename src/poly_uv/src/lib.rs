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

pub use lock::{InstallPlan, LockedPackage, plan_lock, plan_lock_contents};
pub use stage::{stage_locked_wheel, stage_wheel_archive};

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to initialize the embedded uv executor: {0}")]
    Executor(#[source] std::io::Error),
    #[error("failed to initialize the embedded uv cache: {0}")]
    Cache(#[source] std::io::Error),
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
    #[error("failed to decode the validated uv lockfile: {0}")]
    LockToml(#[from] toml::de::Error),
    #[error("uv lockfile package count changed while building the install plan")]
    LockInvariant,
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
/// This is the first stable Poly-owned boundary over uv's internal crates.
/// Resolver, lockfile, distribution and installer operations will be added to
/// this crate rather than exposed directly to the rest of the runtime.
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
    }
}
