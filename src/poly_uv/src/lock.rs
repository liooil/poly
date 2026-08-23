use std::fs;
use std::path::Path;
use std::str::FromStr;

use uv_configuration::{
    BuildOptions, DependencyGroupsWithDefaults, ExtrasSpecification, InstallOptions,
};
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::{BuiltDist, Dist, Name, ResolvedDist, SourceDist};
use uv_normalize::{DefaultExtras, PackageName};
use uv_pep440::Version;
use uv_pep508::{MarkerEnvironment, MarkerEnvironmentBuilder};
use uv_platform_tags::{AbiTag, Arch, LanguageTag, Os, Platform, PlatformTag, Tags, TagsOptions};
use uv_pypi_types::{HashAlgorithm, HashDigest, ResolverMarkerEnvironment};
use uv_resolver::{Installable, Lock};

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub requires_python: String,
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub url: String,
    pub sha256: String,
}

struct LockTarget<'lock> {
    install_path: &'lock Path,
    lock: &'lock Lock,
    roots: Vec<&'lock PackageName>,
    project_name: Option<&'lock PackageName>,
}

impl<'lock> Installable<'lock> for LockTarget<'lock> {
    fn install_path(&self) -> &'lock Path {
        self.install_path
    }

    fn lock(&self) -> &'lock Lock {
        self.lock
    }

    fn roots(&self) -> impl Iterator<Item = &PackageName> {
        self.roots.iter().copied()
    }

    fn project_name(&self) -> Option<&PackageName> {
        self.project_name
    }
}

/// Validate a uv lockfile and select artifacts reachable from its root project.
pub fn plan_lock(path: &Path) -> Result<InstallPlan, Error> {
    let contents = fs::read_to_string(path).map_err(|source| Error::ReadLock {
        path: path.to_path_buf(),
        source,
    })?;
    let install_path = path.parent().unwrap_or_else(|| Path::new("."));
    plan_lock_contents_at(&contents, install_path)
}

/// Build an install plan from validated uv lockfile contents.
///
/// This uses uv's lock traversal to evaluate dependency markers, resolution
/// forks and transitive reachability. Only the root project's production
/// dependency graph is selected; extras and dependency groups are not enabled.
pub fn plan_lock_contents(contents: &str) -> Result<InstallPlan, Error> {
    plan_lock_contents_at(contents, Path::new("."))
}

pub(crate) fn plan_lock_contents_for_project(
    contents: &str,
    install_path: &Path,
    project_name: Option<&PackageName>,
) -> Result<InstallPlan, Error> {
    plan_lock_contents_for_project_with_version(
        contents,
        install_path,
        project_name,
        poly_python::PYTHON_VERSION,
        &poly_python::python_full_version(),
    )
}

fn plan_lock_contents_for_project_with_version(
    contents: &str,
    install_path: &Path,
    project_name: Option<&PackageName>,
    python_version: (u8, u8, u8),
    python_full_version: &str,
) -> Result<InstallPlan, Error> {
    let lock = Lock::from_toml(contents).map_err(|error| Error::InvalidLock(error.to_string()))?;
    let install_path = std::path::absolute(install_path)?;
    let roots = match project_name {
        Some(project_name) => vec![project_name],
        None if !lock.members().is_empty() => lock.members().iter().collect(),
        None => lock
            .root()
            .map(|root| vec![root.name()])
            .unwrap_or_default(),
    };
    let project_name = project_name.or_else(|| lock.root().map(|root| root.name()));
    let target = LockTarget {
        install_path: &install_path,
        lock: &lock,
        roots,
        project_name,
    };
    plan_target(&target, python_version, python_full_version)
}

fn plan_lock_contents_at(contents: &str, install_path: &Path) -> Result<InstallPlan, Error> {
    plan_lock_contents_for_project(contents, install_path, None)
}

fn plan_target(
    target: &LockTarget<'_>,
    python_version: (u8, u8, u8),
    python_full_version: &str,
) -> Result<InstallPlan, Error> {
    let marker_env = marker_environment(python_version, python_full_version)?;
    let version =
        Version::from_str(python_full_version).map_err(|error| Error::InvalidPythonVersion {
            version: python_full_version.to_owned(),
            message: error.to_string(),
        })?;
    if !target.lock.requires_python().contains(&version) {
        return Err(Error::PythonVersion {
            version: version.to_string(),
            requires_python: target.lock.requires_python().to_string(),
        });
    }

    if !target.lock.supported_environments().is_empty()
        && !target
            .lock
            .supported_environments()
            .iter()
            .any(|environment| environment.evaluate(marker_env.markers(), &[]))
    {
        return Err(Error::UnsupportedEnvironment);
    }

    // The concrete implementation tag is irrelevant to Poly's accepted
    // artifacts: after uv computes the graph, locked_package below applies the
    // stricter py3-none-any RustPython boundary. CPython tags let uv retain a
    // source fallback long enough for us to produce a precise rejection.
    let tags = Tags::from_env(
        current_tag_platform()?,
        (python_version.0, python_version.1),
        "cpython",
        (python_version.0, python_version.1),
        TagsOptions::default(),
    )
    .map_err(|error| Error::PlatformTags(error.to_string()))?;
    let extras = ExtrasSpecification::default().with_defaults(DefaultExtras::default());
    let groups = DependencyGroupsWithDefaults::none();
    let install_options = InstallOptions::new(
        true,
        false,
        false,
        false,
        false,
        false,
        Vec::new(),
        Vec::new(),
    );
    let resolution = if target.lock.conflicts().is_empty() && !target.roots.is_empty() {
        let roots = target
            .roots
            .iter()
            .map(|name| {
                target
                    .lock
                    .find_by_name(name)
                    .map_err(Error::LockResolution)?
                    .ok_or_else(|| {
                        Error::LockResolution(format!(
                            "project package `{name}` is missing from the lockfile"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        target.lock.to_resolution(
            target.install_path,
            roots,
            target.project_name,
            &marker_env,
            &tags,
            &extras,
            &groups,
            &BuildOptions::default(),
            &install_options,
        )
    } else {
        Installable::to_resolution(
            target,
            &marker_env,
            &tags,
            &extras,
            &groups,
            &BuildOptions::default(),
            &install_options,
        )
    }
    .map_err(|error| Error::LockResolution(error.to_string()))?;

    let mut packages = resolution
        .hashes()
        .map(|(distribution, hashes)| locked_package(distribution, hashes))
        .collect::<Result<Vec<_>, Error>>()?;
    packages.sort_by(|left, right| {
        (&left.name, &left.version, &left.filename).cmp(&(
            &right.name,
            &right.version,
            &right.filename,
        ))
    });

    Ok(InstallPlan {
        requires_python: target.lock.requires_python().to_string(),
        packages,
    })
}

fn locked_package(
    distribution: &ResolvedDist,
    resolution_hashes: &[HashDigest],
) -> Result<LockedPackage, Error> {
    let name = distribution.name().to_string();
    let version = distribution
        .version()
        .map(ToString::to_string)
        .unwrap_or_else(|| "<unknown>".to_owned());
    let ResolvedDist::Installable { dist, .. } = distribution else {
        return Err(Error::UnsupportedSource {
            name,
            version,
            kind: "an already-installed distribution".to_owned(),
        });
    };

    match dist.as_ref() {
        Dist::Built(BuiltDist::Registry(distribution)) => {
            let available = distribution
                .wheels
                .iter()
                .map(|wheel| wheel.filename.to_string())
                .collect::<Vec<_>>();
            let Some(wheel) = distribution
                .wheels
                .iter()
                .find(|wheel| is_generic_python3_wheel(&wheel.filename))
            else {
                return Err(Error::NoPureWheel {
                    name,
                    version,
                    available,
                });
            };
            let filename = wheel.filename.to_string();
            let sha256 = sha256(wheel.file.hashes.iter()).ok_or_else(|| Error::MissingHash {
                name: name.clone(),
                version: version.clone(),
                filename: filename.clone(),
            })?;
            let url = wheel
                .file
                .url
                .to_url()
                .map_err(|error| Error::ArtifactUrl {
                    filename: filename.clone(),
                    message: error.to_string(),
                })?
                .as_str()
                .to_owned();
            Ok(LockedPackage {
                name,
                version,
                filename,
                url,
                sha256,
            })
        }
        Dist::Built(BuiltDist::DirectUrl(wheel)) => {
            if !is_generic_python3_wheel(&wheel.filename) {
                return Err(Error::NoPureWheel {
                    name,
                    version,
                    available: vec![wheel.filename.to_string()],
                });
            }
            let filename = wheel.filename.to_string();
            let sha256 = sha256(resolution_hashes.iter()).ok_or_else(|| Error::MissingHash {
                name: name.clone(),
                version: version.clone(),
                filename: filename.clone(),
            })?;
            Ok(LockedPackage {
                name,
                version,
                filename,
                url: wheel.url.raw().as_str().to_owned(),
                sha256,
            })
        }
        Dist::Built(BuiltDist::Path(_)) => Err(Error::UnsupportedSource {
            name,
            version,
            kind: "an unhashed local wheel".to_owned(),
        }),
        Dist::Built(BuiltDist::GitPath(_)) => Err(Error::UnsupportedSource {
            name,
            version,
            kind: "a Git wheel".to_owned(),
        }),
        Dist::Source(source) => Err(Error::UnsupportedSource {
            name,
            version,
            kind: source_description(source).to_owned(),
        }),
    }
}

fn sha256<'a>(hashes: impl IntoIterator<Item = &'a HashDigest>) -> Option<String> {
    hashes
        .into_iter()
        .find(|hash| hash.algorithm() == HashAlgorithm::Sha256)
        .map(|hash| hash.digest.to_string())
}

fn source_description(source: &SourceDist) -> &'static str {
    match source {
        SourceDist::Registry(_) => "a registry source distribution",
        SourceDist::DirectUrl(_) => "a URL source distribution",
        SourceDist::GitDirectory(_) | SourceDist::GitPath(_) => "a Git source distribution",
        SourceDist::Path(_) | SourceDist::Directory(_) => "a local source tree",
    }
}

fn marker_environment(
    python_version: (u8, u8, u8),
    python_full_version: &str,
) -> Result<ResolverMarkerEnvironment, Error> {
    let short_version = format!("{}.{}", python_version.0, python_version.1);
    let (platform_machine, platform_release, platform_version) = platform_markers();
    let (os_name, sys_platform, platform_system) = if cfg!(windows) {
        ("nt", "win32", "Windows")
    } else if cfg!(target_os = "macos") {
        ("posix", "darwin", "Darwin")
    } else if cfg!(target_os = "linux") {
        ("posix", "linux", "Linux")
    } else {
        ("posix", std::env::consts::OS, std::env::consts::OS)
    };
    MarkerEnvironment::try_from(MarkerEnvironmentBuilder {
        implementation_name: "rustpython",
        implementation_version: python_full_version,
        os_name,
        platform_machine: &platform_machine,
        platform_python_implementation: "RustPython",
        platform_release: &platform_release,
        platform_system,
        platform_version: &platform_version,
        python_full_version,
        python_version: &short_version,
        sys_platform,
    })
    .map(ResolverMarkerEnvironment::from)
    .map_err(|error| Error::MarkerEnvironment(error.to_string()))
}

#[cfg(unix)]
fn platform_markers() -> (String, String, String) {
    let uname = rustix::system::uname();
    (
        uname.machine().to_string_lossy().into_owned(),
        uname.release().to_string_lossy().into_owned(),
        uname.version().to_string_lossy().into_owned(),
    )
}

#[cfg(windows)]
fn platform_markers() -> (String, String, String) {
    let version = windows_version::OsVersion::current();
    let release = windows_release(
        version.major,
        version.minor,
        version.build,
        windows_version::is_server(),
    );
    (
        if cfg!(target_arch = "aarch64") {
            "ARM64".to_owned()
        } else {
            "AMD64".to_owned()
        },
        release,
        format!("{}.{}.{}", version.major, version.minor, version.build),
    )
}

#[cfg(not(any(unix, windows)))]
fn platform_markers() -> (String, String, String) {
    (
        std::env::consts::ARCH.to_owned(),
        String::new(),
        String::new(),
    )
}

#[cfg(windows)]
fn windows_release(major: u32, minor: u32, build: u32, server: bool) -> String {
    // Keep this table aligned with RustPython's vendored `Lib/platform.py`.
    let releases: &[((u32, u32, u32), &str)] = if server {
        &[
            ((10, 1, 0), "post2025Server"),
            ((10, 0, 26100), "2025Server"),
            ((10, 0, 20348), "2022Server"),
            ((10, 0, 17763), "2019Server"),
            ((6, 4, 0), "2016Server"),
            ((6, 3, 0), "2012ServerR2"),
            ((6, 2, 0), "2012Server"),
            ((6, 1, 0), "2008ServerR2"),
            ((6, 0, 0), "2008Server"),
            ((5, 2, 0), "2003Server"),
            ((5, 0, 0), "2000Server"),
        ]
    } else {
        &[
            ((10, 1, 0), "post11"),
            ((10, 0, 22000), "11"),
            ((6, 4, 0), "10"),
            ((6, 3, 0), "8.1"),
            ((6, 2, 0), "8"),
            ((6, 1, 0), "7"),
            ((6, 0, 0), "Vista"),
            ((5, 2, 3790), "XP64"),
            ((5, 2, 0), "XPMedia"),
            ((5, 1, 0), "XP"),
            ((5, 0, 0), "2000"),
        ]
    };
    releases
        .iter()
        .find_map(|(minimum, release)| {
            ((major, minor, build) >= *minimum).then(|| (*release).to_owned())
        })
        .unwrap_or_else(|| format!("{major}.{minor}"))
}

fn current_tag_platform() -> Result<Platform, Error> {
    let arch = if cfg!(target_arch = "aarch64") {
        Arch::Aarch64
    } else if cfg!(target_arch = "x86_64") {
        Arch::X86_64
    } else {
        return Err(Error::PlatformTags(format!(
            "unsupported architecture `{}`",
            std::env::consts::ARCH
        )));
    };
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Macos {
            major: 11,
            minor: 0,
        }
    } else if cfg!(target_os = "linux") {
        Os::Manylinux {
            major: 2,
            minor: 17,
        }
    } else {
        return Err(Error::PlatformTags(format!(
            "unsupported operating system `{}`",
            std::env::consts::OS
        )));
    };
    Ok(Platform::new(os, arch))
}

pub(crate) fn is_generic_python3_wheel(filename: &WheelFilename) -> bool {
    filename.python_tags().iter().any(|tag| {
        matches!(
            tag,
            LanguageTag::Python {
                major: 3,
                minor: None
            }
        )
    }) && filename.abi_tags().contains(&AbiTag::None)
        && filename.platform_tags().contains(&PlatformTag::Any)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn registry_package(name: &str, wheel: &str) -> String {
        format!(
            r#"[[package]]
name = "{name}"
version = "1.0.0"
source = {{ registry = "https://example.invalid/simple" }}
wheels = [
    {{ url = "https://example.invalid/{wheel}", hash = "sha256:{HASH}" }},
]
"#
        )
    }

    #[test]
    fn selects_only_reachable_marker_branch() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [
    {{ name = "included", marker = "implementation_name == 'rustpython'" }},
    {{ name = "excluded", marker = "implementation_name != 'rustpython'" }},
]

{}
{}
{}"#,
            registry_package("included", "included-1.0.0-py3-none-any.whl"),
            registry_package("excluded", "excluded-1.0.0-py3-none-any.whl"),
            registry_package(
                "unreachable",
                "unreachable-1.0.0-cp314-cp314-manylinux_2_17_x86_64.whl"
            ),
        );

        let plan = plan_lock_contents(&lock).expect("build reachable install plan");
        assert_eq!(
            plan.packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            ["included"]
        );
    }

    #[test]
    fn follows_transitive_dependencies() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "parent" }}]

[[package]]
name = "parent"
version = "1.0.0"
source = {{ registry = "https://example.invalid/simple" }}
dependencies = [{{ name = "child" }}]
wheels = [
    {{ url = "https://example.invalid/parent-1.0.0-py3-none-any.whl", hash = "sha256:{HASH}" }},
]

{}"#,
            registry_package("child", "child-1.0.0-py3-none-any.whl"),
        );

        let plan = plan_lock_contents(&lock).expect("build transitive install plan");
        assert_eq!(
            plan.packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            ["child", "parent"]
        );
    }

    #[test]
    fn rejects_reachable_native_only_package() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "native-only" }}]

{}"#,
            registry_package("native-only", "native_only-1.0.0-cp314-none-any.whl"),
        );

        let error = plan_lock_contents(&lock).expect_err("reject native wheel");
        assert!(
            matches!(
                error,
                Error::NoPureWheel { ref name, .. } if name == "native-only"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn rejects_reachable_source_distribution() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.12"

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [{{ name = "source-only" }}]

[[package]]
name = "source-only"
version = "1.0.0"
source = {{ registry = "https://example.invalid/simple" }}
sdist = {{ url = "https://example.invalid/source_only-1.0.0.tar.gz", hash = "sha256:{HASH}" }}
"#,
        );

        let error = plan_lock_contents(&lock).expect_err("reject source distribution");
        assert!(
            matches!(
                error,
                Error::UnsupportedSource { ref name, .. } if name == "source-only"
            ),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn rejects_incompatible_python_version() {
        let lock = r#"version = 1
revision = 3
requires-python = ">=3.15"

[[package]]
name = "project"
version = "0.1.0"
source = { virtual = "." }
"#;
        let error = plan_lock_contents(lock).expect_err("reject incompatible Python");
        assert!(matches!(error, Error::PythonVersion { .. }));
    }

    #[test]
    fn evaluates_version_markers_against_the_embedded_prerelease() {
        let lock = format!(
            r#"version = 1
revision = 3
requires-python = ">=3.14"

[[package]]
name = "project"
version = "0.1.0"
source = {{ virtual = "." }}
dependencies = [
    {{ name = "alpha-only", marker = "implementation_version < '3.14'" }},
]

{}"#,
            registry_package("alpha-only", "alpha_only-1.0.0-py3-none-any.whl"),
        );
        let plan = plan_lock_contents_for_project_with_version(
            &lock,
            Path::new("."),
            None,
            (3, 14, 0),
            "3.14.0a0",
        )
        .expect("evaluate prerelease marker");
        assert_eq!(plan.packages[0].name, "alpha-only");
    }

    #[test]
    fn lets_uv_reject_invalid_lockfile() {
        let error = plan_lock_contents("version = 999\nrequires-python = \">=3.12\"\n")
            .expect_err("reject unsupported uv lock version");
        assert!(matches!(error, Error::InvalidLock(_)));
    }
}
