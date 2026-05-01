use super::*;

pub(in crate::browser_worker) fn endpoint_id_string(endpoint_id: EndpointId) -> String {
    endpoint_id.to_string()
}

pub(in crate::browser_worker) fn dial_id_string(dial_id: DialId) -> String {
    WorkerSessionKey::from(dial_id).as_str().to_owned()
}

pub(in crate::browser_worker) fn accept_key_string(accept_id: WorkerAcceptId) -> String {
    format!("accept-{}", accept_id.0)
}

pub(in crate::browser_worker) fn connection_key_string(
    connection_key: WorkerConnectionKey,
) -> String {
    format!("conn-{}", connection_key.0)
}

pub(in crate::browser_worker) fn stream_key_string(stream_key: WorkerStreamKey) -> String {
    format!("stream-{}", stream_key.0)
}

pub(in crate::browser_worker) fn custom_addr_string(addr: &CustomAddr) -> String {
    let mut encoded = format!("{}:", addr.id());
    for byte in addr.data() {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub(in crate::browser_worker) fn validate_alpn(alpn: String) -> BrowserWorkerResult<String> {
    if alpn.is_empty() {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::UnsupportedAlpn,
            "ALPN must not be empty",
        ));
    }
    Ok(alpn)
}

pub(in crate::browser_worker) fn bootstrap_alpn_str() -> &'static str {
    std::str::from_utf8(WEBRTC_BOOTSTRAP_ALPN).expect("WebRTC bootstrap ALPN must be valid UTF-8")
}

pub(in crate::browser_worker) fn required_connection_key(
    payload: &Value,
    field: &str,
) -> BrowserWorkerResult<WorkerConnectionKey> {
    let value = required_string_field(payload, field)?;
    parse_prefixed_key(value, "conn-", field, BrowserWorkerErrorCode::WebRtcFailed)
        .map(WorkerConnectionKey)
}

pub(in crate::browser_worker) fn parse_prefixed_key(
    value: &str,
    prefix: &str,
    field: &str,
    code: BrowserWorkerErrorCode,
) -> BrowserWorkerResult<u64> {
    let raw = value.strip_prefix(prefix).ok_or_else(|| {
        BrowserWorkerError::new(
            code,
            format!("payload field {field:?} must start with {prefix:?}"),
        )
    })?;
    raw.parse::<u64>().map_err(|err| {
        BrowserWorkerError::new(
            code,
            format!("payload field {field:?} has an invalid numeric suffix: {err}"),
        )
    })
}

pub(in crate::browser_worker) fn dial_id_from_hex(value: &str) -> BrowserWorkerResult<DialId> {
    if value.len() != 32 {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::BootstrapFailed,
            "dial id must be a 32-character lowercase hex string",
        ));
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                format!("dial id contains invalid hex: {err}"),
            )
        })?;
    }
    Ok(DialId(bytes))
}

fn required_string_field<'a>(payload: &'a Value, field: &str) -> BrowserWorkerResult<&'a str> {
    let value = payload.as_object().and_then(|object| object.get(field));
    value.and_then(Value::as_str).ok_or_else(|| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("payload field {field:?} must be a string key"),
        )
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const _: fn(SecretKey, WebRtcTransport) = |secret_key, transport| {
    let builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0).secret_key(secret_key);
    let _bind_future = crate::transport::configure_endpoint(builder, transport).bind();
};
