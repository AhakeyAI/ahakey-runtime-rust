//! JSON-RPC operations for one connection-owned OLED source transfer.
use super::protocol::{FinishResource, decode};
use crate::{
    error::{Error, Result},
    oled::Transfer,
    runtime::Runtime,
};
use serde_json::{Value, json};
pub(super) fn begin(transfer: &mut Option<Transfer>, params: Value) -> Result<Value> {
    if transfer.is_some() {
        Err(Error("BUSY"))
    } else {
        match decode(params).and_then(crate::oled::Transfer::begin) {
            Ok(upload) => {
                let response =
                    json!({"uploadId":upload.id,"chunkBytes":crate::oled::MAX_CHUNK_BYTES});
                *transfer = Some(upload);
                Ok(response)
            }
            Err(error) => Err(error),
        }
    }
}
pub(super) async fn finish(
    runtime: &Runtime,
    transfer: &mut Option<Transfer>,
    params: Value,
) -> Result<Value> {
    match decode::<FinishResource>(params) {
        Ok(params) => {
            if transfer
                .as_ref()
                .is_some_and(|t| t.id == params.upload_id && t.total > 0)
            {
                match transfer.take().unwrap().complete(params.upload_id) {
                    Ok((bytes, hash)) => runtime
                        .resources
                        .finish(bytes, hash)
                        .await
                        .map(|resource| json!(resource)),
                    Err(error) => Err(error),
                }
            } else {
                Err(Error("RESOURCE_UPLOAD_NOT_FOUND"))
            }
        }
        Err(error) => Err(error),
    }
}
pub(super) fn abort(transfer: &mut Option<Transfer>, params: Value) -> Result<Value> {
    match decode::<FinishResource>(params) {
        Ok(params) if transfer.as_ref().is_some_and(|t| t.id == params.upload_id) => {
            *transfer = None;
            Ok(json!({}))
        }
        Ok(_) => Err(Error("RESOURCE_UPLOAD_NOT_FOUND")),
        Err(error) => Err(error),
    }
}
