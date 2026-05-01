#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
use super::names::{MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, STREAM_RECEIVE_CHUNK_COMMAND};

pub(super) const NO_TRANSFERS: &[ProtocolTransferRequirement] = &[];
pub(super) const RESULT_CHANNEL_TRANSFER: &[ProtocolTransferRequirement] =
    &[ProtocolTransferRequirement::new(
        "result.channel",
        "rtc-data-channel",
    )];
pub(super) const RESULT_CHUNK_TRANSFER: &[ProtocolTransferRequirement] =
    &[ProtocolTransferRequirement::new(
        "result.chunk",
        "array-buffer",
    )];

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProtocolTransferRequirement {
    pub(crate) path: &'static str,
    pub(crate) kind: &'static str,
}

impl ProtocolTransferRequirement {
    const fn new(path: &'static str, kind: &'static str) -> Self {
        Self { path, kind }
    }
}

#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) fn response_transfer_requirements_for_command(
    command: &str,
) -> &'static [ProtocolTransferRequirement] {
    match command {
        MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND => RESULT_CHANNEL_TRANSFER,
        STREAM_RECEIVE_CHUNK_COMMAND => RESULT_CHUNK_TRANSFER,
        _ => NO_TRANSFERS,
    }
}
