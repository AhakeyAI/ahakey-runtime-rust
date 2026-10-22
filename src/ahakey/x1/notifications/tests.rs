use super::*;
use crate::ahakey::x1::{commands::frame, oled::CATALOG};
use futures_util::stream;
use std::time::Duration;

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
async fn replies_reassemble_at_mtu_23_and_byte_boundaries_with_binary_delimiters() {
    let catalog = frame(0x9f, &[&[0][..], CATALOG].concat());
    let mut binding = vec![0, 0, 0, 0x73, 49];
    binding.extend_from_slice(&[7; 49]);
    // First MTU-23 chunk ends in CC DD; the next starts in AA BB.
    binding[15..19].copy_from_slice(&[0xcc, 0xdd, 0xaa, 0xbb]);
    binding.extend_from_slice(&[4, 0xaa, 0xbb, 0xcc, 0xdd]);
    let key = frame(0x87, &binding);
    assert_eq!(key.len(), 64);
    let resource = frame(
        0x88,
        &[0, 3, 0, 1, 0, 0, 0x70, 0, 0, 0xaa, 0xbb, 0xcc, 0xdd],
    );
    let switch = frame(
        0x97,
        &[0, 0, 2, 0, 0, 3, 6, 0x73, 4, 0xaa, 0xbb, 0xcc, 0xdd],
    );
    for expected in [catalog, key, resource, switch] {
        for chunk in [1, 20, expected.len()] {
            let fragments = expected.chunks(chunk).map(<[u8]>::to_vec).collect();
            let actual = tokio::time::timeout(
                Duration::from_millis(100),
                session(fragments).response(expected[2]),
            )
            .await
            .expect("fragmented replies must complete without splitting binary payloads")
            .unwrap();
            assert_eq!(actual, expected, "chunk size {chunk}");
        }
    }
}

#[tokio::test]
async fn only_exact_key_read_status_two_and_three_reach_the_decoder() {
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
async fn fragmented_replies_preserve_ack_state_and_mode_sequence_order() {
    let catalog = frame(0x9f, &[&[0][..], CATALOG].concat());
    let mut invalid = mode(2, 1);
    invalid[3] = 1;
    let mut unknown_lever = status(1, 61);
    unknown_lever[9] = 2;
    let mut automatic = status(1, 61);
    automatic[9] = 0;
    let frames = [
        status(0, 35),
        catalog.clone(),
        mode(4, 1),
        invalid,
        frame(0x85, &[0]),
        mode(3, 255),
        frame(0x85, &[0]),
        status(3, 61),
        mode(1, 0),
        unknown_lever,
        automatic,
    ]
    .concat();
    let mut input = session(frames.chunks(20).map(<[u8]>::to_vec).collect());
    input.response(0).await.unwrap();
    assert_eq!(input.next_state().await.unwrap().work_mode, 0);
    assert_eq!(input.response(0x9f).await.unwrap(), catalog);
    input.response(0x85).await.unwrap();
    // Invalid notifications must not publish a state before the next valid mode.
    assert_eq!(input.next_state().await.unwrap().work_mode, 3);
    input.response(0x85).await.unwrap();
    input.response(0).await.unwrap();
    let state = input.next_state().await.unwrap();
    assert_eq!((state.work_mode, state.brightness), (3, Some(61)));
    assert_eq!(input.next_state().await.unwrap().work_mode, 1);
    assert_eq!(input.next_state().await.unwrap().lever, None);
    assert_eq!(input.next_state().await.unwrap().lever, Some("automatic"));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), input.next_state())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancellation_keeps_partial_reply_and_other_characteristics_do_not_pollute_it() {
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
        tokio::time::timeout(Duration::from_millis(20), input.next_state())
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
        tokio::time::timeout(Duration::from_millis(100), input.response(0x9f))
            .await
            .unwrap()
            .unwrap(),
        catalog
    );
    // A partial frame must not prevent a disconnect from waking the consumer.
    sender.send(notify(vec![0xaa, 0xbb, 0x87, 0])).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), input.next_state())
            .await
            .is_err()
    );
    disconnect.send(()).unwrap();
    assert_eq!(
        input.response(0x87).await.unwrap_err().0,
        "DEVICE_DISCONNECTED"
    );
}

#[tokio::test]
async fn overlong_partial_replies_fail_instead_of_growing_without_bound() {
    let mut overlong = vec![0xaa, 0xbb, 0x9f, 0];
    overlong.resize(256, b'x');
    let mut input = session(overlong.chunks(20).map(<[u8]>::to_vec).collect());
    let error = tokio::time::timeout(Duration::from_millis(100), input.response(0x9f))
        .await
        .expect("overlong replies must fail without waiting for the command timeout")
        .unwrap_err();
    assert_eq!(error.0, "DEVICE_INVALID_RESPONSE");
}
