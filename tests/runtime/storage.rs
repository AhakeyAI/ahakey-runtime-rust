use super::*;

#[tokio::test]
async fn restart_marks_unfinished_operations_unknown_and_corrupt_storage_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let (storage, _) = Storage::open(path.clone()).await.unwrap();
    let mut journal = Journal::default();
    journal.operations.insert(
        "pending".into(),
        Record {
            request: apply("base", "pending").into(),
            oled_transaction: false,
            operation: Operation {
                operation_id: "pending".into(),
                device_id: "test-x1".into(),
                status: "running".into(),
                progress: 0.4,
                effect: "partial".into(),
                error_code: None,
            },
        },
    );
    storage.save(&journal).await.unwrap();
    let runtime = Runtime::open(path.clone()).await.unwrap();
    let recovered = runtime.operation("pending").await.unwrap();
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.effect, "unknown");
    assert_eq!(recovered.error_code.as_deref(), Some("RUNTIME_RESTARTED"));
    runtime.shutdown().await;
    std::fs::write(path, b"broken").unwrap();
    assert!(
        Runtime::open(directory.path().join("journal.json"))
            .await
            .is_err()
    );
}

#[test]
fn journal_still_reads_legacy_configuration_records() {
    let value = json!({"version":1,"operations":{"old":{"request":apply("base","old"),
        "operation":{"operationId":"old","deviceId":"test-x1","status":"completed","progress":1.0,"effect":"complete","errorCode":null}}},"configurations":{}});
    let journal: Journal = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(journal).unwrap(), value);
}
