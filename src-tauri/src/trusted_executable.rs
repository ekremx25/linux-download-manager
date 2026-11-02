//! Conservative executable/registration checks shared by startup and the native host.
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path};

pub(crate) fn current_uid() -> Result<u32, String> {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(|error| format!("Cannot identify application user: {error}"))
}

pub(crate) fn trusted_file(path: &Path, uid: u32, executable: bool) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file()
        || metadata.len() == 0
        || (metadata.uid() != 0 && metadata.uid() != uid)
        || metadata.permissions().mode() & 0o022 != 0
        || (executable && metadata.permissions().mode() & 0o111 == 0)
    {
        return false;
    }
    path.parent()
        .is_some_and(|parent| trusted_directory(parent, uid))
}

pub(crate) fn trusted_directory(path: &Path, uid: u32) -> bool {
    path.is_absolute()
        && path.ancestors().all(|parent| {
            let Ok(metadata) = fs::symlink_metadata(parent) else {
                return false;
            };
            let mode = metadata.permissions().mode();
            // Root-owned sticky directories (e.g. /tmp) protect user-owned children.
            let protected_sticky = metadata.uid() == 0 && mode & 0o1000 != 0;
            metadata.is_dir()
                && (metadata.uid() == 0 || metadata.uid() == uid)
                && (mode & 0o022 == 0 || protected_sticky)
        })
}

#[cfg(test)]
#[path = "startup_test_support.rs"]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use test_support::Fixture;

    #[test]
    fn requires_regular_executable_without_writable_or_symlink_boundaries() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        let app = fixture.script("app", "exit 0");
        assert!(trusted_file(&app, uid, true));
        fs::set_permissions(&app, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!trusted_file(&app, uid, true));
        fs::set_permissions(&app, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!trusted_file(&app, uid, true));
        fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
        let link = fixture.path.join("app-link");
        symlink(&app, &link).unwrap();
        assert!(!trusted_file(&link, uid, true));
        let directory_link = fixture.path.join("directory-link");
        symlink(&fixture.path, &directory_link).unwrap();
        assert!(!trusted_file(&directory_link.join("app"), uid, true));
        assert!(!trusted_file(&fixture.path, uid, true));
        fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!trusted_file(&app, uid, true));
        fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn rejects_empty_and_relative_files_but_allows_read_only_registration() {
        let fixture = Fixture::new();
        let uid = current_uid().unwrap();
        let empty = fixture.path.join("empty");
        fs::write(&empty, []).unwrap();
        fs::set_permissions(&empty, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!trusted_file(&empty, uid, true));
        assert!(!trusted_file(Path::new("relative-app"), uid, true));
        let registration = fixture.path.join("registration");
        fs::write(&registration, "registered path").unwrap();
        fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(trusted_file(&registration, uid, false));
        assert!(!trusted_file(&registration, uid, true));
    }
}
