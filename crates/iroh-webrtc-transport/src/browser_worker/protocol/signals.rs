use super::*;

pub(in crate::browser_worker) fn to_protocol_value<T: serde::Serialize>(
    value: T,
) -> BrowserWorkerResult<Value> {
    let mut value = serde_json::to_value(value).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("failed to encode worker result: {err}"),
        )
    })?;
    normalize_protocol_value(&mut value);
    Ok(value)
}

pub(in crate::browser_worker) fn normalize_protocol_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(signals) = object
                .get_mut("outboundSignals")
                .and_then(Value::as_array_mut)
            {
                for signal in signals {
                    if let Ok(decoded) = serde_json::from_value::<WebRtcSignal>(signal.clone()) {
                        *signal = protocol_signal_value(&decoded, None);
                    }
                }
            }
            let dial_alpn = object
                .get("alpn")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if let Some(bootstrap_signal) = object.get_mut("bootstrapSignal") {
                if let Ok(decoded) =
                    serde_json::from_value::<WebRtcSignal>(bootstrap_signal.clone())
                {
                    *bootstrap_signal = protocol_signal_value(&decoded, dial_alpn.as_deref());
                }
            }
            for value in object.values_mut() {
                normalize_protocol_value(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_protocol_value(value);
            }
        }
        _ => {}
    }
}

pub(in crate::browser_worker) fn protocol_signal_value(
    signal: &WebRtcSignal,
    alpn: Option<&str>,
) -> Value {
    serde_json::to_value(ProtocolWebRtcSignal::from_signal(signal, alpn))
        .expect("protocol signal serialization cannot fail")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
enum ProtocolWebRtcSignal {
    #[serde(rename = "session", rename_all = "camelCase")]
    Session {
        dial_id: String,
        session_key: String,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        #[serde(default)]
        alpn: String,
        transport_intent: BootstrapTransportIntent,
    },
    #[serde(rename = "offer", rename_all = "camelCase")]
    Offer {
        dial_id: String,
        session_key: String,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        sdp: String,
    },
    #[serde(rename = "answer", rename_all = "camelCase")]
    Answer {
        dial_id: String,
        session_key: String,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        sdp: String,
    },
    #[serde(rename = "ice", rename_all = "camelCase")]
    Ice {
        dial_id: String,
        session_key: String,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        ice: WorkerProtocolIceCandidate,
    },
    #[serde(rename = "terminal", rename_all = "camelCase")]
    Terminal {
        dial_id: String,
        session_key: String,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        reason: WebRtcTerminalReason,
        diagnostic: Option<String>,
    },
}

impl ProtocolWebRtcSignal {
    fn from_signal(signal: &WebRtcSignal, alpn: Option<&str>) -> Self {
        let session_key = dial_id_string(signal.dial_id());
        match signal {
            WebRtcSignal::DialRequest {
                dial_id,
                from,
                to,
                generation,
                alpn: signal_alpn,
                transport_intent,
            } => Self::Session {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                alpn: alpn
                    .or_else(|| (!signal_alpn.is_empty()).then_some(signal_alpn.as_str()))
                    .unwrap_or_default()
                    .to_owned(),
                transport_intent: *transport_intent,
            },
            WebRtcSignal::Offer {
                dial_id,
                from,
                to,
                generation,
                sdp,
            } => Self::Offer {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                sdp: sdp.clone(),
            },
            WebRtcSignal::Answer {
                dial_id,
                from,
                to,
                generation,
                sdp,
            } => Self::Answer {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                sdp: sdp.clone(),
            },
            WebRtcSignal::IceCandidate {
                dial_id,
                from,
                to,
                generation,
                candidate,
                sdp_mid,
                sdp_mline_index,
            } => Self::Ice {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                ice: WorkerProtocolIceCandidate::Candidate {
                    candidate: candidate.clone(),
                    sdp_mid: sdp_mid.clone(),
                    sdp_mline_index: *sdp_mline_index,
                },
            },
            WebRtcSignal::EndOfCandidates {
                dial_id,
                from,
                to,
                generation,
            } => Self::Ice {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                ice: WorkerProtocolIceCandidate::EndOfCandidates,
            },
            WebRtcSignal::Terminal {
                dial_id,
                from,
                to,
                generation,
                reason,
                message,
            } => Self::Terminal {
                dial_id: dial_id_string(*dial_id),
                session_key,
                from: *from,
                to: *to,
                generation: *generation,
                reason: *reason,
                diagnostic: message.clone(),
            },
        }
    }

    fn into_signal(self) -> BrowserWorkerResult<WebRtcSignal> {
        match self {
            Self::Session {
                dial_id,
                from,
                to,
                generation,
                alpn,
                transport_intent,
                ..
            } => Ok(WebRtcSignal::dial_request_with_alpn(
                dial_id_from_hex(&dial_id)?,
                from,
                to,
                generation,
                alpn,
                transport_intent,
            )),
            Self::Offer {
                dial_id,
                from,
                to,
                generation,
                sdp,
                ..
            } => Ok(WebRtcSignal::Offer {
                dial_id: dial_id_from_hex(&dial_id)?,
                from,
                to,
                generation,
                sdp,
            }),
            Self::Answer {
                dial_id,
                from,
                to,
                generation,
                sdp,
                ..
            } => Ok(WebRtcSignal::Answer {
                dial_id: dial_id_from_hex(&dial_id)?,
                from,
                to,
                generation,
                sdp,
            }),
            Self::Ice {
                dial_id,
                from,
                to,
                generation,
                ice,
                ..
            } => match ice {
                WorkerProtocolIceCandidate::Candidate {
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                } => Ok(WebRtcSignal::IceCandidate {
                    dial_id: dial_id_from_hex(&dial_id)?,
                    from,
                    to,
                    generation,
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                }),
                WorkerProtocolIceCandidate::EndOfCandidates => Ok(WebRtcSignal::EndOfCandidates {
                    dial_id: dial_id_from_hex(&dial_id)?,
                    from,
                    to,
                    generation,
                }),
            },
            Self::Terminal {
                dial_id,
                from,
                to,
                generation,
                reason,
                diagnostic,
                ..
            } => Ok(WebRtcSignal::terminal_with_message(
                dial_id_from_hex(&dial_id)?,
                from,
                to,
                generation,
                reason,
                diagnostic,
            )),
        }
    }
}

pub(in crate::browser_worker) fn bootstrap_signal_input_from_payload(
    payload: &Value,
) -> BrowserWorkerResult<WorkerBootstrapSignalInput> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Payload {
        signal: ProtocolWebRtcSignal,
        #[serde(default)]
        alpn: Option<String>,
    }

    let payload: Payload = serde_json::from_value(payload.clone()).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::BootstrapFailed,
            format!("malformed bootstrap signal payload: {err}"),
        )
    })?;
    Ok(WorkerBootstrapSignalInput {
        signal: payload.signal.into_signal()?,
        alpn: payload.alpn,
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_worker) fn outbound_signals_from_protocol_value(
    result: &Value,
) -> BrowserWorkerResult<Vec<WebRtcSignal>> {
    let container: OutboundSignalsContainer =
        serde_json::from_value(result.clone()).map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                format!("malformed outboundSignals payload: {err}"),
            )
        })?;
    container
        .outbound_signals
        .unwrap_or_default()
        .into_iter()
        .map(ProtocolWebRtcSignal::into_signal)
        .collect()
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_worker) fn terminal_diagnostic_from_protocol_value(
    result: &Value,
) -> Option<String> {
    let container = serde_json::from_value::<OutboundSignalsContainer>(result.clone()).ok()?;
    container
        .outbound_signals
        .unwrap_or_default()
        .into_iter()
        .find_map(|signal| match signal {
            ProtocolWebRtcSignal::Terminal { diagnostic, .. } => diagnostic,
            _ => None,
        })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutboundSignalsContainer {
    #[serde(default, rename = "outboundSignals", alias = "outbound_signals")]
    outbound_signals: Option<Vec<ProtocolWebRtcSignal>>,
}
