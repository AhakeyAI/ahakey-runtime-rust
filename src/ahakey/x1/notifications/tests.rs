use super::*;
use futures_util::stream;

fn session(frames: Vec<Vec<u8>>) -> Notifications {
    Notifications::new(
        Box::pin(
            stream::iter(frames.into_iter().map(|value| ValueNotification {
                service_uuid: uuid_from_u16(0x7340),
                uuid: uuid_from_u16(0x7344),
                value,
            }))
            .chain(stream::pending()),
        ),
        Box::pin(stream::pending()),
    )
}
fn status(mode: u8, brightness: u8) -> Vec<u8> {
    vec![
        0xaa, 0xbb, 0, 82, 0, 1, 4, mode, 0, 1, brightness, 0xcc, 0xdd,
    ]
}
fn mode(mode: u8, sequence: u8) -> Vec<u8> {
    vec![0xaa, 0xbb, 0x9b, 0, mode, 1, sequence, 0xcc, 0xdd]
}

#[tokio::test]
async fn catalog_and_maximum_key_replies_survive_mtu_23() {
    use crate::ahakey::x1::{commands::frame, oled::CATALOG};
    let catalog = frame(0x9f, &[&[0][..], CATALOG].concat());
    let mut key = vec![0, 0, 0, 0x73, 53];
    key.extend_from_slice(&[7; 53]);
    key.push(0);
    let key = frame(0x87, &key);
    assert_eq!(key.len(), 64);
    for expected in [catalog, key] {
        let fragments = expected.chunks(20).map(<[u8]>::to_vec).collect();
        let received = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            session(fragments).response(expected[2]),
        )
        .await
        .expect("MTU 23 fragments must complete the same command reply")
        .unwrap();
        assert_eq!(received, expected);
    }
}

#[tokio::test]
async fn only_exact_key_read_status_two_and_three_reach_the_decoder() {
    use crate::ahakey::x1::commands::frame;
    for code in [2, 3] {
        let error = frame(0x87, &[code]);
        assert_eq!(
            session(vec![error.clone()]).response(0x87).await.unwrap(),
            error
        );
    }
    for (opcode, payload) in [
        (0x87, vec![1]),
        (0x87, vec![2, 0]),
        (0x87, vec![3, 0]),
        (0x85, vec![3]),
    ] {
        assert_eq!(
            session(vec![frame(opcode, &payload)])
                .response(opcode)
                .await
                .unwrap_err()
                .0,
            "DEVICE_REJECTED"
        );
    }
}

#[tokio::test]
async fn lever_status_with_connection_flags_survives_unrelated_capability_reply() {
    // Extended status frames may append connection flags.
    let status = |lever| vec![0xaa, 0xbb, 0, 92, 50, 1, 4, 2, 0, lever, 35, 3, 0xcc, 0xdd];
    let up = status(0);
    let mut input = session(vec![
        up.clone(),
        up,
        vec![
            0xaa, 0xbb, 0x9f, 0, 3, 2, 1, 4, 1, 0xff, 7, 0, 0, 6, 1, 0xcc, 0xdd,
        ],
        status(1),
        status(2),
        status(0),
    ]);
    input.response(0).await.unwrap();
    for lever in [Some("automatic"), Some("manual"), None, Some("automatic")] {
        let state = tokio::time::timeout(std::time::Duration::from_millis(100), input.next_state())
            .await
            .expect("a lever change must not wait for another command")
            .unwrap();
        assert_eq!(state.lever, lever);
        assert_eq!(state.work_mode, 2);
    }
    // An idle connected device waits; silence is not DEVICE_TIMEOUT.
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), input.next_state(),)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn mode_notifications_survive_ack_wait_and_brightness_confirmation() {
    let mut input = session(vec![
        status(0, 35),
        mode(3, 255),
        vec![0xaa, 0xbb, 0x85, 0, 0xcc, 0xdd],
        status(3, 61),
        mode(1, 0),
    ]);
    input.response(0).await.unwrap();
    assert_eq!(input.next_state().await.unwrap().work_mode, 0);
    input.response(0x85).await.unwrap();
    input.response(0).await.unwrap();
    let state = input.next_state().await.unwrap();
    assert_eq!(state.work_mode, 3);
    assert_eq!(state.brightness, Some(61));
    assert_eq!(input.next_state().await.unwrap().work_mode, 1);
}

#[tokio::test]
async fn invalid_mode_frames_do_not_change_state_or_complete_other_commands() {
    let mut invalid = mode(2, 1);
    invalid[3] = 1;
    let mut input = session(vec![
        status(0, 35),
        mode(4, 1),
        invalid,
        mode(2, 2),
        vec![0xaa, 0xbb, 0x85, 0, 0xcc, 0xdd],
    ]);
    input.response(0).await.unwrap();
    input.next_state().await.unwrap();
    input.response(0x85).await.unwrap();
    assert_eq!(input.next_state().await.unwrap().work_mode, 2);
}

#[tokio::test]
async fn link_loss_wakes_idle_notification_reader() {
    let mut input = session(vec![]);
    input.disconnects = Box::pin(stream::iter([()]));
    assert_eq!(
        input.next_state().await.unwrap_err().0,
        "DEVICE_DISCONNECTED"
    );
}

#[tokio::test]
async fn binary_readback_delimiters_survive_every_split_position() {
    use crate::ahakey::x1::commands::frame;
    let mut binding = vec![0, 0, 0, 0x73, 53];
    binding.extend_from_slice(&[7; 53]);
    // End the first MTU-23 notification with CC DD, and begin the next with AA BB.
    binding[15..19].copy_from_slice(&[0xcc, 0xdd, 0xaa, 0xbb]);
    binding.push(0);
    let key = frame(0x87, &binding);
    let resource = frame(
        0x88,
        &[0, 3, 0, 1, 0, 0, 0x70, 0, 0, 0xaa, 0xbb, 0xcc, 0xdd],
    );
    // Two side-switch positions; the custom binding itself contains CC DD.
    let switch = frame(
        0x97,
        &[0, 0, 2, 0, 0, 3, 6, 0x73, 4, 0xaa, 0xbb, 0xcc, 0xdd],
    );
    for expected in [key, resource, switch] {
        for split in 1..expected.len() {
            let fragments = vec![expected[..split].to_vec(), expected[split..].to_vec()];
            let received = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                session(fragments).response(expected[2]),
            )
            .await
            .expect("payload delimiters must not terminate a partial reply")
            .unwrap();
            assert_eq!(received, expected, "split at {split}");
        }
    }
}

#[tokio::test]
async fn fragmented_and_concatenated_frames_keep_ack_and_event_order() {
    use crate::ahakey::x1::{commands::frame, oled::CATALOG};
    let catalog = frame(0x9f, &[&[0][..], CATALOG].concat());
    let frames = [
        status(0, 35),
        catalog.clone(),
        mode(2, 255),
        frame(0x85, &[0]),
        mode(1, 0),
    ]
    .concat();
    let mut input = session(frames.chunks(20).map(<[u8]>::to_vec).collect());
    input.response(0).await.unwrap();
    assert_eq!(input.next_state().await.unwrap().work_mode, 0);
    assert_eq!(input.response(0x9f).await.unwrap(), catalog);
    input.response(0x85).await.unwrap();
    assert_eq!(input.next_state().await.unwrap().work_mode, 2);
    assert_eq!(input.next_state().await.unwrap().work_mode, 1);
}

#[tokio::test]
async fn cancellation_keeps_partial_reply_and_other_characteristics_do_not_pollute_it() {
    use crate::ahakey::x1::{commands::frame, oled::CATALOG};
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let (disconnect, mut losses) = tokio::sync::mpsc::unbounded_channel();
    let mut input = Notifications::new(
        Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx))),
        Box::pin(stream::poll_fn(move |cx| losses.poll_recv(cx))),
    );
    let notify = |value: Vec<u8>| ValueNotification {
        service_uuid: uuid_from_u16(0x7340),
        uuid: uuid_from_u16(0x7344),
        value,
    };
    let catalog = frame(0x9f, &[&[0][..], CATALOG].concat());
    sender.send(notify(catalog[..20].to_vec())).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), input.next_state())
            .await
            .is_err()
    );
    let mut unrelated = notify(vec![0xcc, 0xdd]);
    unrelated.uuid = uuid_from_u16(0x7343);
    sender.send(unrelated.clone()).unwrap();
    unrelated.uuid = uuid_from_u16(0x7344);
    unrelated.service_uuid = uuid_from_u16(0x180f);
    sender.send(unrelated).unwrap();
    sender.send(notify(catalog[20..].to_vec())).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_millis(100), input.response(0x9f))
            .await
            .unwrap()
            .unwrap(),
        catalog
    );
    // A partial frame must not prevent a disconnect from waking the consumer.
    sender.send(notify(vec![0xaa, 0xbb, 0x87, 0])).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), input.next_state())
            .await
            .is_err()
    );
    disconnect.send(()).unwrap();
    assert_eq!(
        input.response(0x87).await.unwrap_err().0,
        "DEVICE_DISCONNECTED"
    );
    // Even if the caller inspects this object before dropping the session, no stale prefix remains.
    input.disconnects = Box::pin(stream::pending());
    let ack = frame(0x85, &[0]);
    sender.send(notify(ack.clone())).unwrap();
    assert_eq!(input.response(0x85).await.unwrap(), ack);
}

#[tokio::test]
async fn overlong_partial_replies_fail_instead_of_growing_without_bound() {
    let mut overlong = vec![0xaa, 0xbb, 0x9f, 0];
    overlong.resize(256, b'x');
    let mut input = session(overlong.chunks(20).map(<[u8]>::to_vec).collect());
    let error = tokio::time::timeout(std::time::Duration::from_millis(100), input.response(0x9f))
        .await
        .expect("overlong replies must fail without waiting for the command timeout")
        .unwrap_err();
    assert_eq!(error.0, "DEVICE_INVALID_RESPONSE");
}
