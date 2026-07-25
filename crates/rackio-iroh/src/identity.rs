use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};

use iroh::SecretKey;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("identity file I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("identity file must contain exactly 32 bytes, found {0}")]
    InvalidLength(usize),
}

/// Loads the device identity or creates it with owner-only permissions.
///
/// Raw key bytes are deliberately stored without a printable encoding so they
/// are less likely to be copied into logs or diagnostic output.
pub fn load_or_create_secret_key(path: &Path) -> Result<SecretKey, IdentityError> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|value: Vec<u8>| IdentityError::InvalidLength(value.len()))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_secret_key(path),
        Err(error) => Err(error.into()),
    }
}

fn create_secret_key(path: &Path) -> Result<SecretKey, IdentityError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        // Windows does not provide the Unix mode bit contract we enforce here.
        // Keep the permission change inside the platform that can guarantee it.
        #[cfg(unix)]
        set_owner_only_dir(parent)?;
    }

    let secret = SecretKey::generate();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&secret.to_bytes())?;
            file.sync_all()?;
            Ok(secret)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            load_or_create_secret_key(path)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_owner_only_dir(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::load_or_create_secret_key;

    #[test]
    fn persists_the_same_identity() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");
        let first = load_or_create_secret_key(&path).unwrap_or_else(|error| panic!("{error}"));
        let second = load_or_create_secret_key(&path).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(first.public(), second.public());
        assert_eq!(
            fs::read(path)
                .unwrap_or_else(|error| panic!("{error}"))
                .len(),
            32
        );
    }

    #[test]
    fn rejects_a_truncated_identity() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");
        fs::write(&path, [1_u8; 4]).unwrap_or_else(|error| panic!("{error}"));

        assert!(load_or_create_secret_key(&path).is_err());
    }
}
