use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::Path;

use fs_err::File;
use uv_install_wheel::read_record;
use uv_pypi_types::ModuleName;

use crate::{Error, LockedPackage};

/// Compatibility state for one staged distribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityStatus {
    /// The package passed the static artifact scan and has import targets.
    StaticCompatible,
    /// Every derived import target loaded successfully in RustPython.
    SmokeCompatible,
    /// The package passed the static scan but provides no safe import target.
    Unverified,
    /// Static artifacts or a RustPython import failed validation.
    Incompatible,
}

/// Machine-readable class of compatibility diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityDiagnosticKind {
    /// A native dynamic library unsupported by the embedded RustPython runtime.
    NativeArtifact,
    /// CPython bytecode without corresponding Python source.
    SourcelessBytecode,
    /// A derived module failed to import with RustPython.
    ImportFailure,
    /// The distribution exposes no safe Python source import target.
    NoImportTargets,
}

impl CompatibilityDiagnosticKind {
    fn is_error(self) -> bool {
        matches!(
            self,
            Self::NativeArtifact | Self::SourcelessBytecode | Self::ImportFailure
        )
    }
}

/// One compatibility finding for a staged distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityDiagnostic {
    /// Stable diagnostic category.
    pub kind: CompatibilityDiagnosticKind,
    /// Human-readable detail including the relevant artifact or traceback.
    pub message: String,
}

/// Static and optional smoke-import results for one distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageCompatibility {
    /// Normalized distribution name.
    pub name: String,
    /// Locked distribution version.
    pub version: String,
    /// Safe module names derived from the installed `RECORD`.
    pub imports: Vec<String>,
    /// Strongest validation state reached by this sync.
    pub status: CompatibilityStatus,
    /// Findings collected during static and dynamic validation.
    pub diagnostics: Vec<CompatibilityDiagnostic>,
}

/// Compatibility results for every distribution in a staged environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    /// Whether RustPython smoke imports were requested and completed.
    pub smoke_checked: bool,
    /// Per-distribution results in lock traversal order.
    pub packages: Vec<PackageCompatibility>,
}

impl CompatibilityReport {
    /// Number of unique derived module imports represented by the report.
    pub fn import_count(&self) -> usize {
        self.packages
            .iter()
            .flat_map(|package| package.imports.iter())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Number of distributions in one compatibility state.
    pub fn count(&self, status: CompatibilityStatus) -> usize {
        self.packages
            .iter()
            .filter(|package| package.status == status)
            .count()
    }

    /// Whether any distribution must block the environment commit.
    pub fn has_failures(&self) -> bool {
        self.packages
            .iter()
            .any(|package| package.status == CompatibilityStatus::Incompatible)
    }
}

impl fmt::Display for CompatibilityReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let failures = self.count(CompatibilityStatus::Incompatible);
        write!(
            formatter,
            "Python compatibility check failed for {failures} package{}",
            if failures == 1 { "" } else { "s" }
        )?;
        for package in self
            .packages
            .iter()
            .filter(|package| package.status == CompatibilityStatus::Incompatible)
        {
            write!(formatter, "\n  {}=={}", package.name, package.version)?;
            for diagnostic in package
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.kind.is_error())
            {
                for line in diagnostic.message.lines() {
                    write!(formatter, "\n    {line}")?;
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn inspect_staged_package(
    package: &LockedPackage,
    dist_info: &Path,
) -> Result<PackageCompatibility, Error> {
    let record = read_record(File::open(dist_info.join("RECORD"))?)?;
    let paths = record
        .into_iter()
        .filter_map(|entry| normalize_record_path(&entry.path))
        .collect::<BTreeSet<_>>();
    let mut diagnostics = Vec::new();

    for path in &paths {
        let filename = path.rsplit('/').next().unwrap_or(path);
        let filename = filename.to_ascii_lowercase();
        if [".so", ".pyd", ".dylib", ".dll"]
            .iter()
            .any(|suffix| filename.ends_with(suffix))
        {
            diagnostics.push(CompatibilityDiagnostic {
                kind: CompatibilityDiagnosticKind::NativeArtifact,
                message: format!("contains unsupported native artifact `{path}`"),
            });
        }
        if filename.ends_with(".pyc")
            && let Some(source) = bytecode_source_path(path)
            && !paths.contains(&source)
        {
            diagnostics.push(CompatibilityDiagnostic {
                kind: CompatibilityDiagnosticKind::SourcelessBytecode,
                message: format!("contains CPython bytecode without source `{path}`"),
            });
        }
    }

    let imports = import_candidates(&paths);
    if imports.is_empty() {
        diagnostics.push(CompatibilityDiagnostic {
            kind: CompatibilityDiagnosticKind::NoImportTargets,
            message: "provides no importable Python source modules".to_owned(),
        });
    }
    let status = if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind.is_error())
    {
        CompatibilityStatus::Incompatible
    } else if imports.is_empty() {
        CompatibilityStatus::Unverified
    } else {
        CompatibilityStatus::StaticCompatible
    };

    Ok(PackageCompatibility {
        name: package.name.clone(),
        version: package.version.clone(),
        imports,
        status,
        diagnostics,
    })
}

pub(crate) fn run_smoke_imports(
    report: &mut CompatibilityReport,
    site_packages: &Path,
) -> Result<(), Error> {
    let modules = report
        .packages
        .iter()
        .filter(|package| package.status == CompatibilityStatus::StaticCompatible)
        .flat_map(|package| package.imports.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if modules.is_empty() {
        report.smoke_checked = true;
        return Ok(());
    }
    let results = poly_python::smoke_imports(site_packages, &modules)?
        .into_iter()
        .map(|result| (result.module, result.traceback))
        .collect::<HashMap<_, _>>();

    for package in &mut report.packages {
        if package.status != CompatibilityStatus::StaticCompatible {
            continue;
        }
        for module in &package.imports {
            match results.get(module) {
                Some(None) => {}
                Some(Some(traceback)) => {
                    package.diagnostics.push(CompatibilityDiagnostic {
                        kind: CompatibilityDiagnosticKind::ImportFailure,
                        message: format!("failed to import `{module}`:\n{traceback}"),
                    });
                }
                None => {
                    package.diagnostics.push(CompatibilityDiagnostic {
                        kind: CompatibilityDiagnosticKind::ImportFailure,
                        message: format!("RustPython returned no result for import `{module}`"),
                    });
                }
            }
        }
        package.status = if package
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind.is_error())
        {
            CompatibilityStatus::Incompatible
        } else {
            CompatibilityStatus::SmokeCompatible
        };
    }
    report.smoke_checked = true;
    Ok(())
}

fn normalize_record_path(path: &str) -> Option<String> {
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            component => components.push(component),
        }
    }
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.ends_with(".dist-info"))
        || components
            .first()
            .is_some_and(|component| component.ends_with(".data"))
    {
        return None;
    }
    Some(components.join("/"))
}

fn bytecode_source_path(path: &str) -> Option<String> {
    let mut components = path.split('/').collect::<Vec<_>>();
    let filename = components.pop()?;
    let stem = filename.strip_suffix(".pyc")?;
    if components.last() == Some(&"__pycache__") {
        components.pop();
        let source_stem = stem.split('.').next()?;
        components.push(source_stem);
    } else {
        components.push(stem);
    }
    Some(format!("{}.py", components.join("/")))
}

fn import_candidates(paths: &BTreeSet<String>) -> Vec<String> {
    let initializers = paths
        .iter()
        .filter_map(|path| {
            let components = path.split('/').collect::<Vec<_>>();
            (components.last() == Some(&"__init__.py"))
                .then(|| components[..components.len() - 1].join("."))
        })
        .collect::<BTreeSet<_>>();
    let mut candidates = BTreeSet::new();

    for path in paths {
        let Some(path) = path.strip_suffix(".py") else {
            continue;
        };
        let mut components = path.split('/').collect::<Vec<_>>();
        if components.last() == Some(&"__init__") {
            components.pop();
        }
        if components.is_empty() {
            continue;
        }
        let full_name = components.join(".");
        let candidate = (1..=components.len())
            .map(|length| components[..length].join("."))
            .find(|prefix| initializers.contains(prefix))
            .unwrap_or(full_name);
        if ModuleName::from_components(candidate.split('.')).is_ok() {
            candidates.insert(candidate);
        }
    }

    let mut by_root = BTreeMap::new();
    for candidate in candidates {
        let root = candidate.split('.').next().unwrap_or(&candidate);
        by_root.entry(root.to_owned()).or_insert(candidate);
    }
    by_root.into_values().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn derives_normal_and_namespace_imports() {
        let paths = [
            "normal/__init__.py",
            "normal/internal.py",
            "namespace/feature/__init__.py",
            "namespace/feature/api.py",
            "namespace/standalone.py",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();

        assert_eq!(import_candidates(&paths), ["namespace.feature", "normal"]);
    }

    #[test]
    fn maps_bytecode_cache_paths_to_their_sources() {
        assert_eq!(
            bytecode_source_path("package/__pycache__/module.cpython-314.pyc"),
            Some("package/module.py".to_owned())
        );
        assert_eq!(
            bytecode_source_path("package/module.pyc"),
            Some("package/module.py".to_owned())
        );
    }

    #[test]
    fn reports_native_artifacts_and_sourceless_bytecode_from_record() {
        let directory = tempfile::tempdir().expect("create staged environment");
        let dist_info = directory.path().join("demo_pkg-1.0.0.dist-info");
        fs::create_dir_all(&dist_info).expect("create dist-info directory");
        fs::write(
            dist_info.join("RECORD"),
            "demo_pkg/__init__.py,,\ndemo_pkg/native.so,,\ndemo_pkg/__pycache__/legacy.cpython-314.pyc,,\ndemo_pkg-1.0.0.dist-info/RECORD,,\n",
        )
        .expect("write RECORD");
        let package = LockedPackage {
            name: "demo-pkg".to_owned(),
            version: "1.0.0".to_owned(),
            filename: "demo_pkg-1.0.0-py3-none-any.whl".to_owned(),
            url: "https://example.invalid/demo.whl".to_owned(),
            sha256: "unused".to_owned(),
        };

        let compatibility =
            inspect_staged_package(&package, &dist_info).expect("inspect staged package");

        assert_eq!(compatibility.status, CompatibilityStatus::Incompatible);
        assert_eq!(compatibility.imports, ["demo_pkg"]);
        assert!(compatibility.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == CompatibilityDiagnosticKind::NativeArtifact
                && diagnostic.message.contains("native.so")
        }));
        assert!(compatibility.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == CompatibilityDiagnosticKind::SourcelessBytecode
                && diagnostic.message.contains("legacy.cpython-314.pyc")
        }));
    }

    #[test]
    fn leaves_data_only_distributions_unverified() {
        let directory = tempfile::tempdir().expect("create staged environment");
        let dist_info = directory.path().join("demo_data-1.0.0.dist-info");
        fs::create_dir_all(&dist_info).expect("create dist-info directory");
        fs::write(
            dist_info.join("RECORD"),
            "demo_data/resources/schema.json,,\ndemo_data-1.0.0.dist-info/RECORD,,\n",
        )
        .expect("write RECORD");
        let package = LockedPackage {
            name: "demo-data".to_owned(),
            version: "1.0.0".to_owned(),
            filename: "demo_data-1.0.0-py3-none-any.whl".to_owned(),
            url: "https://example.invalid/demo.whl".to_owned(),
            sha256: "unused".to_owned(),
        };

        let compatibility =
            inspect_staged_package(&package, &dist_info).expect("inspect staged package");

        assert_eq!(compatibility.status, CompatibilityStatus::Unverified);
        assert!(compatibility.imports.is_empty());
        assert_eq!(
            compatibility.diagnostics[0].kind,
            CompatibilityDiagnosticKind::NoImportTargets
        );
    }
}
