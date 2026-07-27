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
        // Only narrow permissions on a directory this call creates. Chmod-ing
        // a directory we did not create can strip access from other
        // processes that directory is provisioned to serve — for example
        // macOS installs the daemon's data directory as an installer-owned
        // 0750 `_rackio:_rackio-viewers` directory so the viewer group can
        // still traverse it to reach a sibling runtime socket. Narrowing that
        // directory to 0700 on first key creation broke that access
        // permanently. The 0600 mode on the key file below is the guarantee
        // this function owns; an installer or service manager owns the
        // directory permissions of a directory it provisioned.
        // Windows does not provide the Unix mode bit contract we enforce here.
        // Keep both the check and the permission change inside the platform
        // that can guarantee it, so neither is dead code elsewhere.
        #[cfg(unix)]
        let parent_already_existed = parent.exists();
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        if !parent_already_existed {
            set_owner_only_dir(parent)?;
        }
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

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_owner_only_even_in_a_shared_directory() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        // Simulate an installer-provisioned data directory shared with a
        // viewer group (e.g. macOS's `_rackio:_rackio-viewers` 0750
        // directory), which must not be narrowed by key creation.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750))
            .unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");

        load_or_create_secret_key(&path).unwrap_or_else(|error| panic!("{error}"));

        let directory_mode = fs::metadata(directory.path())
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            directory_mode, 0o750,
            "key creation must not narrow a directory it did not create"
        );

        let file_mode = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "the key file itself must stay owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_created_for_the_key_is_narrowed_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let created_parent = root.path().join("nested").join("state");
        let path = created_parent.join("identity.key");

        load_or_create_secret_key(&path).unwrap_or_else(|error| panic!("{error}"));

        let directory_mode = fs::metadata(&created_parent)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            directory_mode, 0o700,
            "a directory created for the key must be owner-only"
        );
    }
}
