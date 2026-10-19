use ahakey_runtime_rust::{
    ahakey::{device::AhaKey, hardware::Hardware, mock::MockX1, state::HardwareState},
    configuration::Configuration,
    error::{Error, Result},
    ipc,
    runtime::Runtime,
    storage::{Journal, Record, Storage},
    wire::Operation,
};
use async_trait::async_trait;
use futures_util::SinkExt;
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

#[path = "runtime/support/mod.rs"]
mod support;
use support::*;
#[path = "runtime/configuration.rs"]
mod configuration;
#[path = "runtime/events.rs"]
mod events;
#[path = "runtime/keymap.rs"]
mod keymap;
#[path = "runtime/oled.rs"]
mod oled;
#[path = "runtime/storage.rs"]
mod storage;
#[path = "runtime/ipc.rs"]
mod websocket;
