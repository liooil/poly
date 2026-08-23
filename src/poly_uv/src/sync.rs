use std::fs;
use std::path::{Component, Path, PathBuf};

use tempfile::TempPath;
use tokio::io::AsyncWriteExt;
use uv_client::BaseClient;
use uv_fs::{LockedFile, LockedFileMode};
use uv_pep508::VerbatimUrl;
use uv_workspace::{DiscoveryOptions, MemberDiscovery, VirtualProject, WorkspaceCache};

use crate::lock::plan_lock_contents_for_project;
use crate::resolve::{client_builder_for_indexes, project_indexes, resolve_project_lock};
use crate::stage::{stage_verified_locked_wheel, wheel_sha256_async};
use crate::{Error, InstallPlan, LockedPackage};

/// Configuration for resolving or materializing a uv project.
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

/// Result of a successful sync.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMode {
    Resolve,
    Frozen,
}

/// Resolve the current project, update `uv.lock`, and materialize its environment.
pub fn sync(options: &SyncOptions) -> Result<SyncReport, Error> {
    sync_with_mode(options, SyncMode::Resolve)
}

/// Materialize the current project's existing `uv.lock` without resolving it.
///
/// Downloads use uv's in-process HTTP client. Every wheel is content-checked
/// against the lock before installation, and the previous `.poly/python`
/// directory is replaced only after the complete staging directory succeeds.
pub fn sync_frozen(options: &SyncOptions) -> Result<SyncReport, Error> {
    sync_with_mode(options, SyncMode::Frozen)
}

fn sync_with_mode(options: &SyncOptions, mode: SyncMode) -> Result<SyncReport, Error> {
    let project_dir = fs::canonicalize(&options.project_dir)?;
    let executor = runtime()?;
    let project = executor.block_on(discover_project(&project_dir))?;
    let project_root = project.workspace().install_path().clone();
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
    let lock_dir = project_root.join(".poly");
    fs::create_dir_all(&lock_dir)?;
    let _sync_lock = executor
        .block_on(LockedFile::acquire(
            lock_dir.join("sync.lock"),
            LockedFileMode::Exclusive,
            "Poly Python environment",
        ))
        .map_err(Error::SyncLock)?;

    fs::create_dir_all(&cache_dir)?;
    let cache_dir = fs::canonicalize(&cache_dir)?;
    let lock_dir = fs::canonicalize(&lock_dir)?;
    let resolver_cache_dir = canonicalize_destination(&project_root.join(".poly/cache/uv"))?;
    let target_dir = validate_target(
        &project_root,
        &target_dir,
        &[&lock_dir, &cache_dir, &resolver_cache_dir],
    )?;

    let lock_path = project_root.join("uv.lock");
    let contents = if mode == SyncMode::Frozen {
        executor
            .block_on(tokio::fs::read_to_string(&lock_path))
            .map_err(|source| Error::ReadLock {
                path: lock_path.clone(),
                source,
            })?
    } else {
        executor.block_on(resolve_project_lock(&project, &resolver_cache_dir))?
    };
    let plan = plan_lock_contents_for_project(&contents, &project_root, project.project_name())?;
    if mode == SyncMode::Resolve {
        executor.block_on(uv_fs::write_atomic(&lock_path, &contents))?;
    }

    let _cache_lock = executor
        .block_on(LockedFile::acquire(
            cache_dir.join(".lock"),
            LockedFileMode::Exclusive,
            "Poly wheel cache",
        ))
        .map_err(Error::CacheLock)?;

    let mut archives = Vec::with_capacity(plan.packages.len());
    let mut missing = Vec::new();
    let mut cache_hits = 0;
    for package in &plan.packages {
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
        let client = client_builder_for_indexes(&project_indexes(project.workspace()))?
            .build()
            .map_err(|error| Error::HttpClient(error.to_string()))?;
        let downloads = executor.block_on(download_missing(&client, &missing))?;
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
        &plan,
        &archives,
        &target_dir,
        &options.sys_executable,
        (poly_python::PYTHON_VERSION.0, poly_python::PYTHON_VERSION.1),
    )?;
    Ok(SyncReport {
        project_root,
        target: target_dir,
        packages: plan
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

async fn discover_project(project_dir: &Path) -> Result<VirtualProject, Error> {
    let discovery_cache = uv_cache::Cache::temp().map_err(Error::Cache)?;
    let workspace_cache = WorkspaceCache::default();
    VirtualProject::discover(
        project_dir,
        &DiscoveryOptions {
            members: MemberDiscovery::Existing,
            ..DiscoveryOptions::default()
        },
        &discovery_cache,
        &workspace_cache,
    )
    .await
    .map_err(Error::Workspace)
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
    let target = canonicalize_destination(&target)?;
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

fn canonicalize_destination(path: &Path) -> Result<PathBuf, Error> {
    let filename = path
        .file_name()
        .ok_or_else(|| Error::UnsafeTarget(path.to_path_buf()))?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::UnsafeTarget(path.to_path_buf()))?;
    fs::create_dir_all(parent)?;
    Ok(fs::canonicalize(parent)?.join(filename))
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

async fn download_missing(
    client: &BaseClient,
    missing: &[(&LockedPackage, PathBuf)],
) -> Result<Vec<TempPath>, Error> {
    let mut downloads = Vec::with_capacity(missing.len());
    for (package, destination) in missing {
        let parent = destination.parent().expect("cached wheel has a parent");
        fs::create_dir_all(parent)?;
        downloads.push(download_one(client, package, parent).await?);
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
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    use uv_pep508::VerbatimUrl;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::stage::wheel_sha256;

    fn wheel_bytes() -> Vec<u8> {
        wheel_bytes_for("1.0.0", 42)
    }

    fn wheel_bytes_for(version: &str, value: u8) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (path, contents) in [
            (
                "demo_pkg/__init__.py".to_owned(),
                format!("VALUE = {value}\n"),
            ),
            (
                format!("demo_pkg-{version}.dist-info/WHEEL"),
                "Wheel-Version: 1.0\nGenerator: poly-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n".to_owned(),
            ),
            (
                format!("demo_pkg-{version}.dist-info/METADATA"),
                format!("Metadata-Version: 2.1\nName: demo-pkg\nVersion: {version}\n"),
            ),
            (
                format!("demo_pkg-{version}.dist-info/RECORD"),
                format!(
                    "demo_pkg/__init__.py,,\ndemo_pkg-{version}.dist-info/WHEEL,,\ndemo_pkg-{version}.dist-info/METADATA,,\ndemo_pkg-{version}.dist-info/RECORD,,\n"
                ),
            ),
        ] {
            writer.start_file(path, options).expect("start wheel member");
            writer
                .write_all(contents.as_bytes())
                .expect("write wheel member");
        }
        writer.finish().expect("finish wheel archive").into_inner()
    }

    struct LocalRegistry {
        address: std::net::SocketAddr,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl LocalRegistry {
        fn start(files: Vec<(String, Vec<u8>, String)>) -> Self {
            Self::start_with_auth(files, None)
        }

        fn start_authenticated(files: Vec<(String, Vec<u8>, String)>) -> Self {
            Self::start_with_auth(files, Some("Basic cG9seTpzZWNyZXQ=".to_owned()))
        }

        fn start_with_auth(
            files: Vec<(String, Vec<u8>, String)>,
            expected_authorization: Option<String>,
        ) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local registry");
            let address = listener.local_addr().expect("local registry address");
            let simple = files
                .iter()
                .map(|(filename, _, hash)| {
                    format!(
                        "<a href=\"/files/{filename}#sha256={hash}\" data-core-metadata=\"true\">{filename}</a>"
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let thread = thread::spawn(move || {
                for incoming in listener.incoming() {
                    let mut stream = incoming.expect("accept registry request");
                    let (path, authorization) = read_request(&mut stream);
                    if path == "/shutdown" {
                        break;
                    }
                    let (status, content_type, body): (&str, &str, Vec<u8>) =
                        if expected_authorization
                            .as_ref()
                            .is_some_and(|expected| authorization.as_ref() != Some(expected))
                        {
                            ("401 Unauthorized", "text/plain", b"unauthorized".to_vec())
                        } else if path.trim_end_matches('/') == "/simple/demo-pkg" {
                            (
                                "200 OK",
                                "application/vnd.pypi.simple.v1+html",
                                simple.as_bytes().to_vec(),
                            )
                        } else if let Some(filename) = path
                            .strip_prefix("/files/")
                            .and_then(|path| path.strip_suffix(".metadata"))
                        {
                            files
                                .iter()
                                .find(|(candidate, _, _)| candidate == filename)
                                .map(|(filename, _, _)| {
                                    let version = filename
                                        .strip_prefix("demo_pkg-")
                                        .expect("fixture distribution name")
                                        .split('-')
                                        .next()
                                        .expect("fixture distribution version")
                                        .trim_end_matches(".tar.gz");
                                    (
                                        "200 OK",
                                        "text/plain",
                                        format!(
                                            "Metadata-Version: 2.1\nName: demo-pkg\nVersion: {version}\n"
                                        )
                                        .into_bytes(),
                                    )
                                })
                                .unwrap_or_else(|| {
                                    ("404 Not Found", "text/plain", b"missing".to_vec())
                                })
                        } else if let Some(filename) = path.strip_prefix("/files/") {
                            files
                                .iter()
                                .find(|(candidate, _, _)| candidate == filename)
                                .map(|(_, bytes, _)| {
                                    ("200 OK", "application/octet-stream", bytes.clone())
                                })
                                .unwrap_or_else(|| {
                                    ("404 Not Found", "text/plain", b"missing".to_vec())
                                })
                        } else {
                            ("404 Not Found", "text/plain", b"missing".to_vec())
                        };
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .expect("write registry headers");
                    stream.write_all(&body).expect("write registry body");
                }
            });
            Self {
                address,
                thread: Some(thread),
            }
        }

        fn index_url(&self) -> String {
            format!("http://{}/simple", self.address)
        }

        fn authenticated_index_url(&self) -> String {
            format!("http://poly:secret@{}/simple", self.address)
        }

        fn artifact_url(&self, filename: &str) -> String {
            format!("http://{}/files/{filename}", self.address)
        }
    }

    fn read_request(stream: &mut TcpStream) -> (String, Option<String>) {
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set registry read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("read registry request");
            assert_ne!(count, 0, "registry request ended before its headers");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() <= 64 * 1024, "registry request is too large");
        }
        let request = String::from_utf8_lossy(&request);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            .unwrap_or("/")
            .to_owned();
        let authorization = request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("authorization")
                .then(|| value.trim().to_owned())
        });
        (path, authorization)
    }

    impl Drop for LocalRegistry {
        fn drop(&mut self) {
            if let Ok(mut stream) = TcpStream::connect(self.address) {
                let _ = write!(
                    stream,
                    "GET /shutdown HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    self.address
                );
            }
            if let Some(thread) = self.thread.take() {
                thread.join().expect("join local registry");
            }
        }
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
    fn resolves_latest_version_writes_lock_and_installs() {
        let directory = tempfile::tempdir().expect("create project");
        let mut files = Vec::new();
        let mut old_hash = None;
        for (version, value) in [("1.0.0", 1), ("2.0.0", 2)] {
            let filename = format!("demo_pkg-{version}-py3-none-any.whl");
            let bytes = wheel_bytes_for(version, value);
            let path = directory.path().join(&filename);
            fs::write(&path, &bytes).expect("write registry wheel");
            let hash = wheel_sha256(&path).expect("hash registry wheel");
            if version == "1.0.0" {
                old_hash = Some(hash.clone());
            }
            files.push((filename, bytes, hash));
        }
        let registry = LocalRegistry::start(files);
        fs::write(
            directory.path().join("pyproject.toml"),
            format!(
                r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["demo-pkg"]

[tool.uv]
package = false

[[tool.uv.index]]
name = "local"
url = "{}"
default = true
"#,
                registry.index_url()
            ),
        )
        .expect("write pyproject.toml");
        fs::write(
            directory.path().join("uv.lock"),
            format!(
                r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "demo-pkg"
version = "1.0.0"
source = {{ registry = "{}" }}
wheels = [
    {{ url = "{}", hash = "sha256:{}" }},
]

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "demo-pkg" }}]
"#,
                registry.index_url(),
                registry.artifact_url("demo_pkg-1.0.0-py3-none-any.whl"),
                old_hash.expect("old wheel hash"),
            ),
        )
        .expect("write existing uv.lock");

        let report = sync(&options(directory.path())).expect("resolve and sync project");
        assert_eq!(report.packages, ["demo-pkg==2.0.0"]);
        assert_eq!(report.downloaded, 1);
        assert_eq!(
            fs::read_to_string(report.target.join("demo_pkg/__init__.py"))
                .expect("read installed package"),
            "VALUE = 2\n"
        );
        let lock = fs::read_to_string(directory.path().join("uv.lock")).expect("read uv.lock");
        assert!(lock.contains("version = \"2.0.0\""), "{lock}");
        assert!(!lock.contains("version = \"1.0.0\""), "{lock}");

        let frozen = sync_frozen(&options(directory.path())).expect("reuse generated lock");
        assert_eq!(frozen.packages, ["demo-pkg==2.0.0"]);
        assert_eq!((frozen.downloaded, frozen.cache_hits), (0, 1));
    }

    #[test]
    fn resolves_latest_version_with_a_generic_python_wheel() {
        let directory = tempfile::tempdir().expect("create project");
        let mut files = Vec::new();
        for (version, tag) in [
            ("1.0.0", "py3-none-any"),
            ("2.0.0", "py314-none-any"),
            (
                "3.0.0",
                "py3-none-manylinux_2_17_x86_64.manylinux_2_17_aarch64.win_amd64.win_arm64.macosx_11_0_universal2",
            ),
        ] {
            let filename = format!("demo_pkg-{version}-{tag}.whl");
            let bytes = wheel_bytes_for(version, version.as_bytes()[0] - b'0');
            let path = directory.path().join(&filename);
            fs::write(&path, &bytes).expect("write registry wheel");
            let hash = wheel_sha256(&path).expect("hash registry wheel");
            files.push((filename, bytes, hash));
        }
        let registry = LocalRegistry::start(files);
        fs::write(
            directory.path().join("pyproject.toml"),
            format!(
                r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["demo-pkg"]

[tool.uv]
package = false

[[tool.uv.index]]
name = "local"
url = "{}"
default = true
"#,
                registry.index_url()
            ),
        )
        .expect("write pyproject.toml");

        let report = sync(&options(directory.path())).expect("resolve generic wheel version");
        assert_eq!(report.packages, ["demo-pkg==1.0.0"]);
        let lock = fs::read_to_string(directory.path().join("uv.lock")).expect("read uv.lock");
        assert!(lock.contains("version = \"1.0.0\""), "{lock}");
        assert!(!lock.contains("version = \"2.0.0\""), "{lock}");
        assert!(!lock.contains("version = \"3.0.0\""), "{lock}");
    }

    #[test]
    fn resolves_and_downloads_from_an_authenticated_index() {
        let directory = tempfile::tempdir().expect("create project");
        let filename = "demo_pkg-1.0.0-py3-none-any.whl";
        let bytes = wheel_bytes();
        let path = directory.path().join(filename);
        fs::write(&path, &bytes).expect("write registry wheel");
        let hash = wheel_sha256(&path).expect("hash registry wheel");
        let registry = LocalRegistry::start_authenticated(vec![(filename.to_owned(), bytes, hash)]);
        fs::write(
            directory.path().join("pyproject.toml"),
            format!(
                r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["demo-pkg"]

[tool.uv]
package = false

[[tool.uv.index]]
name = "private"
url = "{}"
default = true
"#,
                registry.authenticated_index_url()
            ),
        )
        .expect("write pyproject.toml");

        let report = sync(&options(directory.path())).expect("sync authenticated package");
        assert_eq!(report.packages, ["demo-pkg==1.0.0"]);
        assert_eq!(report.downloaded, 1);
        assert!(report.target.join("demo_pkg/__init__.py").is_file());
        let lock = fs::read_to_string(directory.path().join("uv.lock")).expect("read uv.lock");
        assert!(!lock.contains("secret"), "{lock}");
    }

    #[test]
    fn resolution_failure_preserves_the_existing_lock() {
        let directory = tempfile::tempdir().expect("create project");
        let filename = "demo_pkg-1.0.0.tar.gz";
        let bytes = b"not a source archive".to_vec();
        let path = directory.path().join(filename);
        fs::write(&path, &bytes).expect("write source fixture");
        let hash = wheel_sha256(&path).expect("hash source fixture");
        let registry = LocalRegistry::start(vec![(filename.to_owned(), bytes, hash)]);
        fs::write(
            directory.path().join("pyproject.toml"),
            format!(
                r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["demo-pkg"]

[tool.uv]
package = false

[[tool.uv.index]]
name = "local"
url = "{}"
default = true
"#,
                registry.index_url()
            ),
        )
        .expect("write pyproject.toml");
        let previous_lock = "existing lock remains unchanged\n";
        fs::write(directory.path().join("uv.lock"), previous_lock).expect("write existing lock");

        let error = sync(&options(directory.path())).expect_err("reject source-only resolution");
        assert!(
            matches!(error, Error::Resolve(_) | Error::UnsupportedSource { .. }),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(directory.path().join("uv.lock")).expect("read existing lock"),
            previous_lock
        );
    }

    #[test]
    fn invalid_target_preserves_the_existing_lock() {
        let directory = tempfile::tempdir().expect("create project");
        fs::write(
            directory.path().join("pyproject.toml"),
            r#"[project]
name = "project"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = []

[tool.uv]
package = false
"#,
        )
        .expect("write pyproject.toml");
        let previous_lock = "existing lock remains unchanged\n";
        fs::write(directory.path().join("uv.lock"), previous_lock).expect("write existing lock");
        let mut options = options(directory.path());
        options.target_dir = Some(directory.path().to_path_buf());

        let error = sync(&options).expect_err("reject project root as installation target");
        assert!(matches!(error, Error::UnsafeTarget(_)), "{error}");
        assert_eq!(
            fs::read_to_string(directory.path().join("uv.lock")).expect("read existing lock"),
            previous_lock
        );
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
