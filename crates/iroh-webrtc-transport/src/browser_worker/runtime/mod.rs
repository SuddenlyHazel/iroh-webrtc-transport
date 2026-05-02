use super::*;

mod dispatcher;

#[derive(Default)]
pub(in crate::browser_worker) struct BrowserWorkerRuntimeCore {
    node: RefCell<Option<BrowserWorkerNode>>,
    worker_protocols: BrowserWorkerProtocolRegistry,
    protocol_transport_prepare_tx: Option<mpsc::Sender<WorkerProtocolTransportPrepareRequest>>,
    spawn_lock: tokio::sync::Mutex<()>,
}

impl BrowserWorkerRuntimeCore {
    #[cfg(test)]
    pub(in crate::browser_worker) fn new() -> Self {
        Self::default()
    }

    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_worker) fn new_with_protocols_and_transport_prepare(
        worker_protocols: BrowserWorkerProtocolRegistry,
        protocol_transport_prepare_tx: mpsc::Sender<WorkerProtocolTransportPrepareRequest>,
    ) -> Self {
        Self {
            node: RefCell::new(None),
            worker_protocols,
            protocol_transport_prepare_tx: Some(protocol_transport_prepare_tx),
            spawn_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(in crate::browser_worker) fn node(&self) -> Option<BrowserWorkerNode> {
        self.node.borrow().clone()
    }

    #[cfg(all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    ))]
    pub(in crate::browser_worker) async fn spawn_node_from_payload(
        &self,
        payload: Value,
        bootstrap_connection_tx: Option<mpsc::UnboundedSender<Connection>>,
    ) -> BrowserWorkerResult<WorkerSpawnResult> {
        let WorkerCommand::Spawn { config } = WorkerCommand::decode(WORKER_SPAWN_COMMAND, payload)?
        else {
            unreachable!("worker spawn command decoded to another variant");
        };
        self.spawn_node(config, bootstrap_connection_tx).await
    }

    pub(super) async fn spawn_node(
        &self,
        mut config: BrowserWorkerNodeConfig,
        bootstrap_connection_tx: Option<mpsc::UnboundedSender<Connection>>,
    ) -> BrowserWorkerResult<WorkerSpawnResult> {
        let _spawn_guard = self.spawn_lock.lock().await;
        if let Some(node) = self.node().filter(|node| !node.is_closed()) {
            if bootstrap_connection_tx.is_some() {
                node.set_bootstrap_connection_sender(bootstrap_connection_tx)?;
            }
            return Ok(node.spawn_result());
        }
        if config.protocol_transport_prepare_tx.is_none() {
            config.protocol_transport_prepare_tx = self.protocol_transport_prepare_tx.clone();
        }
        let node = BrowserWorkerNode::spawn_with_secret_key(
            config,
            SecretKey::generate(),
            self.worker_protocols.clone(),
        )?;
        if bootstrap_connection_tx.is_some() {
            node.set_bootstrap_connection_sender(bootstrap_connection_tx)?;
        }
        node.start_endpoint().await?;
        let result = node.spawn_result();
        *self.node.borrow_mut() = Some(node);
        Ok(result)
    }
}
