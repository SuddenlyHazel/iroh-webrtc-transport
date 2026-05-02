use super::*;

pub(in crate::browser_runtime) fn endpoint_id_string(endpoint_id: EndpointId) -> String {
    endpoint_id.to_string()
}

pub(in crate::browser_runtime) fn dial_id_string(dial_id: DialId) -> String {
    BrowserSessionKey::from(dial_id).as_str().to_owned()
}

pub(in crate::browser_runtime) fn accept_key_string(accept_id: BrowserAcceptId) -> String {
    format!("accept-{}", accept_id.0)
}

pub(in crate::browser_runtime) fn connection_key_string(
    connection_key: BrowserConnectionKey,
) -> String {
    format!("conn-{}", connection_key.0)
}

pub(in crate::browser_runtime) fn stream_key_string(stream_key: BrowserStreamKey) -> String {
    format!("stream-{}", stream_key.0)
}

pub(in crate::browser_runtime) fn custom_addr_string(addr: &CustomAddr) -> String {
    let mut encoded = format!("{}:", addr.id());
    for byte in addr.data() {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

pub(in crate::browser_runtime) fn validate_alpn(alpn: String) -> BrowserRuntimeResult<String> {
    if alpn.is_empty() {
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::UnsupportedAlpn,
            "ALPN must not be empty",
        ));
    }
    Ok(alpn)
}

pub(in crate::browser_runtime) fn bootstrap_alpn_str() -> &'static str {
    std::str::from_utf8(WEBRTC_BOOTSTRAP_ALPN).expect("WebRTC bootstrap ALPN must be valid UTF-8")
}

pub(in crate::browser_runtime) fn parse_prefixed_key(
    value: &str,
    prefix: &str,
    field: &str,
    code: BrowserRuntimeErrorCode,
) -> BrowserRuntimeResult<u64> {
    let raw = value.strip_prefix(prefix).ok_or_else(|| {
        BrowserRuntimeError::new(
            code,
            format!("value field {field:?} must start with {prefix:?}"),
        )
    })?;
    raw.parse::<u64>().map_err(|err| {
        BrowserRuntimeError::new(
            code,
            format!("value field {field:?} has an invalid numeric suffix: {err}"),
        )
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
const _: fn(SecretKey, WebRtcTransport) = |secret_key, transport| {
    let builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0).secret_key(secret_key);
    let _bind_future = crate::transport::configure_endpoint(builder, transport).bind();
};
