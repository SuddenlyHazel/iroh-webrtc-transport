use super::*;
use serde::Deserialize;

pub(in crate::browser_worker) fn main_rtc_command_request_envelope(
    id: u64,
    command: &WorkerMainRtcCommand,
) -> BrowserWorkerResult<(Value, PendingMainRtcRequest)> {
    let command_value = to_protocol_value(command.clone())?;
    let command_wire: MainRtcCommandWire =
        serde_json::from_value(command_value).map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to decode encoded main RTC command: {err}"),
            )
        })?;
    let payload = command_wire.payload.unwrap_or_else(|| json!({}));
    let payload_metadata: MainRtcCommandPayloadMetadata = serde_json::from_value(payload.clone())
        .map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("main RTC command payload has invalid shape: {err}"),
        )
    })?;
    let session_key = payload_metadata.session_key.ok_or_else(|| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "main RTC command payload is missing sessionKey",
        )
    })?;
    let envelope = serde_json::to_value(MainRtcControlRequestEnvelope {
        kind: "request",
        id,
        command: command_wire.command.clone(),
        payload: Some(payload),
    })
    .map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("failed to encode main RTC command request envelope: {err}"),
        )
    })?;
    Ok((
        envelope,
        PendingMainRtcRequest {
            id,
            command: command_wire.command,
            session_key,
            generation: payload_metadata.generation,
            reason: payload_metadata.reason,
        },
    ))
}

pub(in crate::browser_worker) fn main_rtc_commands_from_protocol_value(
    value: &Value,
) -> BrowserWorkerResult<Vec<WorkerMainRtcCommand>> {
    let container: MainRtcCommandContainer =
        serde_json::from_value(value.clone()).map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to decode main RTC command sequence: {err}"),
            )
        })?;
    Ok(container.main_rtc.unwrap_or_default())
}

pub(in crate::browser_worker) fn clear_main_rtc_commands(value: &mut Value) {
    if let Value::Object(object) = value {
        object.remove("mainRtc");
        object.remove("main_rtc");
    }
}

pub(in crate::browser_worker) fn worker_main_rtc_result_payload_from_response(
    pending: &PendingMainRtcRequest,
    result: Option<Value>,
    error: Option<BrowserWorkerWireError>,
) -> BrowserWorkerResult<Value> {
    serde_json::to_value(MainRtcResultEnvelope {
        session_key: pending.session_key.clone(),
        command: pending.command.clone(),
        reason: pending.reason.clone(),
        result,
        error,
    })
    .map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("failed to encode main RTC response payload: {err}"),
        )
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_worker) fn main_rtc_data_channel_result_payload(
    pending: &PendingMainRtcRequest,
) -> Option<Value> {
    if pending.command != MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND
        && pending.command != MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND
    {
        return None;
    }
    Some(
        serde_json::to_value(MainRtcDataChannelAttachmentPayload {
            session_key: pending.session_key.clone(),
        })
        .ok()?,
    )
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_worker) enum PendingDialAction {
    WebRtc,
    Relay,
    Fail,
    Closed,
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_worker) fn pending_dial_action_from_session(
    session: &Value,
) -> Option<PendingDialAction> {
    let session: PendingDialSessionState = serde_json::from_value(session.clone()).ok()?;
    match session.resolved_transport.as_deref() {
        Some("irohRelay") => return Some(PendingDialAction::Relay),
        Some("webrtc") => return None,
        _ => {}
    }
    match session.channel_attachment.as_deref() {
        Some("open") => return Some(PendingDialAction::WebRtc),
        _ => {}
    }
    match session.lifecycle.as_deref() {
        Some("relaySelected") => Some(PendingDialAction::Relay),
        Some("failed") => Some(PendingDialAction::Fail),
        Some("closed") => Some(PendingDialAction::Closed),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcCommandWire {
    command: String,
    #[serde(default)]
    payload: Option<Value>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MainRtcCommandPayloadMetadata {
    #[serde(default, alias = "session_key")]
    session_key: Option<String>,
    #[serde(default)]
    generation: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcControlRequestEnvelope {
    kind: &'static str,
    id: u64,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcCommandContainer {
    #[serde(default, rename = "mainRtc", alias = "main_rtc")]
    main_rtc: Option<Vec<WorkerMainRtcCommand>>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcResultEnvelope {
    session_key: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BrowserWorkerWireError>,
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcDataChannelAttachmentPayload {
    session_key: String,
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingDialSessionState {
    #[serde(default)]
    resolved_transport: Option<String>,
    #[serde(default)]
    channel_attachment: Option<String>,
    #[serde(default)]
    lifecycle: Option<String>,
}
