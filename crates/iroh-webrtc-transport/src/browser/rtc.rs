use super::{js_boundary::*, *};
use crate::error::{Error, Result};
use serde::Deserialize;

pub(super) fn ice_config_from_js(stun_urls: Option<Vec<String>>) -> Result<WebRtcIceConfig> {
    let Some(stun_urls) = stun_urls else {
        return Ok(WebRtcIceConfig::default_direct_stun());
    };
    if stun_urls.is_empty() {
        return Ok(WebRtcIceConfig::default_direct_stun());
    }
    WebRtcIceConfig::direct_only(stun_urls)
}

pub(super) async fn create_offer_for_peer(peer: &RtcPeerConnection) -> Result<String> {
    let value = JsFuture::from(peer.create_offer())
        .await
        .map_err(js_error)?;
    let sdp = rtc_session_description_sdp(&value, "created offer")?;
    let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
    description.set_sdp(&sdp);
    JsFuture::from(peer.set_local_description(&description))
        .await
        .map_err(js_error)?;
    Ok(sdp)
}

pub(super) async fn create_answer_for_peer(peer: &RtcPeerConnection) -> Result<String> {
    let value = JsFuture::from(peer.create_answer())
        .await
        .map_err(js_error)?;
    let sdp = rtc_session_description_sdp(&value, "created answer")?;
    let description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
    description.set_sdp(&sdp);
    JsFuture::from(peer.set_local_description(&description))
        .await
        .map_err(js_error)?;
    Ok(sdp)
}

pub(super) fn rtc_session_description_sdp(value: &JsValue, context: &str) -> Result<String> {
    let description: RtcSessionDescriptionValue = serde_wasm_bindgen::from_value(value.clone())
        .map_err(|err| Error::WebRtc(format!("malformed {context}: {err}")))?;
    let sdp = description.sdp;
    if sdp.is_empty() {
        return Err(Error::WebRtc(format!("{context} did not include SDP")));
    }
    Ok(sdp)
}

#[derive(Deserialize)]
struct RtcSessionDescriptionValue {
    sdp: String,
}

pub(super) async fn set_remote_description_for_peer(
    peer: &RtcPeerConnection,
    type_: RtcSdpType,
    sdp: &str,
) -> Result<()> {
    let description = RtcSessionDescriptionInit::new(type_);
    description.set_sdp(sdp);
    JsFuture::from(peer.set_remote_description(&description))
        .await
        .map_err(js_error)?;
    Ok(())
}

pub(super) async fn add_ice_candidate_for_peer(
    peer: &RtcPeerConnection,
    candidate: &WebRtcIceCandidate,
) -> Result<()> {
    let init = RtcIceCandidateInit::new(&candidate.candidate);
    init.set_sdp_mid(candidate.sdp_mid.as_deref());
    init.set_sdp_m_line_index(candidate.sdp_mline_index);
    JsFuture::from(peer.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init)))
        .await
        .map_err(js_error)?;
    Ok(())
}

pub(super) async fn add_end_of_candidates_for_peer(peer: &RtcPeerConnection) -> Result<()> {
    JsFuture::from(peer.add_ice_candidate_with_opt_rtc_ice_candidate_init(None))
        .await
        .map_err(js_error)?;
    Ok(())
}

pub(super) fn apply_ice_config(
    configuration: &RtcConfiguration,
    ice_config: &WebRtcIceConfig,
) -> Result<()> {
    ice_config.validate_direct_only()?;
    if ice_config.stun_urls.is_empty() {
        return Ok(());
    }

    let servers = js_sys::Array::new();
    for url in &ice_config.stun_urls {
        let server = js_sys::Object::new();
        js_sys::Reflect::set(&server, &JsValue::from_str("urls"), &JsValue::from_str(url))
            .map_err(js_error)?;
        servers.push(&server);
    }
    configuration.set_ice_servers(&servers);
    Ok(())
}
