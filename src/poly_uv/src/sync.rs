use std::fs;
use std::path::{Component, Path, PathBuf};

use tempfile::TempPath;
use tokio::io::AsyncWriteExt;
use uv_client::{BaseClient, BaseClientBuilder};
use uv_fs::{LockedFile, LockedFileMode};
use uv_pep508::VerbatimUrl;
use uv_workspace::{DiscoveryOptions, MemberDiscovery, VirtualProject, WorkspaceCache};

use crate::lock::plan_lock_contents_for_project;
use crate::stage::{stage_verified_locked_wheel, wheel_sha256_async};
use crate::{Error, InstallPlan, LockedPackage};

/// Configuration for materializing an existing uv lockfile.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Directory from which uv project and workspace discovery starts.
    pub project_dir: PathBuf,
    /// Poly executable written into generated console-script launchers.
    pub sys_executable: PathBuf,
    /// Wheel cache override, relative to the workspace root unless absolute.
    pub cache_dir: Option<PathBuf>,
    /// Installation target override, relative to the workspace root unless absolute.
    pub target_dir: Option<PathBuf>,
}

impl SyncOptions {
    pub fn new(project_dir: impl Into<PathBuf>, sys_executable: impl Into<PathBuf>) -> Self {
        Self {
            project_dir: project_dir.into(),
            sys_executable: sys_executable.into(),
            cache_dir: None,
            target_dir: None,
        }
    }
}

/// Result of a successful frozen sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Discovered uv workspace root.
    pub project_root: PathBuf,
    /// Newly active Python import directory.
    pub target: PathBuf,
    /// Installed distributions formatted as `name==version`.
    pub packages: Vec<String>,
    /// Number of wheels fetched during this run.
    pub downloaded: usize,
    /// Number of wheels reused from the persistent cache.
    pub cache_hits: usize,
    /// Non-fatal cleanup diagnostics emitted after commit.
    pub warnings: Vec<String>,
}

struct PreparedSync {
    project_root: PathBuf,
    cache_dir: PathBuf,
    target_dir: PathBuf,
    plan: InstallPlan,
}

/// Materialize the current project's existing `uv.lock` without resolving it.
///
/// Downloads use uv's in-process HTTP client. Every wheel is content-checked
/// against the lock before installation, and the previous `.poly/python`
/// directory is replaced only after the complete staging directory succeeds.
pub fn sync_frozen(options: &SyncOptions) -> Result<SyncReport, Error> {
    let project_dir = fs::canonicalize(&options.project_dir)?;
    let executor = runtime()?;
    let prepared = executor.block_on(prepare_sync(&project_dir, options))?;
    let lock_dir = prepared.project_root.join(".poly");
    fs::create_dir_all(&lock_dir)?;
    let _sync_lock = executor
        .block_on(LockedFile::acquire(
            lock_dir.join("sync.lock"),
            LockedFileMode::Exclusive,
            "Poly Python environment",
        ))
        .map_err(Error::SyncLock)?;

    fs::create_dir_all(&prepared.cache_dir)?;
    let cache_dir = fs::canonicalize(&prepared.cache_dir)?;
    let lock_dir = fs::canonicalize(&lock_dir)?;
    let target_dir = validate_target(
        &prepared.project_root,
        &prepared.target_dir,
        &[&lock_dir, &cache_dir],
    )?;
    let _cache_lock = executor
        .block_on(LockedFile::acquire(
            cache_dir.join(".lock"),
            LockedFileMode::Exclusive,
            "Poly wheel cache",
        ))
        .map_err(Error::CacheLock)?;

    let mut archives = Vec::with_capacity(prepared.plan.packages.len());
    let mut missing = Vec::new();
    let mut cache_hits = 0;
    for package in &prepared.plan.packages {
        let archive = cached_wheel_path(&cache_dir, package);
        if archive.is_file()
            && executor
                .block_on(wheel_sha256_async(&archive))?
                .eq_ignore_ascii_case(&package.sha256)
        {
            cache_hits += 1;
        } else {
            missing.push((package, archive.clone()));
        }
        archives.push(archive);
    }

    let downloaded = missing.len();
    if !missing.is_empty() {
        let downloads = executor.block_on(download_missing(&missing))?;
        for ((package, destination), temporary) in missing.into_iter().zip(downloads) {
            let actual = executor.block_on(wheel_sha256_async(&temporary))?;
            if !actual.eq_ignore_ascii_case(&package.sha256) {
                return Err(Error::HashMismatch {
                    filename: package.filename.clone(),
                    expected: package.sha256.clone(),
                    actual,
                });
            }
            temporary
                .persist(&destination)
                .map_err(|error| Error::PersistCache {
                    path: destination,
                    source: error.error,
                })?;
        }
    }
    drop(executor);

    let warnings = stage_and_commit(
        &prepared.plan,
        &archives,
        &target_dir,
        &options.sys_executable,
        (poly_python::PYTHON_VERSION.0, poly_python::PYTHON_VERSION.1),
    )?;
    Ok(SyncReport {
        project_root: prepared.project_root,
        target: target_dir,
        packages: prepared
            .plan
            .packages
            .iter()
            .map(|package| format!("{}=={}", package.name, package.version))
            .collect(),
        downloaded,
        cache_hits,
        warnings,
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Executor)
}

async fn prepare_sync(project_dir: &Path, options: &SyncOptions) -> Result<PreparedSync, Error> {
    let discovery_cache = uv_cache::Cache::temp().map_err(Error::Cache)?;
    let workspace_cache = WorkspaceCache::default();
    let project = VirtualProject::discover(
        project_dir,
        &DiscoveryOptions {
            members: MemberDiscovery::Existing,
            ..DiscoveryOptions::default()
        },
        &discovery_cache,
        &workspace_cache,
    )
    .await?;
    let project_root = project.workspace().install_path().clone();
    let lock_path = project_root.join("uv.lock");
    let contents = tokio::fs::read_to_string(&lock_path)
        .await
        .map_err(|source| Error::ReadLock {
            path: lock_path,
            source,
        })?;
    let plan = plan_lock_contents_for_project(&contents, &project_root, project.project_name())?;
    let cache_dir = resolve_from_root(
        &project_root,
        options.cache_dir.as_deref(),
        Path::new(".poly/cache/wheels"),
    );
    let target_dir = resolve_from_root(
        &project_root,
        options.target_dir.as_deref(),
        Path::new(".poly/python"),
    );
    Ok(PreparedSync {
        project_root,
        cache_dir,
        target_dir,
        plan,
    })
}

fn resolve_from_root(root: &Path, configured: Option<&Path>, default: &Path) -> PathBuf {
    let path = configured.unwrap_or(default);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn validate_target(
    project_root: &Path,
    target: &Path,
    protected_paths: &[&Path],
) -> Result<PathBuf, Error> {
    let target =
        normalize_absolute(target).ok_or_else(|| Error::UnsafeTarget(target.to_path_buf()))?;
    if target == project_root
        || project_root.starts_with(&target)
        || protected_paths
            .iter()
            .any(|path| **path == target || path.starts_with(&target))
    {
        return Err(Error::UnsafeTarget(target));
    }
    let filename = target
        .file_name()
        .ok_or_else(|| Error::UnsafeTarget(target.clone()))?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::UnsafeTarget(target.clone()))?;
    fs::create_dir_all(parent)?;
    let target = fs::canonicalize(parent)?.join(filename);
    if target == project_root
        || project_root.starts_with(&target)
        || protected_paths
            .iter()
            .any(|path| **path == target || path.starts_with(&target))
    {
        return Err(Error::UnsafeTarget(target));
    }
    Ok(target)
}

fn normalize_absolute(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
        }
    }
    normalized.is_absolute().then_some(normalized)
}

fn cached_wheel_path(cache_dir: &Path, package: &LockedPackage) -> PathBuf {
    let prefix = package.sha256.get(..2).unwrap_or("00");
    cache_dir
        .join(prefix)
        .join(&package.sha256)
        .join(&package.filename)
}

async fn download_missing(missing: &[(&LockedPackage, PathBuf)]) -> Result<Vec<TempPath>, Error> {
    let client = BaseClientBuilder::default()
        .build()
        .map_err(|error| Error::HttpClient(error.to_string()))?;
    let mut downloads = Vec::with_capacity(missing.len());
    for (package, destination) in missing {
        let parent = destination.parent().expect("cached wheel has a parent");
        fs::create_dir_all(parent)?;
        downloads.push(download_one(&client, package, parent).await?);
    }
    Ok(downloads)
}

async fn download_one(
    client: &BaseClient,
    package: &LockedPackage,
    directory: &Path,
) -> Result<TempPath, Error> {
    let url = VerbatimUrl::parse_url(&package.url).map_err(|error| Error::ArtifactUrl {
        filename: package.filename.clone(),
        message: error.to_string(),
    })?;
    let temporary = tempfile::Builder::new()
        .prefix(".poly-download-")
        .tempfile_in(directory)?
        .into_temp_path();
    let temporary_path: &Path = temporary.as_ref();

    match url.raw().scheme() {
        "file" => {
            let source = url.raw().to_file_path().map_err(|()| Error::ArtifactUrl {
                filename: package.filename.clone(),
                message: "file URL cannot be converted to a local path".to_owned(),
            })?;
            tokio::fs::copy(source, temporary_path).await?;
        }
        "http" | "https" => {
            let mut response = client
                .for_host(url.raw())
                .get(url.raw().as_str())
                .send()
                .await
                .map_err(|error| Error::Download {
                    filename: package.filename.clone(),
                    message: error.to_string(),
                })?;
            if !response.status().is_success() {
                return Err(Error::HttpStatus {
                    filename: package.filename.clone(),
                    status: response.status().as_u16(),
                });
            }
            let mut output = tokio::fs::File::create(temporary_path).await?;
            while let Some(chunk) = response.chunk().await.map_err(|error| Error::Download {
                filename: package.filename.clone(),
                message: error.to_string(),
            })? {
                output.write_all(&chunk).await?;
            }
            output.flush().await?;
            output.sync_all().await?;
        }
        scheme => {
            return Err(Error::ArtifactScheme {
                filename: package.filename.clone(),
                scheme: scheme.to_owned(),
            });
        }
    }
    Ok(temporary)
}

fn stage_and_commit(
    plan: &InstallPlan,
    archives: &[PathBuf],
    target: &Path,
    sys_executable: &Path,
    python_version: (u8, u8),
) -> Result<Vec<String>, Error> {
    debug_assert_eq!(plan.packages.len(), archives.len());
    let parent = target
        .parent()
        .ok_or_else(|| Error::UnsafeTarget(target.to_path_buf()))?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".poly-sync-")
        .tempdir_in(parent)?;
    let staged_target = staging.path().join("python");
    fs::create_dir_all(&staged_target)?;
    for (package, archive) in plan.packages.iter().zip(archives) {
        stage_verified_locked_wheel(
            package,
            archive,
            &staged_target,
            sys_executable,
            python_version,
        )?;
    }

    let mut warnings = Vec::new();
    let previous = match fs::symlink_metadata(target) {
        Ok(_) => {
            let backup_parent = tempfile::Builder::new()
                .prefix(".poly-previous-")
                .tempdir_in(parent)?;
            let backup = backup_parent.path().join("python");
            fs::rename(target, &backup)?;
            Some((backup_parent, backup))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };

    if let Err(commit) = fs::rename(&staged_target, target) {
        if let Some((backup_parent, backup)) = previous
            && let Err(rollback) = fs::rename(&backup, target)
        {
            let backup = backup_parent.keep().join("python");
            return Err(Error::Rollback {
                target: target.to_path_buf(),
                commit: commit.to_string(),
                rollback: rollback.to_string(),
                backup,
            });
        }
        return Err(Error::Commit {
            target: target.to_path_buf(),
            source: commit,
        });
    }

    if let Some((backup_parent, _)) = previous
        && let Err(error) = backup_parent.close()
    {
        warnings.push(format!(
            "the new environment is active, but its generated backup could not be removed: {error}"
        ));
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use uv_pep508::VerbatimUrl;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::stage::wheel_sha256;

    fn wheel_bytes() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (path, contents) in [
            ("demo_pkg/__init__.py", "VALUE = 42\n"),
            (
                "demo_pkg-1.0.0.dist-info/WHEEL",
                "Wheel-Version: 1.0\nGenerator: poly-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
            ),
            (
                "demo_pkg-1.0.0.dist-info/METADATA",
                "Metadata-Version: 2.1\nName: demo-pkg\nVersion: 1.0.0\n",
            ),
            (
                "demo_pkg-1.0.0.dist-info/RECORD",
                "demo_pkg/__init__.py,,\ndemo_pkg-1.0.0.dist-info/WHEEL,,\ndemo_pkg-1.0.0.dist-info/METADATA,,\ndemo_pkg-1.0.0.dist-info/RECORD,,\n",
            ),
        ] {
            writer
                .start_file(path, options)
                .expect("start wheel member");
            writer
                .write_all(contents.as_bytes())
                .expect("write wheel member");
        }
        writer.finish().expect("finish wheel archive").into_inner()
    }

    fn write_project(directory: &Path, wheel_url: &str, hash: &str) {
        fs::write(
            directory.join("pyproject.toml"),
            r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["demo-pkg"]

[tool.uv]
package = false
"#,
        )
        .expect("write pyproject.toml");
        fs::write(
            directory.join("uv.lock"),
            format!(
                r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "demo-pkg"
version = "1.0.0"
source = {{ registry = "https://example.invalid/simple" }}
wheels = [
    {{ url = "{wheel_url}", hash = "sha256:{hash}" }},
]

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "demo-pkg" }}]
"#
            ),
        )
        .expect("write uv.lock");
    }

    fn options(directory: &Path) -> SyncOptions {
        SyncOptions::new(
            directory,
            std::env::current_exe().expect("current executable"),
        )
    }

    #[test]
    fn downloads_caches_and_transactionally_replaces_environment() {
        let directory = tempfile::tempdir().expect("create project");
        let wheel = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        let bytes = wheel_bytes();
        fs::write(&wheel, &bytes).expect("write wheel fixture");
        let hash = wheel_sha256(&wheel).expect("hash wheel fixture");

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local registry");
        let address = listener.local_addr().expect("local registry address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept wheel request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read wheel request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .expect("write response headers");
            stream.write_all(&bytes).expect("write wheel response");
        });
        write_project(
            directory.path(),
            &format!("http://{address}/demo_pkg-1.0.0-py3-none-any.whl"),
            &hash,
        );
        let old_target = directory.path().join(".poly/python");
        fs::create_dir_all(&old_target).expect("create old environment");
        fs::write(old_target.join("obsolete.py"), "OLD = True\n").expect("write obsolete package");

        let first = sync_frozen(&options(directory.path())).expect("first frozen sync");
        server.join().expect("local registry server");
        assert_eq!(first.downloaded, 1);
        assert_eq!(first.cache_hits, 0);
        assert_eq!(first.packages, ["demo-pkg==1.0.0"]);
        assert!(first.target.join("demo_pkg/__init__.py").is_file());
        assert!(!first.target.join("obsolete.py").exists());

        let second = sync_frozen(&options(directory.path())).expect("cached frozen sync");
        assert_eq!(second.downloaded, 0);
        assert_eq!(second.cache_hits, 1);
        assert!(second.target.join("demo_pkg/__init__.py").is_file());
    }

    #[test]
    fn replaces_a_corrupt_cached_wheel() {
        let directory = tempfile::tempdir().expect("create project");
        let wheel = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&wheel, wheel_bytes()).expect("write wheel fixture");
        let wheel_url = VerbatimUrl::from_absolute_path(&wheel)
            .expect("wheel file URL")
            .to_string();
        let hash = wheel_sha256(&wheel).expect("hash wheel fixture");
        write_project(directory.path(), &wheel_url, &hash);

        let first = sync_frozen(&options(directory.path())).expect("initial frozen sync");
        assert_eq!((first.downloaded, first.cache_hits), (1, 0));
        let cached = directory
            .path()
            .join(".poly/cache/wheels")
            .join(&hash[..2])
            .join(&hash)
            .join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&cached, b"corrupt cache entry").expect("corrupt cached wheel");

        let second = sync_frozen(&options(directory.path())).expect("repair corrupt cache");
        assert_eq!((second.downloaded, second.cache_hits), (1, 0));
        assert_eq!(wheel_sha256(&cached).expect("hash repaired cache"), hash);
        assert!(second.target.join("demo_pkg/__init__.py").is_file());
    }

    #[test]
    fn hash_failure_preserves_previous_environment() {
        let directory = tempfile::tempdir().expect("create project");
        let wheel = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&wheel, wheel_bytes()).expect("write wheel fixture");
        let wheel_url = VerbatimUrl::from_absolute_path(&wheel)
            .expect("wheel file URL")
            .to_string();
        write_project(directory.path(), &wheel_url, &"0".repeat(64));
        let target = directory.path().join(".poly/python");
        fs::create_dir_all(&target).expect("create previous environment");
        fs::write(target.join("preserved.py"), "PRESERVED = True\n")
            .expect("write previous environment");

        let error = sync_frozen(&options(directory.path())).expect_err("reject wrong hash");
        assert!(matches!(error, Error::HashMismatch { .. }));
        assert!(target.join("preserved.py").is_file());
        assert!(!target.join("demo_pkg/__init__.py").exists());
    }

    #[test]
    fn staging_failure_preserves_previous_environment() {
        let directory = tempfile::tempdir().expect("create project");
        let wheel = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&wheel, b"not a zip archive").expect("write invalid wheel fixture");
        let wheel_url = VerbatimUrl::from_absolute_path(&wheel)
            .expect("wheel file URL")
            .to_string();
        let hash = wheel_sha256(&wheel).expect("hash invalid wheel fixture");
        write_project(directory.path(), &wheel_url, &hash);
        let target = directory.path().join(".poly/python");
        fs::create_dir_all(&target).expect("create previous environment");
        fs::write(target.join("preserved.py"), "PRESERVED = True\n")
            .expect("write previous environment");

        sync_frozen(&options(directory.path())).expect_err("reject invalid wheel archive");
        assert!(target.join("preserved.py").is_file());
        assert!(!target.join("demo_pkg/__init__.py").exists());
    }

    #[test]
    fn synced_wheel_imports_from_a_python_entrypoint() {
        let directory = tempfile::tempdir().expect("create project");
        let wheel = directory.path().join("demo_pkg-1.0.0-py3-none-any.whl");
        fs::write(&wheel, wheel_bytes()).expect("write wheel fixture");
        let wheel_url = VerbatimUrl::from_absolute_path(&wheel)
            .expect("wheel file URL")
            .to_string();
        let hash = wheel_sha256(&wheel).expect("hash wheel fixture");
        write_project(directory.path(), &wheel_url, &hash);

        sync_frozen(&options(directory.path())).expect("sync frozen wheel");
        let script = directory.path().join("main.py");
        fs::write(&script, "import demo_pkg\nassert demo_pkg.VALUE == 42\n")
            .expect("write Python entrypoint");
        let exit_code = thread::Builder::new()
            .name("poly-uv-rustpython-import".to_owned())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || poly_python::run_file(&script, &[]))
            .expect("spawn RustPython test thread")
            .join()
            .expect("join RustPython test thread")
            .expect("run Python entrypoint");
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn rejects_a_target_that_normalizes_to_a_project_ancestor() {
        let directory = tempfile::tempdir().expect("create project");
        let project = directory.path().join("project");
        fs::create_dir(&project).expect("create project directory");
        let project = fs::canonicalize(project).expect("canonicalize project");
        let target = project.join("..");

        let error = validate_target(&project, &target, &[]).expect_err("reject project ancestor");
        assert!(matches!(error, Error::UnsafeTarget(_)));
    }

    #[test]
    fn rejects_a_target_containing_sync_state() {
        let directory = tempfile::tempdir().expect("create project");
        let project = fs::canonicalize(directory.path()).expect("canonicalize project");
        let lock_dir = project.join(".poly");
        let cache_dir = lock_dir.join("cache/wheels");
        fs::create_dir_all(&cache_dir).expect("create sync state");
        let lock_dir = fs::canonicalize(lock_dir).expect("canonicalize lock directory");
        let cache_dir = fs::canonicalize(cache_dir).expect("canonicalize cache directory");

        let error = validate_target(&project, &project.join(".poly"), &[&lock_dir, &cache_dir])
            .expect_err("reject target containing lock and cache");
        assert!(matches!(error, Error::UnsafeTarget(_)));
    }
}
