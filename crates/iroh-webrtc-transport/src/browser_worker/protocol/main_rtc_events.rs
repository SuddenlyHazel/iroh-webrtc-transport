use super::*;

pub(in crate::browser_worker) fn main_rtc_result_input_from_payload(
    payload: &Value,
) -> BrowserWorkerResult<WorkerMainRtcResultInput> {
    let payload: MainRtcResultPayload = serde_json::from_value(payload.clone()).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("malformed main RTC result payload: {err}"),
        )
    })?;
    let session_key = WorkerSessionKey::new(payload.session_key.clone())?;
    if let Some(message) = payload.error.as_ref().map(|error| error.message.clone()) {
        return Ok(WorkerMainRtcResultInput {
            session_key,
            event: WorkerMainRtcResultEvent::WebRtcFailed { message },
            error_message: None,
        });
    }
    let event = if let Some(ref event) = payload.event {
        main_rtc_event_from_event_name(&event, &payload)?
    } else if let Some(ref command) = payload.command {
        main_rtc_event_from_command(&command, payload.result.as_ref())?
    } else {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "main RTC result payload must include command or event",
        ));
    };
    Ok(WorkerMainRtcResultInput {
        session_key,
        event,
        error_message: None,
    })
}

pub(in crate::browser_worker) fn main_rtc_event_from_command(
    command: &str,
    result: Option<&Value>,
) -> BrowserWorkerResult<WorkerMainRtcResultEvent> {
    match command {
        MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND => {
            Ok(WorkerMainRtcResultEvent::PeerConnectionCreated)
        }
        MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND => Ok(WorkerMainRtcResultEvent::DataChannelCreated),
        MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND => Ok(WorkerMainRtcResultEvent::DataChannelCreated),
        MAIN_RTC_CREATE_OFFER_COMMAND => Ok(WorkerMainRtcResultEvent::OfferCreated {
            sdp: sdp_result(result, command)?,
        }),
        MAIN_RTC_ACCEPT_OFFER_COMMAND => Ok(WorkerMainRtcResultEvent::OfferAccepted),
        MAIN_RTC_CREATE_ANSWER_COMMAND => Ok(WorkerMainRtcResultEvent::AnswerCreated {
            sdp: sdp_result(result, command)?,
        }),
        MAIN_RTC_ACCEPT_ANSWER_COMMAND => Ok(WorkerMainRtcResultEvent::AnswerAccepted),
        MAIN_RTC_NEXT_LOCAL_ICE_COMMAND => Ok(WorkerMainRtcResultEvent::LocalIce {
            ice: protocol_ice_from_value(result.ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "main RTC local ICE result is missing result",
                )
            })?)?,
        }),
        MAIN_RTC_ADD_REMOTE_ICE_COMMAND => Ok(WorkerMainRtcResultEvent::RemoteIceAccepted),
        MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND => {
            Ok(WorkerMainRtcResultEvent::PeerConnectionCloseAcknowledged)
        }
        other => Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("unsupported main RTC result command {other:?}"),
        )),
    }
}

fn main_rtc_event_from_event_name(
    event: &str,
    payload: &MainRtcResultPayload,
) -> BrowserWorkerResult<WorkerMainRtcResultEvent> {
    match event {
        "dataChannelOpen" => Ok(WorkerMainRtcResultEvent::DataChannelOpen),
        "webrtcFailed" | "failed" => Ok(WorkerMainRtcResultEvent::WebRtcFailed {
            message: payload
                .message
                .clone()
                .or_else(|| payload.reason.clone())
                .unwrap_or_else(|| "main-thread WebRTC failed".to_owned()),
        }),
        "closed" | "webrtcClosed" => Ok(WorkerMainRtcResultEvent::WebRtcClosed {
            message: payload
                .message
                .clone()
                .or_else(|| payload.reason.clone())
                .unwrap_or_else(|| "main-thread WebRTC closed".to_owned()),
        }),
        other => Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("unsupported main RTC result event {other:?}"),
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcResultPayload {
    session_key: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<MainRtcErrorPayload>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcErrorPayload {
    #[serde(default)]
    _code: Option<BrowserWorkerErrorCode>,
    message: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SdpResult {
    sdp: String,
}

fn sdp_result(result: Option<&Value>, command: &str) -> BrowserWorkerResult<String> {
    let Some(result) = result else {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("main RTC {command:?} result is missing result"),
        ));
    };
    serde_json::from_value::<SdpResult>(result.clone())
        .map(|result| result.sdp)
        .map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("main RTC SDP result has invalid shape: {err}"),
            )
        })
}

pub(in crate::browser_worker) fn protocol_ice_from_value(
    value: &Value,
) -> BrowserWorkerResult<WorkerProtocolIceCandidate> {
    serde_json::from_value::<WorkerProtocolIceCandidate>(value.clone()).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("main RTC ICE result has invalid shape: {err}"),
        )
    })
}
