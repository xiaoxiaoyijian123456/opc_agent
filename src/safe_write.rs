use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct SafeFileWriter {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct WriteOutcome {
    pub path: PathBuf,
    pub display: String,
}

impl SafeFileWriter {
    pub fn new(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root)
            .map_err(|error| format!("Cannot create code root {}: {error}", root.display()))?;
        let root = root
            .canonicalize()
            .map_err(|error| format!("Cannot resolve code root {}: {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("Code root is not a directory: {}", root.display()));
        }
        Ok(Self { root })
    }

    pub fn resolve(&self, raw: &str) -> Result<(PathBuf, String), String> {
        let mut normalized = raw.replace('\\', "/").trim().to_string();
        if let Some(value) = normalized.strip_prefix("./") {
            normalized = value.to_string();
        }
        if let Some(value) = normalized.strip_prefix("workspace/") {
            normalized = value.to_string();
        }
        if let Some(value) = normalized.strip_prefix("code/") {
            normalized = value.to_string();
        }
        if normalized.is_empty() || Path::new(&normalized).is_absolute() {
            return Err("File path must be a non-empty relative path".into());
        }

        let mut relative = PathBuf::new();
        for component in Path::new(&normalized).components() {
            match component {
                Component::Normal(value) => {
                    let name = value.to_string_lossy();
                    if matches!(
                        name.as_ref(),
                        ".git" | ".myopc_agent" | ".artifacts" | "artifacts"
                    ) {
                        return Err(format!("File path enters a protected directory: {raw}"));
                    }
                    relative.push(value);
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(format!("File path escapes the code root: {raw}"));
                }
            }
        }
        if relative.as_os_str().is_empty() {
            return Err("File path must name a regular file".into());
        }

        let full = self.root.join(&relative);
        self.validate_existing_path(&full)?;
        let display = relative.to_string_lossy().replace('\\', "/");
        Ok((full, display))
    }

    pub fn write(&self, raw: &str, content: &str) -> Result<WriteOutcome, String> {
        let (path, display) = self.resolve(raw)?;
        let parent = path.parent().ok_or("File path has no parent directory")?;
        self.create_secure_parents(parent)?;
        self.validate_existing_path(&path)?;
        atomic_write_private(&path, content.as_bytes())?;
        Ok(WriteOutcome { path, display })
    }

    fn create_secure_parents(&self, parent: &Path) -> Result<(), String> {
        let relative = parent
            .strip_prefix(&self.root)
            .map_err(|_| "File parent escapes the code root".to_string())?;
        let mut current = self.root.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err("Invalid file parent component".into());
            };
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(format!(
                        "File parent is not a real directory: {}",
                        current.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir(&current).map_err(|error| {
                        format!("Cannot create directory {}: {error}", current.display())
                    })?;
                }
                Err(error) => {
                    return Err(format!(
                        "Cannot inspect directory {}: {error}",
                        current.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_existing_path(&self, path: &Path) -> Result<(), String> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| "File path escapes the code root".to_string())?;
        let mut current = self.root.clone();
        let component_count = relative.components().count();
        for (index, component) in relative.components().enumerate() {
            let Component::Normal(name) = component else {
                return Err("Invalid file path component".into());
            };
            current.push(name);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!("File path contains a symbolic link: {path:?}"));
                }
                Ok(metadata) if index + 1 == component_count && !metadata.is_file() => {
                    return Err(format!("Write target is not a regular file: {path:?}"));
                }
                Ok(metadata) if index + 1 < component_count && !metadata.is_dir() => {
                    return Err(format!("File parent is not a directory: {current:?}"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(format!("Cannot inspect write path {current:?}: {error}"));
                }
            }
        }
        Ok(())
    }
}

pub fn atomic_write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Write target has no parent directory")?;
    let file_name = path
        .file_name()
        .ok_or("Write target has no file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(
        ".{file_name}.myopc-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));

    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("Cannot create temporary file {temporary:?}: {error}"))?;
        file.write_all(content)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Cannot write temporary file {temporary:?}: {error}"))?;
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!("Write target is not a regular file: {path:?}"));
            }
        }
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("Cannot replace file {path:?}: {error}"))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
