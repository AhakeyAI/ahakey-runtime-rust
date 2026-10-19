use ahakey_runtime_rust::{
    ahakey::state::X1State,
    configuration::{Action, Apply, Configuration, MacroStep},
    protocol::x1::configuration_commands,
    wire::ConfigurationDocument,
};
use serde_json::json;

#[test]
fn studio_fixtures_preserve_unmodified_modes() {
    let mut document: ConfigurationDocument = serde_json::from_str(include_str!(
        "../contracts/runtime-v1/fixtures/configuration.json"
    ))
    .unwrap();
    let apply: Apply =
        serde_json::from_str(include_str!("../contracts/runtime-v1/fixtures/apply.json")).unwrap();
    apply.validate().unwrap();
    let config = document.configuration.as_mut().unwrap();
    config.validate().unwrap();
    config.merge(&apply.changes);
    // Canonicalization only changes array order; compare canonicalized untouched modes.
    let mut old = Configuration::mock_defaults();
    old.canonicalize();
    assert_eq!(config.modes[1..], old.modes[1..]);
    assert_eq!(config.device.brightness_percent, 60);
}
#[test]
fn firmware_encoding_clears_layers_and_uses_actual_effect_indices() {
    let mut config = Configuration::mock_defaults();
    config.modes.truncate(1);
    config.modes[0].keys[0].action = Action::Shortcut {
        modifiers: vec!["gui".into(), "control".into()],
        key_code: 0,
    };
    config.modes[0].keys[1].action = Action::Macro {
        steps: vec![
            MacroStep::KeyDown { key_code: 40 },
            MacroStep::Delay { delay_ms: 765 },
            MacroStep::KeyUp { key_code: 40 },
            MacroStep::ReleaseAll {},
            MacroStep::NoOp {},
        ],
    };
    config.modes[0].keys[2].action = Action::Disabled {};
    let commands = configuration_commands(&config).unwrap();
    assert_eq!(commands[0], [0xaa, 0xbb, 0x85, 35, 0xcc, 0xdd]);
    assert_eq!(commands[1], [0xaa, 0xbb, 0x73, 0x74, 0, 0, 0xcc, 0xdd]);
    assert_eq!(
        commands[2],
        [0xaa, 0xbb, 0x73, 0x73, 0, 0, 0xe0, 0xe3, 0xcc, 0xdd]
    );
    assert_eq!(commands[4], [0xaa, 0xbb, 0x73, 0x73, 0, 1, 0xcc, 0xdd]);
    assert_eq!(
        commands[5],
        [
            0xaa, 0xbb, 0x73, 0x74, 0, 1, 1, 40, 3, 255, 2, 40, 4, 0, 0, 0, 0xcc, 0xdd
        ]
    );
    assert_eq!(commands[7], [0xaa, 0xbb, 0x73, 0x74, 0, 2, 0xcc, 0xdd]);
    assert_eq!(commands[8], [0xaa, 0xbb, 0x73, 0x73, 0, 2, 0xcc, 0xdd]);
    assert_eq!(
        commands[13],
        [
            0xaa, 0xbb, 0x84, 0, 10, 16, 12, 1, 3, 6, 12, 5, 0, 0xcc, 0xdd
        ]
    );
    assert_eq!(commands.last().unwrap(), &[0xaa, 0xbb, 4, 0xcc, 0xdd]);
}
#[test]
fn invalid_write_groups_never_truncate_or_sanitize() {
    let original = Configuration::mock_defaults();
    let mut config = original.clone();
    config.modes[0].keys[0].description = "中文".into();
    assert!(config.validate().is_err());
    let mut config = original.clone();
    config.modes[0].keys.pop();
    assert_eq!(config.validate().unwrap_err().0, "INCOMPLETE_WRITE_GROUP");
    let mut config = original.clone();
    config.modes[0].keys[1].role = "voice".into();
    assert!(config.validate().is_err());
    let mut config = original.clone();
    config.modes[0].lights[1].state = "notification".into();
    assert!(config.validate().is_err());
    let mut config = original.clone();
    config.device.brightness_percent = 0;
    assert!(config.validate().is_err());
    for delay_ms in [1, 764, 768] {
        let mut config = original.clone();
        config.modes[0].keys[0].action = Action::Macro {
            steps: vec![MacroStep::Delay { delay_ms }],
        };
        assert!(config.validate().is_err());
    }
    let mut config = original.clone();
    config.modes[0].keys[0].action = Action::Macro {
        steps: vec![MacroStep::NoOp {}; 50],
    };
    assert!(config.validate().is_err());
    for action in [
        json!({"type":"disabled","keyCode":4}),
        json!({"type":"shortcut","modifiers":[],"keyCode":256}),
        json!({"type":"macro","steps":[{"type":"noOp","delayMs":3}]}),
    ] {
        assert!(serde_json::from_value::<Action>(action).is_err());
    }
    let mut request: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/runtime-v1/fixtures/apply.json")).unwrap();
    request["changes"]["localAssetPath"] = json!("/tmp/file");
    assert!(serde_json::from_value::<Apply>(request).is_err());
}
#[test]
fn status_frames_handle_old_firmware_unknowns_and_malformed_bytes() {
    let bytes = [0xaa, 0xbb, 0, 82, 0xe0, 1, 4, 3, 6, 0, 60, 0xcc, 0xdd];
    let state = X1State::from_bytes(&bytes).unwrap().0;
    assert_eq!(state.battery, Some(82));
    assert_eq!(state.lever, Some("automatic"));
    assert_eq!(state.brightness, Some(60));
    assert_eq!(state.work_mode, 3);
    let old = [0xaa, 0xbb, 0, 255, 0, 1, 0, 0, 6, 2, 0xcc, 0xdd];
    let state = X1State::from_bytes(&old).unwrap().0;
    assert_eq!(state.battery, None);
    assert_eq!(state.lever, None);
    assert_eq!(state.brightness, None);
    for length in 0..12 {
        assert!(X1State::from_bytes(&bytes[..length]).is_none());
    }
    let mut wrong = bytes;
    wrong[2] = 0x73;
    assert!(X1State::from_bytes(&wrong).is_none());
    let mut wrong = bytes;
    wrong[7] = 4;
    assert!(X1State::from_bytes(&wrong).is_none());
}
