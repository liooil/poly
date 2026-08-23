use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use uv_cache::{Cache, Refresh};
use uv_client::{BaseClientBuilder, FlatIndexClient, RegistryClientBuilder};
use uv_configuration::{
    BuildKind, BuildOptions, BuildOutput, Concurrency, Constraints, Excludes, NoBinary, NoBuild,
    NoSources, Override, Overrides, PackageOverride,
};
use uv_distribution::{BuildRequires, DistributionDatabase};
use uv_distribution_filename::DistFilename;
use uv_distribution_types::{
    CachedDist, ConfigSettings, DependencyMetadata, ExtraBuildRequires, ExtraBuildVariables,
    HashGeneration, Index, IndexCapabilities, IndexFormat, IndexLocations, PackageConfigSettings,
    Requirement, RequiresPython, SourceDist,
};
use uv_git::GitResolver;
use uv_normalize::PackageName;
use uv_pep440::Version;
use uv_pep508::RequirementOrigin;
use uv_platform_tags::{Arch, Os, Platform, Tags, TagsOptions};
use uv_pypi_types::BuildRequires as PepBuildRequires;
use uv_python::{Interpreter, PythonEnvironment};
use uv_requirements::{ExtrasResolver, LookaheadResolver};
use uv_resolver::{
    Exclusions, FlatIndex, InMemoryIndex, Lock, Manifest, OptionsBuilder, Preferences,
    PythonRequirement, Resolver, ResolverEnvironment, ResolverManifest,
};
use uv_types::{
    BuildArena, BuildContext, BuildIsolation, BuildStack, EmptyInstalledPackages, HashStrategy,
    ResolvedRequirements, SourceBuildTrait,
};
use uv_workspace::dependency_groups::FlatDependencyGroup;
use uv_workspace::{VirtualProject, Workspace, WorkspaceCache};

use crate::Error;
use crate::lock::{current_tag_platform, marker_environment};

pub(crate) async fn resolve_project_lock(
    project: &VirtualProject,
    cache_dir: &Path,
) -> Result<String, Error> {
    let workspace = project.workspace();
    tokio::fs::create_dir_all(cache_dir).await?;
    let cache =
        Cache::from_path(cache_dir).with_refresh(Refresh::from_args(Some(true), Vec::new()));
    let workspace_cache = WorkspaceCache::default();
    let sources = NoSources::default();
    let indexes = project_indexes(workspace);
    let client_builder = client_builder_for_indexes(&indexes)?;
    let (flat_indexes, registry_indexes): (Vec<_>, Vec<_>) = indexes
        .into_iter()
        .partition(|index| index.format == IndexFormat::Flat);
    let index_locations = IndexLocations::new(registry_indexes, flat_indexes, false);

    let marker_env = marker_environment(
        poly_python::PYTHON_VERSION,
        &poly_python::python_full_version(),
    )?;
    let platform = current_tag_platform()?;
    let client = RegistryClientBuilder::new(client_builder, cache.clone())
        .index_locations(index_locations.clone())
        .markers(marker_env.markers())
        .platform(&platform)
        .build()
        .map_err(|error| Error::HttpClient(error.to_string()))?;

    let requirements = lower_requirements(
        workspace,
        workspace.requirements(),
        &index_locations,
        &sources,
        &cache,
        &workspace_cache,
        client.credentials_cache(),
    )
    .await?;
    let constraints = lower_requirements(
        workspace,
        workspace.constraints(),
        &index_locations,
        &sources,
        &cache,
        &workspace_cache,
        client.credentials_cache(),
    )
    .await?;
    let build_constraints = lower_requirements(
        workspace,
        workspace.build_constraints(),
        &index_locations,
        &sources,
        &cache,
        &workspace_cache,
        client.credentials_cache(),
    )
    .await?;
    let overrides = lower_overrides(
        workspace,
        workspace.overrides(),
        &index_locations,
        &sources,
        &cache,
        &workspace_cache,
        client.credentials_cache(),
    )
    .await?;
    let dependency_groups = lower_dependency_groups(
        workspace,
        workspace.workspace_dependency_groups()?,
        &index_locations,
        &sources,
        &cache,
        &workspace_cache,
        client.credentials_cache(),
    )
    .await?;

    let excludes = workspace.exclude_dependencies();
    let resolver_constraints = Constraints::from_requirements(constraints.iter().cloned());
    let resolver_overrides = Overrides::from_entries(overrides.clone())?;
    let resolver_excludes = Excludes::from_entries(excludes.iter().cloned());
    let requires_python = requires_python(workspace)?;
    let embedded_version =
        Version::from_str(&poly_python::python_full_version()).map_err(|error| {
            Error::InvalidPythonVersion {
                version: poly_python::python_full_version(),
                message: error.to_string(),
            }
        })?;
    if !requires_python.contains(&embedded_version) {
        return Err(Error::PythonVersion {
            version: embedded_version.to_string(),
            requires_python: requires_python.to_string(),
        });
    }
    let python_requirement =
        PythonRequirement::from_marker_environment(marker_env.markers(), requires_python);

    let supported_environments = workspace
        .environments()
        .cloned()
        .unwrap_or_default()
        .into_markers();
    let required_environments = workspace
        .required_environments()
        .cloned()
        .unwrap_or_default()
        .into_markers();
    let resolver_env = ResolverEnvironment::universal(supported_environments.clone());
    let mut conflicts = workspace.conflicts()?;
    if let (Some(groups), Some(project)) = (
        &workspace.pyproject_toml().dependency_groups,
        &workspace.pyproject_toml().project,
    ) {
        conflicts.expand_transitive_group_includes(&project.name, groups);
    }
    let build_options = BuildOptions::new(NoBinary::None, NoBuild::All);
    let initial_hasher = HashStrategy::Generate(HashGeneration::Url);
    let concurrency = Concurrency::default();
    let context = WheelOnlyBuildContext::new(
        cache.clone(),
        index_locations.clone(),
        build_options.clone(),
        sources,
        workspace_cache,
    );
    let index = InMemoryIndex::default();
    let flat_index = {
        let flat_client =
            FlatIndexClient::new(client.cached_client(), client.connectivity(), &cache);
        let entries = flat_client
            .fetch_all(index_locations.flat_indexes().map(Index::url))
            .await?;
        FlatIndex::from_entries(entries, None, &initial_hasher, &build_options)
    };

    let database =
        || DistributionDatabase::new(&client, &context, concurrency.downloads_semaphore.clone());
    let mut resolver_requirements = ExtrasResolver::new(&initial_hasher, &index, database())
        .resolve(workspace.members_requirements())
        .await?;
    resolver_requirements.extend(workspace.group_requirements());
    resolver_requirements.extend(requirements.iter().cloned());
    resolver_requirements.extend(
        dependency_groups
            .values()
            .flat_map(|requirements| requirements.iter().cloned()),
    );

    let (lookaheads, hasher) = LookaheadResolver::new(
        &resolver_requirements,
        &resolver_constraints,
        &resolver_overrides,
        &resolver_excludes,
        &initial_hasher,
        &index,
        database(),
    )
    .resolve(&resolver_env)
    .await?;

    let tags = generic_python3_tags()?;
    let workspace_members = workspace
        .packages()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let manifest = Manifest::new(
        resolver_requirements,
        resolver_constraints,
        resolver_overrides,
        resolver_excludes,
        Preferences::default(),
        None,
        workspace_members,
        Exclusions::default(),
        lookaheads,
    );
    let resolver = Resolver::new(
        manifest,
        OptionsBuilder::new().build_options(build_options).build(),
        &python_requirement,
        resolver_env,
        marker_env.markers(),
        conflicts.clone(),
        Some(&tags),
        &flat_index,
        &index,
        &hasher,
        &context,
        EmptyInstalledPackages,
        database(),
    )?;
    let resolution = resolver.resolve().await?;

    let lock_manifest = ResolverManifest::new(
        lock_members(workspace),
        requirements,
        constraints,
        overrides,
        excludes,
        build_constraints,
        dependency_groups,
        context.dependency_metadata.values().cloned(),
    )
    .relative_to(workspace.install_path())?;
    let lock = Lock::from_resolution(
        &resolution,
        lock_manifest,
        workspace.install_path(),
        supported_environments,
        &index_locations,
    )?
    .with_conflicts(conflicts)
    .with_required_environments(required_environments);
    lock.to_toml()
        .map_err(|error| Error::LockSerialize(error.to_string()))
}

pub(crate) fn project_indexes(workspace: &Workspace) -> Vec<Index> {
    let mut indexes = Vec::new();
    for index in workspace
        .indexes()
        .iter()
        .chain(workspace.packages().values().flat_map(|member| {
            member
                .pyproject_toml()
                .tool
                .as_ref()
                .and_then(|tool| tool.uv.as_ref())
                .and_then(|uv| uv.index.as_ref())
                .into_iter()
                .flatten()
        }))
    {
        if !indexes.contains(index) {
            indexes.push(index.clone());
        }
    }
    indexes
}

pub(crate) fn client_builder_for_indexes(
    indexes: &[Index],
) -> Result<BaseClientBuilder<'static>, Error> {
    let client_builder = BaseClientBuilder::default();
    for index in indexes {
        if let Some(credentials) = index.credentials()? {
            if let Some(root_url) = index.root_url() {
                client_builder.store_credentials(&root_url, credentials.clone());
            }
            client_builder.store_credentials(index.raw_url(), credentials);
        }
    }
    Ok(client_builder)
}

fn lock_members(workspace: &Workspace) -> Vec<PackageName> {
    let mut members = workspace.packages().keys().cloned().collect::<Vec<_>>();
    if members.len() == 1 && !workspace.is_non_project() {
        members.clear();
    }
    members
}

fn requires_python(workspace: &Workspace) -> Result<RequiresPython, Error> {
    let sources =
        workspace.requires_python(&uv_configuration::DependencyGroupsWithDefaults::none())?;
    if sources.is_empty() {
        return Ok(RequiresPython::greater_than_equal_version(&Version::new([
            u64::from(poly_python::PYTHON_VERSION.0),
            u64::from(poly_python::PYTHON_VERSION.1),
        ])));
    }
    RequiresPython::intersection(sources.values()).ok_or(Error::EmptyPythonRange)
}

fn generic_python3_tags() -> Result<Tags, Error> {
    // uv does not expose a custom Tags constructor. A private sentinel platform
    // leaves py3-none-any as the only expected published match; lock validation is exact.
    let platform = Platform::new(
        Os::Haiku {
            release: "0_poly_pure_wheel_only".to_owned(),
        },
        Arch::Wasm32,
    );
    Tags::from_env(platform, (3, 0), "pyston", (3, 0), TagsOptions::default())
        .map_err(|error| Error::PlatformTags(error.to_string()))
}

async fn lower_requirements(
    workspace: &Workspace,
    requirements: Vec<uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>>,
    locations: &IndexLocations,
    sources: &NoSources,
    cache: &Cache,
    workspace_cache: &WorkspaceCache,
    credentials: &uv_auth::CredentialsCache,
) -> Result<Vec<Requirement>, Error> {
    let name = workspace
        .pyproject_toml()
        .project
        .as_ref()
        .map(|project| project.name.clone());
    let metadata = BuildRequires::from_workspace(
        PepBuildRequires {
            name,
            requires_dist: requirements,
        },
        workspace,
        locations,
        sources,
        cache,
        workspace_cache,
        credentials,
    )
    .await?;
    Ok(metadata
        .requires_dist
        .into_iter()
        .map(|requirement| requirement.with_origin(RequirementOrigin::Workspace))
        .collect())
}

async fn lower_overrides(
    workspace: &Workspace,
    overrides: Vec<Override<uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>>>,
    locations: &IndexLocations,
    sources: &NoSources,
    cache: &Cache,
    workspace_cache: &WorkspaceCache,
    credentials: &uv_auth::CredentialsCache,
) -> Result<Vec<Override<Requirement>>, Error> {
    let mut lowered = Vec::new();
    for entry in overrides {
        match entry {
            Override::Requirement(requirement) => {
                lowered.extend(
                    lower_requirements(
                        workspace,
                        vec![requirement],
                        locations,
                        sources,
                        cache,
                        workspace_cache,
                        credentials,
                    )
                    .await?
                    .into_iter()
                    .map(Override::Requirement),
                );
            }
            Override::Package(package) => lowered.push(Override::Package(PackageOverride {
                package: package.package,
                dependencies: lower_requirements(
                    workspace,
                    package.dependencies.into_vec(),
                    locations,
                    sources,
                    cache,
                    workspace_cache,
                    credentials,
                )
                .await?
                .into_boxed_slice(),
            })),
        }
    }
    Ok(lowered)
}

async fn lower_dependency_groups(
    workspace: &Workspace,
    groups: BTreeMap<uv_normalize::GroupName, FlatDependencyGroup>,
    locations: &IndexLocations,
    sources: &NoSources,
    cache: &Cache,
    workspace_cache: &WorkspaceCache,
    credentials: &uv_auth::CredentialsCache,
) -> Result<BTreeMap<uv_normalize::GroupName, Vec<Requirement>>, Error> {
    let mut lowered = BTreeMap::new();
    for (name, group) in groups {
        lowered.insert(
            name,
            lower_requirements(
                workspace,
                group.requirements,
                locations,
                sources,
                cache,
                workspace_cache,
                credentials,
            )
            .await?,
        );
    }
    Ok(lowered)
}

#[derive(Debug, thiserror::Error)]
#[error("source distribution builds are disabled in Poly")]
struct BuildsDisabled;

impl uv_errors::Hint for BuildsDisabled {
    fn hints(&self) -> uv_errors::Hints<'_> {
        uv_errors::Hints::none()
    }
}

impl uv_distribution_types::IsBuildBackendError for BuildsDisabled {
    fn is_build_backend_error(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct DisabledSourceBuilder;

impl SourceBuildTrait for DisabledSourceBuilder {
    async fn metadata(&mut self) -> Result<Option<PathBuf>, uv_types::AnyErrorBuild> {
        Err(BuildsDisabled.into())
    }

    async fn wheel(&self, _wheel_dir: &Path) -> Result<String, uv_types::AnyErrorBuild> {
        Err(BuildsDisabled.into())
    }
}

struct WheelOnlyBuildContext {
    cache: Cache,
    git: GitResolver,
    build_arena: BuildArena<DisabledSourceBuilder>,
    capabilities: IndexCapabilities,
    dependency_metadata: DependencyMetadata,
    build_options: BuildOptions,
    config_settings: ConfigSettings,
    config_settings_package: PackageConfigSettings,
    sources: NoSources,
    locations: IndexLocations,
    workspace_cache: WorkspaceCache,
    extra_build_requires: ExtraBuildRequires,
    extra_build_variables: ExtraBuildVariables,
}

impl WheelOnlyBuildContext {
    fn new(
        cache: Cache,
        locations: IndexLocations,
        build_options: BuildOptions,
        sources: NoSources,
        workspace_cache: WorkspaceCache,
    ) -> Self {
        Self {
            cache,
            git: GitResolver::default(),
            build_arena: BuildArena::default(),
            capabilities: IndexCapabilities::default(),
            dependency_metadata: DependencyMetadata::default(),
            build_options,
            config_settings: ConfigSettings::default(),
            config_settings_package: PackageConfigSettings::default(),
            sources,
            locations,
            workspace_cache,
            extra_build_requires: ExtraBuildRequires::default(),
            extra_build_variables: ExtraBuildVariables::default(),
        }
    }
}

#[allow(refining_impl_trait)]
impl BuildContext for WheelOnlyBuildContext {
    type SourceDistBuilder = DisabledSourceBuilder;

    async fn interpreter(&self) -> &Interpreter {
        panic!("Poly's wheel-only resolver never requests a CPython interpreter")
    }

    fn cache(&self) -> &Cache {
        &self.cache
    }

    fn git(&self) -> &GitResolver {
        &self.git
    }

    fn build_arena(&self) -> &BuildArena<Self::SourceDistBuilder> {
        &self.build_arena
    }

    fn capabilities(&self) -> &IndexCapabilities {
        &self.capabilities
    }

    fn dependency_metadata(&self) -> &DependencyMetadata {
        &self.dependency_metadata
    }

    fn build_options(&self) -> &BuildOptions {
        &self.build_options
    }

    fn build_isolation(&self) -> BuildIsolation<'_> {
        BuildIsolation::Isolated
    }

    fn config_settings(&self) -> &ConfigSettings {
        &self.config_settings
    }

    fn config_settings_package(&self) -> &PackageConfigSettings {
        &self.config_settings_package
    }

    fn sources(&self) -> &NoSources {
        &self.sources
    }

    fn locations(&self) -> &IndexLocations {
        &self.locations
    }

    fn workspace_cache(&self) -> &WorkspaceCache {
        &self.workspace_cache
    }

    fn extra_build_requires(&self) -> &ExtraBuildRequires {
        &self.extra_build_requires
    }

    fn extra_build_variables(&self) -> &ExtraBuildVariables {
        &self.extra_build_variables
    }

    async fn resolve(
        &self,
        _requirements: &[Requirement],
        _build_stack: &BuildStack,
    ) -> Result<ResolvedRequirements, BuildsDisabled> {
        Err(BuildsDisabled)
    }

    async fn install(
        &self,
        _requirements: &ResolvedRequirements,
        _venv: &PythonEnvironment,
        _build_stack: &BuildStack,
    ) -> Result<Vec<CachedDist>, BuildsDisabled> {
        Err(BuildsDisabled)
    }

    async fn setup_build(
        &self,
        _source: &Path,
        _subdirectory: Option<&Path>,
        _install_path: &Path,
        _stop_discovery_at: Option<&Path>,
        _version_id: Option<&str>,
        _dist: Option<&SourceDist>,
        _sources: &NoSources,
        _build_kind: BuildKind,
        _build_output: BuildOutput,
        _build_stack: BuildStack,
    ) -> Result<Self::SourceDistBuilder, BuildsDisabled> {
        Err(BuildsDisabled)
    }

    async fn direct_build(
        &self,
        _source: &Path,
        _subdirectory: Option<&Path>,
        _output_dir: &Path,
        _sources: NoSources,
        _build_kind: BuildKind,
        _version_id: Option<&str>,
    ) -> Result<Option<DistFilename>, BuildsDisabled> {
        Err(BuildsDisabled)
    }
}
