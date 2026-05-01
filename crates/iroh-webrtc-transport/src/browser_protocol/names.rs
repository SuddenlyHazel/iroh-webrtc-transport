pub(crate) const WORKER_SPAWN_COMMAND: &str = "worker.spawn";
pub(crate) const WORKER_DIAL_COMMAND: &str = "worker.dial";
pub(crate) const WORKER_ACCEPT_OPEN_COMMAND: &str = "worker.accept-open";
pub(crate) const WORKER_ACCEPT_NEXT_COMMAND: &str = "worker.accept-next";
pub(crate) const WORKER_ACCEPT_CLOSE_COMMAND: &str = "worker.accept-close";
#[cfg(any(
    test,
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub(crate) const WORKER_BOOTSTRAP_SIGNAL_COMMAND: &str = "worker.bootstrap-signal";
pub(crate) const WORKER_ATTACH_DATA_CHANNEL_COMMAND: &str = "worker.attach-data-channel";
#[cfg(any(
    test,
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub(crate) const WORKER_MAIN_RTC_RESULT_COMMAND: &str = "worker.main-rtc-result";
pub(crate) const WORKER_CONNECTION_CLOSE_COMMAND: &str = "worker.connection-close";
pub(crate) const WORKER_NODE_CLOSE_COMMAND: &str = "worker.node-close";

pub(crate) const BENCHMARK_ECHO_OPEN_COMMAND: &str = "benchmark.echo-open";
pub(crate) const BENCHMARK_LATENCY_COMMAND: &str = "benchmark.latency";
pub(crate) const BENCHMARK_THROUGHPUT_COMMAND: &str = "benchmark.throughput";

pub(crate) const MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND: &str = "main.create-peer-connection";
pub(crate) const MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND: &str = "main.create-data-channel";
pub(crate) const MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND: &str = "main.take-data-channel";
pub(crate) const MAIN_RTC_CREATE_OFFER_COMMAND: &str = "main.create-offer";
pub(crate) const MAIN_RTC_ACCEPT_OFFER_COMMAND: &str = "main.accept-offer";
pub(crate) const MAIN_RTC_CREATE_ANSWER_COMMAND: &str = "main.create-answer";
pub(crate) const MAIN_RTC_ACCEPT_ANSWER_COMMAND: &str = "main.accept-answer";
pub(crate) const MAIN_RTC_NEXT_LOCAL_ICE_COMMAND: &str = "main.next-local-ice";
pub(crate) const MAIN_RTC_ADD_REMOTE_ICE_COMMAND: &str = "main.add-remote-ice";
pub(crate) const MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND: &str = "main.close-peer-connection";

pub(crate) const STREAM_OPEN_BI_COMMAND: &str = "stream.open-bi";
pub(crate) const STREAM_ACCEPT_BI_COMMAND: &str = "stream.accept-bi";
pub(crate) const STREAM_SEND_CHUNK_COMMAND: &str = "stream.send-chunk";
pub(crate) const STREAM_RECEIVE_CHUNK_COMMAND: &str = "stream.receive-chunk";
pub(crate) const STREAM_CLOSE_SEND_COMMAND: &str = "stream.close-send";
pub(crate) const STREAM_RESET_COMMAND: &str = "stream.reset";
pub(crate) const STREAM_CANCEL_PENDING_COMMAND: &str = "stream.cancel-pending";
