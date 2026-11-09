use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub struct InstanceLock {
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(team_root: &Path) -> Result<Self, String> {
        let path = team_root.join("myopc.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("Cannot open instance lock {}: {error}", path.display()))?;
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if locked != 0 {
            return Err(format!(
                "Another MYOPC instance is already running for {}",
                team_root.display()
            ));
        }
        file.set_len(0)
            .and_then(|_| write!(file, "{}", std::process::id()))
            .map_err(|error| format!("Cannot write instance lock {}: {error}", path.display()))?;
        Ok(Self { file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_instance_cannot_acquire_the_same_project() {
        let dir =
            std::env::temp_dir().join(format!("myopc-lock-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = InstanceLock::acquire(&dir).unwrap();
        assert!(InstanceLock::acquire(&dir).is_err());
        drop(first);
        assert!(InstanceLock::acquire(&dir).is_ok());
        std::fs::remove_dir_all(dir).ok();
    }
}
