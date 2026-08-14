use std::{fs::OpenOptions, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use fs4::{FileExt, TryLockError};

pub struct InstanceGuard {
    _lock: std::fs::File,
}

pub fn acquire() -> Result<Option<InstanceGuard>> {
    let directory = dirs::config_dir()
        .ok_or_else(|| anyhow!("config directory unavailable"))?
        .join("epd-agent");
    std::fs::create_dir_all(&directory).context("create agent config directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }

    let path = lock_path(directory);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open instance lock at {}", path.display()))?;

    match FileExt::try_lock(&lock) {
        Ok(()) => Ok(Some(InstanceGuard { _lock: lock })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock agent instance at {}", path.display()))
        }
    }
}

fn lock_path(directory: PathBuf) -> PathBuf {
    directory.join("instance.lock")
}
