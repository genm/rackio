use std::{
    fs,
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
    match read_secret_key(path)? {
        Some(secret) => Ok(secret),
        None => create_secret_key(path),
    }
}

/// Read the identity, distinguishing "absent" from "unreadable".
///
/// `Ok(None)` is the only state that justifies minting a new key. Reporting an
/// unreadable file as absent would mint a *second* endpoint identity and
/// silently invalidate every pairing the operator already completed, so any
/// other failure is surfaced instead.
fn read_secret_key(path: &Path) -> Result<Option<SecretKey>, IdentityError> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|value: Vec<u8>| IdentityError::InvalidLength(value.len()))?;
            Ok(Some(SecretKey::from_bytes(&bytes)))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
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
    // Write the key beside its destination and publish it in one step. Creating
    // the destination empty and filling it afterwards let a daemon starting at
    // the same moment read the file between those two operations and reject a
    // zero-byte identity, which is a startup failure rather than a race the
    // loser can recover from.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::Builder::new()
        .prefix(".identity-")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&secret.to_bytes())?;
    file.as_file().sync_all()?;
    // `persist_noclobber` refuses to replace an existing key, so a loser of the
    // race never overwrites the identity the winner just published.
    match file.persist_noclobber(path) {
        Ok(_) => Ok(secret),
        Err(error) => {
            // Losing the race is decided by what is on disk, not by the errno
            // the publish happened to fail with: if a readable key is there
            // now, another process published it and this one adopts it. Read
            // it once rather than restarting the whole load, which made the
            // previous recursive retry spin until the stack ran out on a path
            // that always exists yet never reads back.
            match read_secret_key(path)? {
                Some(existing) => Ok(existing),
                None => Err(error.error.into()),
            }
        }
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

    use super::{IdentityError, load_or_create_secret_key};

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
    fn an_unreadable_identity_is_not_treated_as_a_missing_one() {
        // Minting a key here would give the machine a second endpoint identity
        // and silently invalidate every pairing already completed against the
        // first one, so an unreadable file must fail closed instead.
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");
        let existing = load_or_create_secret_key(&path).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|error| panic!("{error}"));
        if fs::read(&path).is_ok() {
            // A privileged test runner ignores the mode bits, so there is no
            // unreadable state to assert against on this host.
            return;
        }

        let error = load_or_create_secret_key(&path)
            .err()
            .unwrap_or_else(|| panic!("an unreadable identity must not load"));

        assert!(
            matches!(&error, IdentityError::Io(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied),
            "the read failure must be reported as-is, got {error:?}"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            load_or_create_secret_key(&path)
                .unwrap_or_else(|error| panic!("{error}"))
                .public(),
            existing.public(),
            "the original identity must survive the failed load"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_key_path_that_exists_but_never_reads_back_fails_instead_of_recursing() {
        // A dangling symlink reads as absent and then refuses an exclusive
        // create, which is exactly the shape that made the previous recursive
        // retry spin until the stack ran out.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");
        std::os::unix::fs::symlink(directory.path().join("absent"), &path)
            .unwrap_or_else(|error| panic!("{error}"));

        let error = load_or_create_secret_key(&path)
            .err()
            .unwrap_or_else(|| panic!("a key that cannot be read back must not load"));

        assert!(
            matches!(&error, IdentityError::Io(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists),
            "the failed publish must be reported as-is, got {error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_key_that_cannot_be_created_reports_the_creation_failure() {
        // Only `AlreadyExists` means another process won the race. Any other
        // create failure is this machine's problem and must be reported rather
        // than re-read as if a key had appeared.
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let read_only = directory.path().join("state");
        fs::create_dir(&read_only).unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o500))
            .unwrap_or_else(|error| panic!("{error}"));
        if fs::write(read_only.join("probe"), b"probe").is_ok() {
            // A privileged test runner can write into a read-only directory.
            return;
        }

        let error = load_or_create_secret_key(&read_only.join("identity.key"))
            .err()
            .unwrap_or_else(|| panic!("an uncreatable identity must not load"));

        assert!(
            matches!(&error, IdentityError::Io(error)
                if error.kind() == std::io::ErrorKind::PermissionDenied),
            "the create failure must be reported as-is, got {error:?}"
        );
    }

    #[test]
    fn concurrent_starts_converge_on_one_identity() {
        // Two daemons starting together both find no key and both try to
        // create it. The loser must adopt the winner's key rather than fail or
        // mint a second identity.
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = directory.path().join("identity.key");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let keys: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let barrier = std::sync::Arc::clone(&barrier);
                    let path = path.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        load_or_create_secret_key(&path).map(|secret| secret.public())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_else(|_| panic!("thread panicked")))
                .collect()
        });

        let public_keys: Vec<_> = keys
            .into_iter()
            .map(|key| key.unwrap_or_else(|error| panic!("{error}")))
            .collect();
        assert!(
            public_keys.windows(2).all(|pair| pair[0] == pair[1]),
            "every starter must end up with the same identity: {public_keys:?}"
        );
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
