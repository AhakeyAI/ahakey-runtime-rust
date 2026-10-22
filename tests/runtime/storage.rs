use super::*;

#[tokio::test]
async fn restart_marks_unfinished_operations_unknown_and_corrupt_storage_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.json");
    let record = |id: &str, status: &str| {
        json!({
            "request": apply("base", id),
            "operation": {"operationId":id, "deviceId":"test-x1", "status":status,
                "progress":if status == "completed" {1.0} else {0.4},
            "effect":if status == "completed" {"complete"} else {"partial"}, "errorCode":null}
        })
    };
    // Older journals omit oled_transaction; recover them through the real storage path.
    let journal = json!({"version":1,"operations":{
        "pending":record("pending", "running"), "completed":record("completed", "completed")
    },"configurations":{}});
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    let runtime = Runtime::open(path.clone()).await.unwrap();
    let recovered = runtime.operation("pending").await.unwrap();
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.effect, "unknown");
    assert_eq!(recovered.error_code.as_deref(), Some("RUNTIME_RESTARTED"));
    assert_eq!(
        runtime.operation("completed").await.unwrap().status,
        "completed"
    );
    runtime.shutdown().await;
    std::fs::write(path, b"broken").unwrap();
    assert!(
        Runtime::open(directory.path().join("journal.json"))
            .await
            .is_err()
    );
}
