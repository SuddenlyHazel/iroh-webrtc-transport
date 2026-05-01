use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "command", content = "payload")]
pub(in crate::browser_worker) enum WorkerMainRtcCommand {
    #[serde(rename = "main.create-peer-connection")]
    CreatePeerConnection(WorkerMainCreatePeerConnectionPayload),
    #[serde(rename = "main.create-data-channel")]
    CreateDataChannel(WorkerMainCreateDataChannelPayload),
    #[serde(rename = "main.take-data-channel")]
    TakeDataChannel(WorkerMainSessionPayload),
    #[serde(rename = "main.create-offer")]
    CreateOffer(WorkerMainSessionPayload),
    #[serde(rename = "main.accept-offer")]
    AcceptOffer(WorkerMainAcceptOfferPayload),
    #[serde(rename = "main.create-answer")]
    CreateAnswer(WorkerMainSessionPayload),
    #[serde(rename = "main.accept-answer")]
    AcceptAnswer(WorkerMainAcceptAnswerPayload),
    #[serde(rename = "main.next-local-ice")]
    NextLocalIce(WorkerMainSessionPayload),
    #[serde(rename = "main.add-remote-ice")]
    AddRemoteIce(WorkerMainAddRemoteIcePayload),
    #[serde(rename = "main.close-peer-connection")]
    ClosePeerConnection(WorkerMainClosePeerConnectionPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainCreatePeerConnectionPayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) role: WorkerSessionRole,
    pub(in crate::browser_worker) generation: u64,
    pub(in crate::browser_worker) stun_urls: Vec<String>,
    pub(in crate::browser_worker) remote_endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainCreateDataChannelPayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) label: String,
    pub(in crate::browser_worker) protocol: String,
    pub(in crate::browser_worker) ordered: bool,
    pub(in crate::browser_worker) max_retransmits: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainSessionPayload {
    pub(in crate::browser_worker) session_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainAcceptOfferPayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) remote_endpoint: Option<String>,
    pub(in crate::browser_worker) sdp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainAcceptAnswerPayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) sdp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainAddRemoteIcePayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) ice: WorkerProtocolIceCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainClosePeerConnectionPayload {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub(in crate::browser_worker) enum WorkerProtocolIceCandidate {
    #[serde(rename = "candidate", rename_all = "camelCase")]
    Candidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    #[serde(rename = "endOfCandidates")]
    EndOfCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerMainRtcResult {
    pub(in crate::browser_worker) accepted: bool,
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) session: Option<WorkerSessionSnapshot>,
    pub(in crate::browser_worker) connection: Option<WorkerProtocolConnectionInfo>,
    pub(in crate::browser_worker) main_rtc: Vec<WorkerMainRtcCommand>,
    pub(in crate::browser_worker) outbound_signals: Vec<WebRtcSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_worker) struct WorkerMainRtcResultInput {
    pub(in crate::browser_worker) session_key: WorkerSessionKey,
    pub(in crate::browser_worker) event: WorkerMainRtcResultEvent,
    pub(in crate::browser_worker) error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_worker) enum WorkerMainRtcResultEvent {
    PeerConnectionCreated,
    DataChannelCreated,
    OfferCreated { sdp: String },
    OfferAccepted,
    AnswerCreated { sdp: String },
    AnswerAccepted,
    LocalIce { ice: WorkerProtocolIceCandidate },
    RemoteIceAccepted,
    DataChannelOpen,
    WebRtcFailed { message: String },
    WebRtcClosed { message: String },
    PeerConnectionCloseAcknowledged,
}

#[cfg(any(test, all(target_family = "wasm", target_os = "unknown")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingMainRtcRequest {
    pub(super) id: u64,
    pub(super) command: String,
    pub(super) session_key: String,
    pub(super) generation: Option<u64>,
    pub(super) reason: Option<String>,
}
