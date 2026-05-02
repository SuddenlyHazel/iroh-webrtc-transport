use super::*;

#[derive(Default)]
pub(in crate::browser_runtime) struct BrowserRuntimeCore {
    node: RefCell<Option<BrowserRuntimeNode>>,
    browser_protocols: BrowserProtocolRegistry,
    protocol_transport_prepare_tx: Option<mpsc::Sender<ProtocolTransportPrepareRequest>>,
    spawn_lock: tokio::sync::Mutex<()>,
}

impl BrowserRuntimeCore {
    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_runtime) fn new_with_protocols_and_transport_prepare(
        browser_protocols: BrowserProtocolRegistry,
        protocol_transport_prepare_tx: mpsc::Sender<ProtocolTransportPrepareRequest>,
    ) -> Self {
        Self {
            node: RefCell::new(None),
            browser_protocols,
            protocol_transport_prepare_tx: Some(protocol_transport_prepare_tx),
            spawn_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(in crate::browser_runtime) fn node(&self) -> Option<BrowserRuntimeNode> {
        self.node.borrow().clone()
    }

    pub(in crate::browser_runtime) async fn spawn_node(
        &self,
        mut config: BrowserRuntimeNodeConfig,
        bootstrap_connection_tx: Option<mpsc::UnboundedSender<Connection>>,
    ) -> BrowserRuntimeResult<BrowserNodeInfo> {
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
        let node = BrowserRuntimeNode::spawn_with_secret_key(
            config,
            SecretKey::generate(),
            self.browser_protocols.clone(),
        )?;
        if bootstrap_connection_tx.is_some() {
            node.set_bootstrap_connection_sender(bootstrap_connection_tx)?;
        }
        node.start_endpoint().await?;
        let result = node.spawn_result();
        *self.node.borrow_mut() = Some(node);
        Ok(result)
    }

    pub(in crate::browser_runtime) async fn send_protocol_command<P>(
        &self,
        command: P::Command,
    ) -> BrowserRuntimeResult<()>
    where
        P: crate::browser::BrowserProtocol,
    {
        self.open_node()?;
        let alpn = protocol_alpn_string(P::ALPN)?;
        let command = serde_json::to_value(command).map_err(|err| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("failed to encode browser protocol command: {err}"),
            )
        })?;
        self.browser_protocols
            .handle_command(&alpn, command)
            .await?;
        Ok(())
    }

    pub(in crate::browser_runtime) async fn next_protocol_event<P>(
        &self,
    ) -> BrowserRuntimeResult<Option<P::Event>>
    where
        P: crate::browser::BrowserProtocol,
    {
        self.open_node()?;
        let alpn = protocol_alpn_string(P::ALPN)?;
        let event = self.browser_protocols.next_event(&alpn).await?;
        serde_json::from_value(event).map_err(|err| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("malformed browser protocol event: {err}"),
            )
        })
    }

    pub(in crate::browser_runtime) fn open_node(&self) -> BrowserRuntimeResult<BrowserRuntimeNode> {
        let Some(node) = self.node() else {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::Closed,
                "browser runtime node has not been spawned",
            ));
        };
        if node.is_closed() {
            return Err(BrowserRuntimeError::closed());
        }
        Ok(node)
    }
}

fn protocol_alpn_string(alpn: &[u8]) -> BrowserRuntimeResult<String> {
    if alpn.is_empty() {
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::UnsupportedAlpn,
            "browser protocol ALPN must not be empty",
        ));
    }
    std::str::from_utf8(alpn).map(str::to_owned).map_err(|err| {
        BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::UnsupportedAlpn,
            format!("browser protocol ALPN must be UTF-8: {err}"),
        )
    })
}
