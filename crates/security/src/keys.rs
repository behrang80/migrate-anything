//! Workspace key files (32 random bytes, hex). Created on first use with owner-only
//! permissions where the platform supports it.

use std::path::Path;

use ma_types::{MaError, MaResult};

pub struct Key(pub [u8; 32]);

impl Drop for Key {
    fn drop(&mut self) {
        // best-effort wipe
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

pub fn load_or_create(path: &Path) -> MaResult<Key> {
    if path.exists() {
        let text = std::fs::read_to_string(path)?;
        let bytes = hex::decode(text.trim()).map_err(|_| MaError::config(format!("corrupt key file {}", path.display())))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| MaError::config("key file must contain 32 bytes"))?;
        return Ok(Key(arr));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| MaError::internal(format!("rng: {e}")))?;
    std::fs::write(path, hex::encode(k))?;
    restrict_permissions(path);
    Ok(Key(k))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {
    // On Windows the file inherits the ACL of the per-user workspace directory, which is
    // created under the user's profile. See SECURITY.md.
}
