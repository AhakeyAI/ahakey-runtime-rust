use ahakey_runtime_rust::{
    input::{InputDevice, InputMessage, InputState, KeyEvent},
    runtime::Runtime,
    voice::Preferences,
};
use serde_json::json;
use tokio::sync::mpsc;

fn input(status: &str) -> InputMessage {
    InputMessage::State(InputState {
        status: status.into(),
        permission: "granted".into(),
        devices: if status == "listening" {
            vec![InputDevice {
                input_device_id: "hid-1".into(),
                name: "AhaKey".into(),
                stable_id: None,
                pressed_usages: vec![],
            }]
        } else {
            vec![]
        },
    })
}
fn key(pressed: bool) -> InputMessage {
    InputMessage::Key(KeyEvent {
        input_device_id: "hid-1".into(),
        name: "AhaKey".into(),
        usage: 109,
        pressed,
        timestamp_ms: "1".into(),
        held_ms: None,
    })
}

#[tokio::test]
async fn hardware_events_drive_voice_without_a_studio_subscriber_and_disconnect_cancels() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(dir.path().join("state.json")).await.unwrap();
    let (sender, mut receiver) = mpsc::channel(32);
    runtime.attach_voice(sender).await;
    runtime.voice_event(json!({"type":"permissions", "microphone":"granted", "speech":"granted", "accessibility":true})).await;
    runtime.input_message(input("listening")).await;
    runtime
        .configure_voice(Preferences {
            enabled: true,
            input_device_id: Some("hid-1".into()),
            auto_insert: true,
            ..Default::default()
        })
        .await
        .unwrap();
    runtime.input_message(key(true)).await;
    let start = receiver.recv().await.unwrap();
    assert_eq!(start["command"], "start");
    runtime.input_message(key(true)).await;
    assert!(
        receiver.try_recv().is_err(),
        "duplicate HID down must not retrigger"
    );
    runtime.input_message(key(false)).await;
    assert_eq!(receiver.recv().await.unwrap()["command"], "stop");
    runtime.input_message(input("deviceUnavailable")).await;
    assert_eq!(receiver.recv().await.unwrap()["command"], "cancel");
    runtime
        .voice_event(json!({"type":"final", "sessionId":start["sessionId"], "text":"late"}))
        .await;
    assert!(runtime.voice().await.text.is_empty());
    assert!(receiver.try_recv().is_err());
    assert!(!runtime.voice().await.preferences.enabled);
    let snapshot = runtime.subscribe().await.0;
    assert_eq!(snapshot["voice"]["error"], "VOICE_INPUT_LOST");
    runtime.shutdown().await;
}

#[tokio::test]
async fn preview_does_not_insert_and_preferences_survive_without_recording_or_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let runtime = Runtime::open(path.clone()).await.unwrap();
    let (sender, mut receiver) = mpsc::channel(32);
    runtime.attach_voice(sender).await;
    runtime
        .voice_event(json!({"type":"permissions", "microphone":"granted", "speech":"granted"}))
        .await;
    runtime
        .configure_voice(Preferences {
            auto_insert: true,
            locale: "en-US".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let view = runtime.control_voice("start", None).await.unwrap();
    receiver.recv().await.unwrap();
    assert_eq!(
        runtime
            .control_voice("cancel", Some("old-session"))
            .await
            .unwrap_err()
            .0,
        "VOICE_SESSION_CHANGED"
    );
    runtime
        .voice_event(json!({"type":"final", "sessionId":view.session_id, "text":"private words"}))
        .await;
    assert_eq!(runtime.voice().await.status, "completed");
    assert!(receiver.try_recv().is_err());
    runtime.shutdown().await;
    drop(runtime);
    let restarted = Runtime::open(path).await.unwrap();
    assert_eq!(restarted.voice().await.preferences.locale, "en-US");
    assert!(!restarted.voice().await.preferences.enabled);
    assert!(restarted.voice().await.text.is_empty());
    for entry in std::fs::read_dir(dir.path()).unwrap().flatten() {
        if entry.path().is_file() {
            assert!(
                !String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap())
                    .contains("private words")
            );
        }
    }
    restarted.shutdown().await;
}

fn stable_input(ids: &[(&str, &str)], held: bool) -> InputMessage {
    InputMessage::State(InputState {
        status: "listening".into(),
        permission: "granted".into(),
        devices: ids
            .iter()
            .map(|(connection, stable)| InputDevice {
                input_device_id: (*connection).into(),
                name: "Same display name".into(),
                stable_id: Some((*stable).into()),
                pressed_usages: if held { vec![109] } else { vec![] },
            })
            .collect(),
    })
}
#[tokio::test]
async fn restart_restores_only_unique_bound_device_after_permissions_and_key_release() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let runtime = Runtime::open(path.clone()).await.unwrap();
    runtime
        .input_message(stable_input(&[("hid-old", "physical-a")], false))
        .await;
    runtime
        .configure_voice(Preferences {
            enabled: true,
            input_device_id: Some("hid-old".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    runtime.shutdown().await;
    drop(runtime);
    let runtime = Runtime::open(path.clone()).await.unwrap();
    let (sender, mut receiver) = mpsc::channel(32);
    runtime.attach_voice(sender).await;
    runtime
        .input_message(stable_input(&[("hid-new", "physical-a")], false))
        .await;
    assert!(
        !runtime.voice().await.preferences.enabled,
        "permissions have not arrived"
    );
    runtime
        .input_message(stable_input(&[("hid-new", "physical-b")], false))
        .await;
    runtime
        .voice_event(json!({"type":"permissions", "microphone":"granted", "speech":"granted"}))
        .await;
    assert!(
        !runtime.voice().await.preferences.enabled,
        "same name is not the same keyboard"
    );
    runtime
        .input_message(stable_input(
            &[("hid-new", "physical-a"), ("hid-other", "physical-a")],
            false,
        ))
        .await;
    assert!(
        !runtime.voice().await.preferences.enabled,
        "ambiguous identity"
    );
    runtime
        .input_message(stable_input(&[("hid-new", "physical-a")], true))
        .await;
    assert!(
        !runtime.voice().await.preferences.enabled,
        "do not arm a held voice key"
    );
    runtime
        .input_message(stable_input(&[("hid-new", "physical-a")], false))
        .await;
    assert!(runtime.voice().await.preferences.enabled);
    assert_eq!(
        runtime.voice().await.preferences.input_device_id.as_deref(),
        Some("hid-new")
    );
    assert!(
        receiver.try_recv().is_err(),
        "restoring preferences never starts recording"
    );
    assert!(runtime.voice().await.text.is_empty());
    runtime
        .input_message(InputMessage::State(InputState::default()))
        .await;
    assert!(!runtime.voice().await.preferences.enabled);
    runtime
        .input_message(stable_input(&[("hid-reconnected", "physical-a")], false))
        .await;
    assert!(runtime.voice().await.preferences.enabled);
    assert_eq!(
        runtime.voice().await.preferences.input_device_id.as_deref(),
        Some("hid-reconnected")
    );
    runtime
        .configure_voice(Preferences::default())
        .await
        .unwrap();
    runtime.shutdown().await;
    drop(runtime);
    let runtime = Runtime::open(path).await.unwrap();
    runtime
        .input_message(stable_input(&[("hid-next", "physical-a")], false))
        .await;
    runtime
        .voice_event(json!({"type":"permissions", "microphone":"granted", "speech":"granted"}))
        .await;
    assert!(
        !runtime.voice().await.preferences.enabled,
        "explicit disable must persist"
    );
    runtime.shutdown().await;
}
