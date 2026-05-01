use super::*;
use super::{capabilities::*, js_wire::*, rtc::*, rtc_control_wire::*};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(super) struct WebRtcMainThreadRegistry {
    sessions: Rc<RefCell<HashMap<String, MainThreadSession>>>,
    rtc_control_port: Rc<RefCell<Option<MainThreadRtcControlPort>>>,
}

struct MainThreadSession {
    peer: RtcPeerConnection,
    local_ice: LocalIceQueue,
    incoming_data_channels: Rc<RefCell<VecDeque<RtcDataChannel>>>,
    incoming_data_channel_waiters: Rc<RefCell<VecDeque<oneshot::Sender<RtcDataChannel>>>>,
    _ice_handler: Closure<dyn FnMut(RtcPeerConnectionIceEvent)>,
    _data_channel_handler: Closure<dyn FnMut(RtcDataChannelEvent)>,
}

struct MainThreadRtcControlPort {
    port: MessagePort,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
}

impl WebRtcMainThreadRegistry {
    pub(super) fn new() -> Self {
        Self {
            sessions: Rc::new(RefCell::new(HashMap::new())),
            rtc_control_port: Rc::new(RefCell::new(None)),
        }
    }

    pub(super) fn attach_rtc_control_port(
        &self,
        port: MessagePort,
    ) -> std::result::Result<(), JsValue> {
        self.detach_rtc_control_port();

        let handler_registry = self.clone_for_port_handler();
        let handler_port = port.clone();
        let message_handler = Closure::wrap(Box::new(move |event: MessageEvent| {
            let registry = handler_registry.clone_for_port_handler();
            let response_port = handler_port.clone();
            let message = event.data();
            spawn_local(async move {
                registry
                    .handle_rtc_control_port_message(response_port, message)
                    .await;
            });
        }) as Box<dyn FnMut(_)>);

        port.set_onmessage(Some(message_handler.as_ref().unchecked_ref()));
        port.start();
        *self.rtc_control_port.borrow_mut() = Some(MainThreadRtcControlPort {
            port,
            _message_handler: message_handler,
        });
        Ok(())
    }

    pub(super) fn detach_rtc_control_port(&self) {
        if let Some(binding) = self.rtc_control_port.borrow_mut().take() {
            binding.port.set_onmessage(None);
            binding.port.close();
        }
    }
}

impl Clone for WebRtcMainThreadRegistry {
    fn clone(&self) -> Self {
        Self {
            sessions: self.sessions.clone(),
            rtc_control_port: self.rtc_control_port.clone(),
        }
    }
}

impl WebRtcMainThreadRegistry {
    fn clone_for_port_handler(&self) -> Self {
        Self {
            sessions: self.sessions.clone(),
            rtc_control_port: self.rtc_control_port.clone(),
        }
    }

    async fn handle_rtc_control_port_message(&self, port: MessagePort, message: JsValue) {
        match rtc_control_request_from_js(&message) {
            Ok(request) => {
                let response = match self
                    .try_handle_command(&request.command, request.payload)
                    .await
                {
                    Ok(result) => {
                        let transfer = match transfer_list_for_main_rtc_result(&result) {
                            Ok(transfer) => transfer,
                            Err(error) => {
                                let wire_error =
                                    error_to_wire_value_for_command(Some(&request.command), error);
                                let response = rtc_control_error_response(request.id, wire_error);
                                let _ = post_rtc_control_response(&port, response, None);
                                return;
                            }
                        };
                        rtc_control_success_response(request.id, result)
                            .map(|response| (response, Some(transfer)))
                    }
                    Err(error) => {
                        let wire_error =
                            error_to_wire_value_for_command(Some(&request.command), error);
                        Ok((rtc_control_error_response(request.id, wire_error), None))
                    }
                };

                if let Ok((response, transfer)) = response {
                    if let Err(error) =
                        post_rtc_control_response(&port, response, transfer.as_ref())
                    {
                        let wire_error =
                            error_to_wire_value_for_command(Some(&request.command), error);
                        let response = rtc_control_error_response(request.id, wire_error);
                        let _ = post_rtc_control_response(&port, response, None);
                    }
                }
            }
            Err(error) => {
                if let Some(id) = rtc_control_message_id(&message) {
                    let wire_error = error_to_wire_value_for_command(None, error);
                    let response = rtc_control_error_response(id, wire_error);
                    let _ = post_rtc_control_response(&port, response, None);
                }
            }
        }
    }

    async fn try_handle_command(&self, command: &str, payload: JsValue) -> Result<JsValue> {
        match command {
            MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND => {
                let request: MainRtcCreatePeerConnectionPayload =
                    decode_command_payload(command, payload)?;
                self.try_create_peer_connection(request.session_key.clone(), request.stun_urls)?;
                encode_command_result(
                    command,
                    &MainRtcCreatePeerConnectionResult {
                        session_key: request.session_key.clone(),
                        peer_connection_key: request.session_key,
                    },
                )
            }
            MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND => {
                let request: MainRtcCreateDataChannelPayload =
                    decode_command_payload(command, payload)?;
                let channel = self.try_create_data_channel_with_options(
                    &request.session_key,
                    request.label.as_deref(),
                    request.protocol.as_deref(),
                    request.ordered.unwrap_or(false),
                    request.max_retransmits.unwrap_or(0),
                )?;
                encode_command_result(
                    command,
                    &MainRtcDataChannelResult {
                        session_key: request.session_key,
                        channel,
                    },
                )
            }
            MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND => {
                let request: MainRtcSessionPayload = decode_command_payload(command, payload)?;
                let channel = self.wait_next_data_channel(&request.session_key).await?;
                validate_data_channel_transferable(&channel)?;
                encode_command_result(
                    command,
                    &MainRtcDataChannelResult {
                        session_key: request.session_key,
                        channel,
                    },
                )
            }
            MAIN_RTC_CREATE_OFFER_COMMAND => {
                let request: MainRtcSessionPayload = decode_command_payload(command, payload)?;
                let peer = self.peer_for(&request.session_key)?;
                let sdp = create_offer_for_peer(&peer).await?;
                encode_command_result(command, &MainRtcSdpResult { sdp })
            }
            MAIN_RTC_ACCEPT_OFFER_COMMAND => {
                let request: MainRtcSessionSdpPayload = decode_command_payload(command, payload)?;
                let peer = self.peer_for(&request.session_key)?;
                set_remote_description_for_peer(&peer, RtcSdpType::Offer, &request.sdp).await?;
                encode_command_result(command, &MainRtcEmptyResult {})
            }
            MAIN_RTC_CREATE_ANSWER_COMMAND => {
                let request: MainRtcSessionPayload = decode_command_payload(command, payload)?;
                let peer = self.peer_for(&request.session_key)?;
                let sdp = create_answer_for_peer(&peer).await?;
                encode_command_result(command, &MainRtcSdpResult { sdp })
            }
            MAIN_RTC_ACCEPT_ANSWER_COMMAND => {
                let request: MainRtcSessionSdpPayload = decode_command_payload(command, payload)?;
                let peer = self.peer_for(&request.session_key)?;
                set_remote_description_for_peer(&peer, RtcSdpType::Answer, &request.sdp).await?;
                encode_command_result(command, &MainRtcEmptyResult {})
            }
            MAIN_RTC_NEXT_LOCAL_ICE_COMMAND => {
                let request: MainRtcSessionPayload = decode_command_payload(command, payload)?;
                let local_ice = self.local_ice_for(&request.session_key)?;
                local_ice.next().await.and_then(local_ice_to_js)
            }
            MAIN_RTC_ADD_REMOTE_ICE_COMMAND => {
                let request: MainRtcAddRemoteIcePayload = decode_command_payload(command, payload)?;
                let peer = self.peer_for(&request.session_key)?;
                match local_ice_from_payload(request.ice) {
                    LocalIceEvent::Candidate(candidate) => {
                        add_ice_candidate_for_peer(&peer, &candidate).await?;
                    }
                    LocalIceEvent::EndOfCandidates => {
                        add_end_of_candidates_for_peer(&peer).await?;
                    }
                }
                encode_command_result(command, &MainRtcEmptyResult {})
            }
            MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND => {
                let request: MainRtcClosePeerConnectionPayload =
                    decode_command_payload(command, payload)?;
                self.try_close_peer_connection(&request.session_key)?;
                encode_command_result(command, &MainRtcCloseResult { closed: true })
            }
            _ => Err(Error::WebRtc(format!(
                "unknown main-thread RTC command: {command}"
            ))),
        }
    }

    fn try_create_peer_connection(
        &self,
        session_key: String,
        stun_urls: Option<Vec<String>>,
    ) -> Result<()> {
        validate_rtc_peer_connection_available()?;
        validate_session_key(&session_key)?;
        if self.sessions.borrow().contains_key(&session_key) {
            return Err(Error::InvalidConfig("duplicate WebRTC session key"));
        }

        let ice_config = ice_config_from_js(stun_urls)?;
        let configuration = RtcConfiguration::new();
        apply_ice_config(&configuration, &ice_config)?;
        let peer = RtcPeerConnection::new_with_configuration(&configuration).map_err(js_error)?;
        let local_ice =
            LocalIceQueue::with_capacity(crate::config::DEFAULT_LOCAL_ICE_QUEUE_CAPACITY);
        let incoming_data_channels = Rc::new(RefCell::new(VecDeque::new()));
        let incoming_data_channel_waiters = Rc::new(RefCell::new(VecDeque::<
            oneshot::Sender<RtcDataChannel>,
        >::new()));

        let ice_queue = local_ice.clone();
        let ice_handler = Closure::wrap(Box::new(move |event: RtcPeerConnectionIceEvent| {
            let local_event = match event.candidate() {
                Some(candidate) => LocalIceEvent::Candidate(WebRtcIceCandidate {
                    candidate: candidate.candidate(),
                    sdp_mid: candidate.sdp_mid(),
                    sdp_mline_index: candidate.sdp_m_line_index(),
                }),
                None => LocalIceEvent::EndOfCandidates,
            };
            let _ = ice_queue.push(local_event);
        }) as Box<dyn FnMut(_)>);
        peer.set_onicecandidate(Some(ice_handler.as_ref().unchecked_ref()));

        let incoming_queue = incoming_data_channels.clone();
        let incoming_waiters = incoming_data_channel_waiters.clone();
        let data_channel_handler = Closure::wrap(Box::new(move |event: RtcDataChannelEvent| {
            let channel = event.channel();
            channel.set_binary_type(RtcDataChannelType::Arraybuffer);
            match validate_data_channel_transferable(&channel) {
                Ok(()) => {
                    let waiter = incoming_waiters.borrow_mut().pop_front();
                    if let Some(waiter) = waiter {
                        let _ = waiter.send(channel);
                    } else {
                        incoming_queue.borrow_mut().push_back(channel);
                    }
                }
                Err(_) => {}
            }
        }) as Box<dyn FnMut(_)>);
        peer.set_ondatachannel(Some(data_channel_handler.as_ref().unchecked_ref()));

        self.sessions.borrow_mut().insert(
            session_key.clone(),
            MainThreadSession {
                peer,
                local_ice,
                incoming_data_channels,
                incoming_data_channel_waiters,
                _ice_handler: ice_handler,
                _data_channel_handler: data_channel_handler,
            },
        );
        Ok(())
    }

    fn try_create_data_channel_with_options(
        &self,
        session_key: &str,
        label: Option<&str>,
        protocol: Option<&str>,
        ordered: bool,
        max_retransmits: u16,
    ) -> Result<RtcDataChannel> {
        if ordered {
            return Err(Error::InvalidConfig(
                "browser WebRTC data channels must be unordered",
            ));
        }
        if max_retransmits != 0 {
            return Err(Error::InvalidConfig(
                "browser WebRTC data channels must use maxRetransmits 0",
            ));
        }
        let peer = self.peer_for(session_key)?;
        let init = RtcDataChannelInit::new();
        init.set_ordered(ordered);
        init.set_max_retransmits(max_retransmits);
        if let Some(protocol) = protocol.filter(|protocol| !protocol.is_empty()) {
            init.set_protocol(protocol);
        }
        let label = label
            .filter(|label| !label.is_empty())
            .unwrap_or(WEBRTC_DATA_CHANNEL_LABEL);
        let channel = peer.create_data_channel_with_data_channel_dict(label, &init);
        channel.set_binary_type(RtcDataChannelType::Arraybuffer);
        validate_data_channel_transferable(&channel)?;
        Ok(channel)
    }

    fn try_take_next_data_channel(&self, session_key: &str) -> Result<Option<RtcDataChannel>> {
        let incoming = self.incoming_data_channels_for(session_key)?;
        let channel = incoming.borrow_mut().pop_front();
        if let Some(channel) = &channel {
            validate_data_channel_transferable(channel)?;
        }
        Ok(channel)
    }

    async fn wait_next_data_channel(&self, session_key: &str) -> Result<RtcDataChannel> {
        if let Some(channel) = self.try_take_next_data_channel(session_key)? {
            return Ok(channel);
        }
        let waiters = self.incoming_data_channel_waiters_for(session_key)?;
        let (sender, receiver) = oneshot::channel();
        waiters.borrow_mut().push_back(sender);
        receiver.await.map_err(|_| Error::SessionClosed)
    }

    fn try_close_peer_connection(&self, session_key: &str) -> Result<()> {
        validate_session_key(session_key)?;
        if let Some(session) = self.sessions.borrow_mut().remove(session_key) {
            session.peer.close();
            session.local_ice.close();
        }
        Ok(())
    }

    fn peer_for(&self, session_key: &str) -> Result<RtcPeerConnection> {
        validate_session_key(session_key)?;
        self.sessions
            .borrow()
            .get(session_key)
            .map(|session| session.peer.clone())
            .ok_or(Error::UnknownSession)
    }

    fn local_ice_for(&self, session_key: &str) -> Result<LocalIceQueue> {
        validate_session_key(session_key)?;
        self.sessions
            .borrow()
            .get(session_key)
            .map(|session| session.local_ice.clone())
            .ok_or(Error::UnknownSession)
    }

    fn incoming_data_channels_for(
        &self,
        session_key: &str,
    ) -> Result<Rc<RefCell<VecDeque<RtcDataChannel>>>> {
        validate_session_key(session_key)?;
        self.sessions
            .borrow()
            .get(session_key)
            .map(|session| session.incoming_data_channels.clone())
            .ok_or(Error::UnknownSession)
    }

    fn incoming_data_channel_waiters_for(
        &self,
        session_key: &str,
    ) -> Result<Rc<RefCell<VecDeque<oneshot::Sender<RtcDataChannel>>>>> {
        validate_session_key(session_key)?;
        self.sessions
            .borrow()
            .get(session_key)
            .map(|session| session.incoming_data_channel_waiters.clone())
            .ok_or(Error::UnknownSession)
    }
}

fn decode_command_payload<T: DeserializeOwned>(command: &str, payload: JsValue) -> Result<T> {
    serde_wasm_bindgen::from_value(payload)
        .map_err(|err| Error::WebRtc(format!("malformed payload for {command}: {err}")))
}

fn encode_command_result<T: Serialize>(command: &str, result: &T) -> Result<JsValue> {
    serde_wasm_bindgen::to_value(result)
        .map_err(|err| Error::WebRtc(format!("failed to encode result for {command}: {err}")))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcCreatePeerConnectionPayload {
    session_key: String,
    #[serde(default)]
    stun_urls: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcCreateDataChannelPayload {
    session_key: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    ordered: Option<bool>,
    #[serde(default)]
    max_retransmits: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcSessionPayload {
    session_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcSessionSdpPayload {
    session_key: String,
    sdp: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcAddRemoteIcePayload {
    session_key: String,
    ice: MainRtcIcePayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcClosePeerConnectionPayload {
    session_key: String,
    #[serde(default)]
    _reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcCreatePeerConnectionResult {
    session_key: String,
    peer_connection_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcDataChannelResult {
    session_key: String,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    channel: RtcDataChannel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcSdpResult {
    sdp: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcCloseResult {
    closed: bool,
}

#[derive(Serialize)]
struct MainRtcEmptyResult {}
