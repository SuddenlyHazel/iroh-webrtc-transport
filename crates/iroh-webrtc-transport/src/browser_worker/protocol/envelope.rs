use super::*;
use std::future::Future;

fn worker_request_envelope(
    message: Value,
) -> BrowserWorkerResult<Option<StrictWorkerRequestEnvelope>> {
    let peek: WorkerEnvelopeKind = serde_json::from_value(message.clone()).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("malformed worker envelope: {err}"),
        )
    })?;
    if peek.kind != "request" {
        return Ok(None);
    }
    let envelope: StrictWorkerRequestEnvelope = serde_json::from_value(message).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("malformed worker request envelope: {err}"),
        )
    })?;
    Ok(Some(envelope))
}

pub(in crate::browser_worker) async fn worker_response_from_message<F, Fut>(
    message: Value,
    mut handle_command: F,
) -> BrowserWorkerResult<Option<Value>>
where
    F: FnMut(String, Value) -> Fut,
    Fut: Future<Output = Result<Value, BrowserWorkerWireError>>,
{
    let Some(request) = worker_request_envelope(message)? else {
        return Ok(None);
    };
    Ok(Some(worker_response_envelope(
        request.id,
        handle_command(
            request.command,
            request.payload.unwrap_or_else(|| json!({})),
        )
        .await,
    )))
}

fn worker_response_envelope(id: u64, response: Result<Value, BrowserWorkerWireError>) -> Value {
    match response {
        Ok(result) => json!({
            "kind": "response",
            "id": id,
            "ok": true,
            "result": result,
        }),
        Err(error) => json!({
            "kind": "response",
            "id": id,
            "ok": false,
            "error": error,
        }),
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerEnvelopeKind {
    kind: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictWorkerRequestEnvelope {
    #[serde(rename = "kind")]
    _kind: String,
    id: u64,
    command: String,
    #[serde(default)]
    payload: Option<Value>,
}
