use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex as StdMutex},
};

use bytes::Bytes;
use iroh::{Endpoint, EndpointId};
use iroh_gossip::{
    TopicId,
    api::{Event as GossipEvent, GossipSender},
    net::{GOSSIP_ALPN, Gossip},
};
use iroh_webrtc_transport::{
    Error, Result,
    browser::{
        BrowserDialTransportPreference, BrowserProtocol, BrowserProtocolHandle, BrowserWebRtcNode,
        BrowserWebRtcNodeConfig,
    },
};
use n0_future::{StreamExt, task};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Document, Element, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlTextAreaElement,
    KeyboardEvent,
};

const CHAT_TOPIC_BYTES: [u8; 32] = [0x42; 32];

iroh_webrtc_transport::browser_app! {
    app = start_app => run_app;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ChatGossipCommand {
    Join {
        topic: String,
        peers: Vec<String>,
        endpoint: String,
        name: String,
    },
    Send {
        topic: String,
        from_endpoint: String,
        from_name: String,
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ChatGossipEvent {
    Joined {
        topic: String,
    },
    NeighborUp {
        endpoint: String,
    },
    NeighborDown {
        endpoint: String,
    },
    Chat {
        from_endpoint: String,
        from_name: String,
        text: String,
    },
    System {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ChatWireMessage {
    AboutMe {
        endpoint: String,
        name: String,
    },
    Chat {
        from_endpoint: String,
        from_name: String,
        text: String,
    },
}

#[derive(Debug, Clone)]
struct ChatGossipProtocol {
    gossip: Arc<StdMutex<Option<Gossip>>>,
    topics: Arc<StdMutex<HashMap<String, GossipSender>>>,
    events_tx: mpsc::UnboundedSender<ChatGossipEvent>,
    events_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<ChatGossipEvent>>>,
}

impl Default for ChatGossipProtocol {
    fn default() -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        Self {
            gossip: Arc::new(StdMutex::new(None)),
            topics: Arc::new(StdMutex::new(HashMap::new())),
            events_tx,
            events_rx: Arc::new(AsyncMutex::new(events_rx)),
        }
    }
}

impl BrowserProtocol for ChatGossipProtocol {
    const ALPN: &'static [u8] = GOSSIP_ALPN;

    type Command = ChatGossipCommand;
    type Event = ChatGossipEvent;
    type Handler = Gossip;

    fn handler(&self, endpoint: Endpoint) -> Self::Handler {
        self.gossip(endpoint)
    }

    async fn handle_command(&self, command: Self::Command) -> Result<()> {
        match command {
            ChatGossipCommand::Join {
                topic,
                peers,
                endpoint,
                name,
            } => {
                let topic_id = parse_topic(&topic)?;
                let peers = parse_peers(&peers)?;
                let gossip = self.existing_gossip()?;
                let topic_handle = gossip
                    .subscribe(topic_id, peers)
                    .await
                    .map_err(error_from_display)?;
                let (sender, mut receiver) = topic_handle.split();
                self.topics
                    .lock()
                    .expect("chat gossip topic mutex poisoned")
                    .insert(topic.clone(), sender.clone());
                let events = self.events_tx.clone();
                let topic_for_task = topic.clone();
                let sender_for_task = sender.clone();
                let endpoint_for_task = endpoint.clone();
                let name_for_task = name.clone();
                task::spawn(async move {
                    while let Some(event) = receiver.next().await {
                        match event {
                            Ok(GossipEvent::NeighborUp(endpoint)) => {
                                let _ = events.send(ChatGossipEvent::NeighborUp {
                                    endpoint: endpoint.to_string(),
                                });
                                if let Err(error) = broadcast_wire(
                                    &sender_for_task,
                                    ChatWireMessage::AboutMe {
                                        endpoint: endpoint_for_task.clone(),
                                        name: name_for_task.clone(),
                                    },
                                )
                                .await
                                {
                                    let _ = events.send(ChatGossipEvent::System {
                                        text: format!("failed to announce peer: {error}"),
                                    });
                                }
                            }
                            Ok(GossipEvent::NeighborDown(endpoint)) => {
                                let _ = events.send(ChatGossipEvent::NeighborDown {
                                    endpoint: endpoint.to_string(),
                                });
                            }
                            Ok(GossipEvent::Received(message)) => {
                                if let Ok(message) =
                                    serde_json::from_slice::<ChatWireMessage>(&message.content)
                                {
                                    match message {
                                        ChatWireMessage::AboutMe { endpoint, name } => {
                                            let _ = events.send(ChatGossipEvent::System {
                                                text: format!("{name} joined ({endpoint})"),
                                            });
                                        }
                                        ChatWireMessage::Chat {
                                            from_endpoint,
                                            from_name,
                                            text,
                                        } => {
                                            let _ = events.send(ChatGossipEvent::Chat {
                                                from_endpoint,
                                                from_name,
                                                text,
                                            });
                                        }
                                    }
                                }
                            }
                            Ok(GossipEvent::Lagged) => {
                                let _ = events.send(ChatGossipEvent::System {
                                    text: "missed gossip messages".into(),
                                });
                            }
                            Err(error) => {
                                let _ = events.send(ChatGossipEvent::System {
                                    text: format!("gossip receive error: {error}"),
                                });
                                break;
                            }
                        }
                    }
                    let _ = events.send(ChatGossipEvent::System {
                        text: format!("left topic {topic_for_task}"),
                    });
                });
                let _ = self.events_tx.send(ChatGossipEvent::Joined {
                    topic: topic.clone(),
                });
                broadcast_wire(&sender, ChatWireMessage::AboutMe { endpoint, name }).await?;
            }
            ChatGossipCommand::Send {
                topic,
                from_endpoint,
                from_name,
                text,
            } => {
                let sender = self
                    .topics
                    .lock()
                    .expect("chat gossip topic mutex poisoned")
                    .get(&topic)
                    .cloned()
                    .ok_or_else(|| Error::WebRtc(format!("not joined to gossip topic {topic}")))?;
                broadcast_wire(
                    &sender,
                    ChatWireMessage::Chat {
                        from_endpoint,
                        from_name,
                        text,
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn next_event(&self) -> Result<Option<Self::Event>> {
        Ok(self.events_rx.lock().await.recv().await)
    }
}

impl ChatGossipProtocol {
    fn gossip(&self, endpoint: Endpoint) -> Gossip {
        let mut gossip = self.gossip.lock().expect("chat gossip mutex poisoned");
        if let Some(gossip) = gossip.as_ref() {
            return gossip.clone();
        }
        let spawned = Gossip::builder().spawn(endpoint);
        *gossip = Some(spawned.clone());
        spawned
    }

    fn existing_gossip(&self) -> Result<Gossip> {
        self.gossip
            .lock()
            .expect("chat gossip mutex poisoned")
            .clone()
            .ok_or_else(|| Error::WebRtc("gossip handler has not been registered".into()))
    }
}

async fn broadcast_wire(sender: &GossipSender, message: ChatWireMessage) -> Result<()> {
    let encoded = serde_json::to_vec(&message)
        .map_err(|error| Error::WebRtc(format!("failed to encode chat message: {error}")))?;
    sender
        .broadcast(Bytes::from(encoded))
        .await
        .map_err(error_from_display)
}

fn parse_topic(topic: &str) -> Result<TopicId> {
    TopicId::from_str(topic).map_err(error_from_display)
}

fn parse_peers(peers: &[String]) -> Result<Vec<EndpointId>> {
    peers
        .iter()
        .map(|peer| EndpointId::from_str(peer).map_err(error_from_display))
        .collect()
}

fn error_from_display(error: impl std::fmt::Display) -> Error {
    Error::WebRtc(error.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoomMode {
    Hosting,
    Joined,
}

struct AppState {
    _node: BrowserWebRtcNode,
    gossip: BrowserProtocolHandle<ChatGossipProtocol>,
    topic: String,
    local_endpoint: String,
    mode: Option<RoomMode>,
    local_name: String,
    seed_endpoint: Option<String>,
    topic_joined: bool,
    messages_sent: usize,
    messages_received: usize,
    lagged_events: usize,
    last_gossip_event: String,
    remote_names: Vec<String>,
    document: Document,
    host_input: HtmlInputElement,
    name_input: HtmlInputElement,
    message_input: HtmlTextAreaElement,
    send_button: HtmlButtonElement,
    host_button: HtmlButtonElement,
    join_button: HtmlButtonElement,
    room_endpoint: Element,
    participants: Element,
    gossip_state: Element,
    messages: HtmlElement,
    status: Element,
}

async fn run_app() -> std::result::Result<(), JsValue> {
    console_error_panic_hook::set_once();
    iroh_webrtc_transport::browser::install_browser_console_tracing();

    let document = document()?;
    let local = element::<HtmlInputElement>(&document, "local")?;
    let node = BrowserWebRtcNode::builder(
        BrowserWebRtcNodeConfig::default()
            .with_protocol_transport_preference(BrowserDialTransportPreference::WebRtcOnly),
    )
    .protocol(ChatGossipProtocol::default())
    .map_err(|err| JsValue::from_str(&err))?
    .spawn()
    .await?;
    let gossip = node.protocol::<ChatGossipProtocol>().await?;
    let local_endpoint = node.endpoint_id().to_owned();
    let topic = TopicId::from_bytes(CHAT_TOPIC_BYTES).to_string();
    local.set_value(&local_endpoint);

    let state = Rc::new(RefCell::new(AppState {
        _node: node,
        gossip,
        topic,
        local_endpoint: local_endpoint.clone(),
        mode: None,
        local_name: "Browser peer".to_owned(),
        seed_endpoint: None,
        topic_joined: false,
        messages_sent: 0,
        messages_received: 0,
        lagged_events: 0,
        last_gossip_event: "idle".to_owned(),
        remote_names: Vec::new(),
        document: document.clone(),
        host_input: element(&document, "host")?,
        name_input: element(&document, "name")?,
        message_input: element(&document, "message")?,
        send_button: element(&document, "send")?,
        host_button: element(&document, "host-room")?,
        join_button: element(&document, "join-room")?,
        room_endpoint: element(&document, "room-endpoint")?,
        participants: element(&document, "participants")?,
        gossip_state: element(&document, "gossip-state")?,
        messages: element(&document, "messages")?,
        status: element(&document, "status")?,
    }));

    set_status(&state, &format!("local endpoint: {local_endpoint}"));
    render_participants(&state)?;
    render_gossip_state(&state)?;
    wire_controls(state)?;
    Ok(())
}

fn wire_controls(state: Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let events_state = state.clone();
    spawn_local(async move {
        if let Err(error) = poll_gossip_events(events_state.clone()).await {
            append_system(
                &events_state,
                &format!("event error: {}", js_error_text(error)),
            );
        }
    });

    let host_state = state.clone();
    let on_host = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let state = host_state.clone();
        spawn_local(async move {
            if let Err(error) = host_room(state.clone()).await {
                append_system(&state, &format!("host error: {}", js_error_text(error)));
            }
        });
    }) as Box<dyn FnMut(_)>);
    state
        .borrow()
        .host_button
        .add_event_listener_with_callback("click", on_host.as_ref().unchecked_ref())?;
    on_host.forget();

    let join_state = state.clone();
    let on_join = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let state = join_state.clone();
        spawn_local(async move {
            if let Err(error) = join_room(state.clone()).await {
                append_system(&state, &format!("join error: {}", js_error_text(error)));
            }
        });
    }) as Box<dyn FnMut(_)>);
    state
        .borrow()
        .join_button
        .add_event_listener_with_callback("click", on_join.as_ref().unchecked_ref())?;
    on_join.forget();

    let send_state = state.clone();
    let on_send = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let state = send_state.clone();
        spawn_local(async move {
            if let Err(error) = send_current_message(state.clone()).await {
                append_system(&state, &format!("send error: {}", js_error_text(error)));
            }
        });
    }) as Box<dyn FnMut(_)>);
    state
        .borrow()
        .send_button
        .add_event_listener_with_callback("click", on_send.as_ref().unchecked_ref())?;
    on_send.forget();

    let enter_state = state.clone();
    let on_keydown = Closure::wrap(Box::new(move |event: KeyboardEvent| {
        if event.key() != "Enter" || event.shift_key() {
            return;
        }
        event.prevent_default();
        let state = enter_state.clone();
        spawn_local(async move {
            if let Err(error) = send_current_message(state.clone()).await {
                append_system(&state, &format!("send error: {}", js_error_text(error)));
            }
        });
    }) as Box<dyn FnMut(_)>);
    state
        .borrow()
        .message_input
        .add_event_listener_with_callback("keydown", on_keydown.as_ref().unchecked_ref())?;
    on_keydown.forget();

    Ok(())
}

async fn host_room(state: Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let (gossip, topic, endpoint, name) = {
        let mut state = state.borrow_mut();
        if state.mode.is_some() {
            append_system_from_state(&state, "already connected to a room")?;
            return Ok(());
        }
        state.local_name = display_name(&state.name_input);
        state.mode = Some(RoomMode::Hosting);
        state.seed_endpoint = None;
        state.topic_joined = false;
        state.last_gossip_event = "hosting topic".to_owned();
        state
            .room_endpoint
            .set_text_content(Some(&state.local_endpoint));
        state.host_button.set_disabled(true);
        state.join_button.set_disabled(true);
        set_status_from_state(&state, "hosting; share your local endpoint")?;
        update_send_controls_from_state(&state)?;
        (
            state.gossip.clone(),
            state.topic.clone(),
            state.local_endpoint.clone(),
            state.local_name.clone(),
        )
    };

    gossip
        .send(ChatGossipCommand::Join {
            topic,
            peers: Vec::new(),
            endpoint,
            name: name.clone(),
        })
        .await?;
    append_system(&state, &format!("{name} is hosting"));
    render_participants(&state)?;
    render_gossip_state(&state)?;
    Ok(())
}

async fn join_room(state: Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let (gossip, topic, host, endpoint, name) = {
        let mut state = state.borrow_mut();
        if state.mode.is_some() {
            append_system_from_state(&state, "already connected to a room")?;
            return Ok(());
        }
        let host = state.host_input.value().trim().to_owned();
        if host.is_empty() {
            append_system_from_state(&state, "enter a host endpoint first")?;
            return Ok(());
        }
        if host == state.local_endpoint {
            append_system_from_state(&state, "use Host to join your own room locally")?;
            return Ok(());
        }
        state.local_name = display_name(&state.name_input);
        state.mode = Some(RoomMode::Joined);
        state.seed_endpoint = Some(host.clone());
        state.topic_joined = false;
        state.last_gossip_event = format!("joining through {host}");
        state.room_endpoint.set_text_content(Some(&host));
        state.host_button.set_disabled(true);
        state.join_button.set_disabled(true);
        set_status_from_state(&state, &format!("joining {host}"))?;
        update_send_controls_from_state(&state)?;
        (
            state.gossip.clone(),
            state.topic.clone(),
            host,
            state.local_endpoint.clone(),
            state.local_name.clone(),
        )
    };

    gossip
        .send(ChatGossipCommand::Join {
            topic,
            peers: vec![host.clone()],
            endpoint,
            name: name.clone(),
        })
        .await?;
    append_system(&state, &format!("{name} joined the room"));
    render_participants(&state)?;
    render_gossip_state(&state)?;
    Ok(())
}

async fn send_current_message(state: Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let (gossip, topic, from_endpoint, from_name, text, connected) = {
        let state = state.borrow();
        let text = state.message_input.value().trim().to_owned();
        if text.is_empty() {
            return Ok(());
        }
        (
            state.gossip.clone(),
            state.topic.clone(),
            state.local_endpoint.clone(),
            state.local_name.clone(),
            text,
            state.mode.is_some(),
        )
    };
    state.borrow().message_input.set_value("");
    if !connected {
        append_system(&state, "host or join a room first");
        return Ok(());
    }

    append_chat(&state, &from_endpoint, &from_name, &text);
    gossip
        .send(ChatGossipCommand::Send {
            topic,
            from_endpoint,
            from_name,
            text: text.to_string(),
        })
        .await?;
    {
        let mut state = state.borrow_mut();
        state.messages_sent += 1;
        state.last_gossip_event = "broadcast chat message".to_owned();
    }
    render_gossip_state(&state)?;
    Ok(())
}

async fn poll_gossip_events(state: Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let gossip = state.borrow().gossip.clone();
    loop {
        let Some(event) = gossip.next_event().await? else {
            continue;
        };
        match event {
            ChatGossipEvent::Joined { topic } => {
                {
                    let mut state = state.borrow_mut();
                    state.topic_joined = true;
                    state.last_gossip_event = format!("subscribed to topic {topic}");
                }
                set_status(&state, "joined gossip topic; waiting for gossip neighbor");
                update_send_controls(&state)?;
                render_gossip_state(&state)?;
            }
            ChatGossipEvent::NeighborUp { endpoint } => {
                {
                    let mut state = state.borrow_mut();
                    if !state.remote_names.iter().any(|peer| peer == &endpoint) {
                        state.remote_names.push(endpoint.clone());
                    }
                    state.last_gossip_event = format!("neighbor up {endpoint}");
                }
                append_system(&state, &format!("neighbor connected: {endpoint}"));
                update_send_controls(&state)?;
                render_participants(&state)?;
                render_gossip_state(&state)?;
            }
            ChatGossipEvent::NeighborDown { endpoint } => {
                {
                    let mut state = state.borrow_mut();
                    state
                        .remote_names
                        .retain(|candidate| candidate != &endpoint);
                    state.last_gossip_event = format!("neighbor down {endpoint}");
                }
                append_system(&state, &format!("neighbor disconnected: {endpoint}"));
                update_send_controls(&state)?;
                render_participants(&state)?;
                render_gossip_state(&state)?;
            }
            ChatGossipEvent::Chat {
                from_endpoint,
                from_name,
                text,
            } => {
                if from_endpoint != state.borrow().local_endpoint {
                    {
                        let mut state = state.borrow_mut();
                        state.messages_received += 1;
                        state.last_gossip_event = format!("received chat from {from_endpoint}");
                    }
                    append_chat(&state, &from_endpoint, &from_name, &text);
                    render_gossip_state(&state)?;
                }
            }
            ChatGossipEvent::System { text } => {
                {
                    let mut state = state.borrow_mut();
                    if text == "missed gossip messages" {
                        state.lagged_events += 1;
                    }
                    state.last_gossip_event = text.clone();
                }
                append_system(&state, &text);
                render_gossip_state(&state)?;
            }
        }
    }
}

fn render_participants(state: &Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let state = state.borrow();
    state.participants.set_text_content(None);
    if state.mode.is_some() {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-primary badge-outline");
        item.set_text_content(Some(&state.local_name));
        state.participants.append_child(&item)?;
    }
    for peer in &state.remote_names {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-neutral badge-outline");
        item.set_text_content(Some(peer));
        state.participants.append_child(&item)?;
    }
    Ok(())
}

fn update_send_controls(state: &Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let state = state.borrow();
    update_send_controls_from_state(&state)
}

fn update_send_controls_from_state(state: &AppState) -> std::result::Result<(), JsValue> {
    let can_send = state.topic_joined && !state.remote_names.is_empty();
    state.message_input.set_disabled(!can_send);
    state.send_button.set_disabled(!can_send);

    if can_send {
        set_status_from_state(state, "gossip neighbor connected; messages will broadcast")?;
    } else if state.mode.is_some() && state.topic_joined {
        set_status_from_state(state, "waiting for a gossip neighbor before sending")?;
    }

    Ok(())
}

fn render_gossip_state(state: &Rc<RefCell<AppState>>) -> std::result::Result<(), JsValue> {
    let state = state.borrow();
    state.gossip_state.set_text_content(None);

    let mode = match state.mode {
        Some(RoomMode::Hosting) => "hosting",
        Some(RoomMode::Joined) => "joined",
        None => "idle",
    };
    append_state_row(&state, "Mode", mode)?;
    append_state_row(
        &state,
        "ALPN",
        std::str::from_utf8(GOSSIP_ALPN).unwrap_or("gossip"),
    )?;
    append_state_row(&state, "Topic", &state.topic)?;
    append_state_row(
        &state,
        "Seed",
        state.seed_endpoint.as_deref().unwrap_or("none"),
    )?;
    append_state_row(&state, "Neighbors", &state.remote_names.len().to_string())?;
    append_state_row(&state, "Sent", &state.messages_sent.to_string())?;
    append_state_row(&state, "Received", &state.messages_received.to_string())?;
    append_state_row(&state, "Lagged", &state.lagged_events.to_string())?;
    append_state_row(&state, "Last", &state.last_gossip_event)?;
    Ok(())
}

fn append_state_row(
    state: &AppState,
    label: &str,
    value: &str,
) -> std::result::Result<(), JsValue> {
    let row = state.document.create_element("div")?;
    row.set_class_name("grid grid-cols-[5rem_minmax(0,1fr)] gap-2");

    let label_el = state.document.create_element("span")?;
    label_el.set_class_name("text-xs font-semibold uppercase text-base-content/50");
    label_el.set_text_content(Some(label));

    let value_el = state.document.create_element("span")?;
    value_el.set_class_name("endpoint min-w-0 font-mono text-xs text-base-content/85");
    value_el.set_text_content(Some(value));

    row.append_child(&label_el)?;
    row.append_child(&value_el)?;
    state.gossip_state.append_child(&row)?;
    Ok(())
}

fn append_chat(state: &Rc<RefCell<AppState>>, from_endpoint: &str, from_name: &str, text: &str) {
    let state = state.borrow();
    let Ok(message) = state.document.create_element("div") else {
        return;
    };
    if from_endpoint == state.local_endpoint {
        message.set_class_name("chat chat-end");
    } else {
        message.set_class_name("chat chat-start");
    }
    let Ok(name) = state.document.create_element("strong") else {
        return;
    };
    name.set_class_name("chat-header opacity-70");
    name.set_text_content(Some(from_name));
    let Ok(body) = state.document.create_element("div") else {
        return;
    };
    if from_endpoint == state.local_endpoint {
        body.set_class_name("chat-bubble chat-bubble-primary whitespace-pre-wrap break-words");
    } else {
        body.set_class_name("chat-bubble whitespace-pre-wrap break-words");
    }
    body.set_text_content(Some(text));
    let _ = message.append_child(&name);
    let _ = message.append_child(&body);
    let _ = state.messages.append_child(&message);
    state
        .messages
        .set_scroll_top(state.messages.scroll_height());
}

fn append_system(state: &Rc<RefCell<AppState>>, text: &str) {
    let state = state.borrow();
    let _ = append_system_from_state(&state, text);
}

fn append_system_from_state(state: &AppState, text: &str) -> std::result::Result<(), JsValue> {
    let message = state.document.create_element("div")?;
    message.set_class_name("alert alert-info alert-soft my-2 py-2 text-sm");
    message.set_text_content(Some(text));
    state.messages.append_child(&message)?;
    state
        .messages
        .set_scroll_top(state.messages.scroll_height());
    Ok(())
}

fn set_status(state: &Rc<RefCell<AppState>>, text: &str) {
    let state = state.borrow();
    let _ = set_status_from_state(&state, text);
}

fn set_status_from_state(state: &AppState, text: &str) -> std::result::Result<(), JsValue> {
    state.status.set_text_content(Some(text));
    Ok(())
}

fn display_name(input: &HtmlInputElement) -> String {
    let name = input.value().trim().to_owned();
    if name.is_empty() {
        "Browser peer".to_owned()
    } else {
        name
    }
}

fn document() -> std::result::Result<Document, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("browser document is unavailable"))
}

fn element<T: JsCast>(document: &Document, id: &str) -> std::result::Result<T, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing #{id}")))?
        .dyn_into::<T>()
        .map_err(|_| JsValue::from_str(&format!("#{id} has the wrong element type")))
}

fn js_error_text(error: JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
