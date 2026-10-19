//! Strict request DTOs, JSON-RPC errors and authentication token comparison.
use crate::error::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub(super) jsonrpc: String,
    pub(super) id: String,
    pub(super) method: String,
    #[serde(default)]
    pub(super) params: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Hello {
    pub(super) protocol: Version,
    pub(super) client: Client,
    pub(super) token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Version {
    pub(super) major: u32,
    pub(super) minor: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Client {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) kind: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeviceParams {
    pub(super) device_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OperationParams {
    pub(super) operation_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FinishResource {
    pub(super) upload_id: Uuid,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Subscribe {
    pub(super) topics: Vec<String>,
    pub(super) cursor: Option<Value>,
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| Error("INVALID_PARAMS"))
}
pub(super) fn failure(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message, "data": {"code": message}}})
}
pub(super) fn token_matches(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

pub(super) fn request_valid(request: &Request) -> bool {
    request.jsonrpc == "2.0" && request.id.len() <= 128 && !request.id.is_empty()
}
