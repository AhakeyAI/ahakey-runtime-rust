//! Business-method dispatch; no socket IO or per-client transfer ownership.
use super::protocol::{DeviceParams, OperationParams, decode};
use crate::{
    error::{Error, Result},
    runtime::Runtime,
};
use serde_json::{Value, json};
pub(super) async fn dispatch(runtime: &Runtime, method: &str, params: Value) -> Result<Value> {
    match method {
        "configuration.get" => {
            let params: DeviceParams = decode(params)?;
            Ok(json!(runtime.configuration(&params.device_id).await?))
        }
        "oled.get" => {
            let params: DeviceParams = decode(params)?;
            Ok(json!(runtime.oled_state(&params.device_id).await?))
        }
        "oled.upload" => {
            let operation = runtime.upload_oled(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "operation.pause" | "operation.resume" => {
            let params: OperationParams = decode(params)?;
            let operation = if method == "operation.pause" {
                runtime.pause_operation(&params.operation_id).await?
            } else {
                runtime.resume_operation(&params.operation_id).await?
            };
            Ok(json!(operation))
        }
        "operation.get" => {
            let params: OperationParams = decode(params)?;
            Ok(json!(runtime.operation(&params.operation_id).await?))
        }
        "keymap.apply" => {
            let operation = runtime.apply_keymap(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "configuration.apply" => {
            let operation = runtime.apply(decode(params)?).await?;
            Ok(json!({"operationId":operation.operation_id,"status":operation.status}))
        }
        "device.connect" | "device.disconnect" => {
            let params: DeviceParams = decode(params)?;
            runtime
                .connect(&params.device_id, method == "device.connect")
                .await?;
            Ok(json!({}))
        }
        _ => Err(Error("UNSUPPORTED_CAPABILITY")),
    }
}
