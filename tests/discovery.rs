use ahakey_runtime_rust::discovery::Discovery;
use serde_json::Value;

#[test]
fn private_atomic_discovery_holds_exclusive_lock_until_shutdown_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime/discovery.json");
    let mut owner = Discovery::acquire(path.clone()).unwrap();
    assert!(Discovery::acquire(path.clone()).is_err());
    owner
        .publish("ws://127.0.0.1:43210/", "test-token")
        .unwrap();
    let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["endpoint"], "ws://127.0.0.1:43210/");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    owner.unpublish();
    assert!(!path.exists());
    assert!(Discovery::acquire(path.clone()).is_err());
    drop(owner);
    assert!(Discovery::acquire(path).is_ok());
}

#[cfg(unix)]
#[test]
fn shared_directory_and_symlink_lock_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("discovery.json");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Discovery::acquire(path.clone()).is_err());
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    symlink(directory.path().join("other"), path.with_extension("lock")).unwrap();
    assert!(Discovery::acquire(path).is_err());
}
