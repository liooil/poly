use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContext {
    pub invocation_dir: PathBuf,
    pub root: PathBuf,
    pub package_json: Option<PathBuf>,
    pub pyproject_toml: Option<PathBuf>,
    pub bun_lock: Option<PathBuf>,
    pub uv_lock: Option<PathBuf>,
    pub poly_toml: Option<PathBuf>,
}

impl ProjectContext {
    pub fn discover(start: &Path) -> std::io::Result<Self> {
        let invocation_dir = if start.is_dir() {
            start.to_path_buf()
        } else {
            start
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .to_path_buf()
        };

        let root = find_project_root(&invocation_dir).unwrap_or_else(|| invocation_dir.clone());
        Ok(Self {
            invocation_dir,
            package_json: existing_file(&root, "package.json"),
            pyproject_toml: existing_file(&root, "pyproject.toml"),
            bun_lock: existing_file(&root, "bun.lock"),
            uv_lock: existing_file(&root, "uv.lock"),
            poly_toml: existing_file(&root, "poly.toml"),
            root,
        })
    }

    pub fn has_python_project(&self) -> bool {
        self.pyproject_toml.is_some()
    }
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|directory| {
        ["poly.toml", "pyproject.toml", "package.json"]
            .iter()
            .find(|name| directory.join(name).is_file())
            .map(|_| directory.to_path_buf())
    })
}

fn existing_file(root: &Path, name: &str) -> Option<PathBuf> {
    let path = root.join(name);
    path.is_file().then_some(path)
}
