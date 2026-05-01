use std::{cell::RefCell, rc::Rc};

use iroh_webrtc_transport::browser::{
    BrowserDialOptions, BrowserWebRtcConnection, BrowserWebRtcNode, BrowserWebRtcNodeConfig,
    BrowserWebRtcStream,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Document, Element, HtmlButtonElement, HtmlElement, HtmlInputElement, HtmlTextAreaElement,
    KeyboardEvent,
};

const ALPN: &[u8] = b"example/iroh-webrtc-rust-browser-chat-room/1";
const MAX_FRAME_BYTES: usize = 256 * 1024;

iroh_webrtc_transport::browser_app! {
    app = start_app => run_app;
    worker = start_iroh_webrtc_worker;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ChatFrame {
    Join {
        name: String,
    },
    Welcome {
        host_name: String,
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

#[derive(Clone)]
struct Peer {
    endpoint_id: String,
    name: String,
    stream: BrowserWebRtcStream,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RoomMode {
    Hosting,
    Joined,
}

struct AppState {
    node: BrowserWebRtcNode,
    local_endpoint: String,
    mode: Option<RoomMode>,
    local_name: String,
    peers: Vec<Peer>,
    remote_names: Vec<String>,
    host_stream: Option<BrowserWebRtcStream>,
    document: Document,
    host_input: HtmlInputElement,
    name_input: HtmlInputElement,
    message_input: HtmlTextAreaElement,
    send_button: HtmlButtonElement,
    host_button: HtmlButtonElement,
    join_button: HtmlButtonElement,
    room_endpoint: Element,
    participants: Element,
    messages: HtmlElement,
    status: Element,
}

async fn run_app() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    iroh_webrtc_transport::browser::install_browser_console_tracing();

    let document = document()?;
    let local = element::<HtmlInputElement>(&document, "local")?;
    let node = BrowserWebRtcNode::spawn(BrowserWebRtcNodeConfig::default()).await?;
    let local_endpoint = node.endpoint_id().to_owned();
    local.set_value(&local_endpoint);

    let state = Rc::new(RefCell::new(AppState {
        node,
        local_endpoint: local_endpoint.clone(),
        mode: None,
        local_name: "Browser peer".to_owned(),
        peers: Vec::new(),
        remote_names: Vec::new(),
        host_stream: None,
        document: document.clone(),
        host_input: element(&document, "host")?,
        name_input: element(&document, "name")?,
        message_input: element(&document, "message")?,
        send_button: element(&document, "send")?,
        host_button: element(&document, "host-room")?,
        join_button: element(&document, "join-room")?,
        room_endpoint: element(&document, "room-endpoint")?,
        participants: element(&document, "participants")?,
        messages: element(&document, "messages")?,
        status: element(&document, "status")?,
    }));

    set_status(&state, &format!("local endpoint: {local_endpoint}"));
    render_participants(&state)?;
    wire_controls(state)?;
    Ok(())
}

fn wire_controls(state: Rc<RefCell<AppState>>) -> Result<(), JsValue> {
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

async fn host_room(state: Rc<RefCell<AppState>>) -> Result<(), JsValue> {
    let (node, endpoint, name) = {
        let mut state = state.borrow_mut();
        if state.mode.is_some() {
            append_system_from_state(&state, "already connected to a room")?;
            return Ok(());
        }
        state.local_name = display_name(&state.name_input);
        state.mode = Some(RoomMode::Hosting);
        state
            .room_endpoint
            .set_text_content(Some(&state.local_endpoint));
        state.message_input.set_disabled(false);
        state.send_button.set_disabled(false);
        state.host_button.set_disabled(true);
        state.join_button.set_disabled(true);
        (
            state.node.clone(),
            state.local_endpoint.clone(),
            state.local_name.clone(),
        )
    };

    append_system(&state, &format!("{name} is hosting"));
    render_participants(&state)?;
    set_status(&state, "hosting; share your local endpoint");

    let acceptor = node.accept(ALPN).await?;
    let accept_state = state.clone();
    spawn_local(async move {
        loop {
            let connection = match acceptor.accept().await {
                Ok(Some(connection)) => connection,
                Ok(None) => return,
                Err(error) => {
                    append_system(
                        &accept_state,
                        &format!("accept error: {}", js_error_text(error)),
                    );
                    return;
                }
            };
            let state = accept_state.clone();
            spawn_local(async move {
                if let Err(error) = handle_host_connection(state.clone(), connection).await {
                    append_system(&state, &format!("peer error: {}", js_error_text(error)));
                }
            });
        }
    });

    append_system(&state, &format!("room endpoint: {endpoint}"));
    Ok(())
}

async fn join_room(state: Rc<RefCell<AppState>>) -> Result<(), JsValue> {
    let (node, host, name) = {
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
        state.host_button.set_disabled(true);
        state.join_button.set_disabled(true);
        set_status_from_state(&state, &format!("joining {host}"))?;
        (state.node.clone(), host, state.local_name.clone())
    };

    let connection = node
        .dial(&host, ALPN, BrowserDialOptions::webrtc_only())
        .await?;
    let stream = connection.open_bi().await?;
    send_frame(&stream, &ChatFrame::Join { name: name.clone() }).await?;

    {
        let mut state = state.borrow_mut();
        state.mode = Some(RoomMode::Joined);
        state.host_stream = Some(stream.clone());
        state.room_endpoint.set_text_content(Some(&host));
        state.message_input.set_disabled(false);
        state.send_button.set_disabled(false);
        set_status_from_state(&state, "joined")?;
    }
    append_system(&state, &format!("{name} joined the room"));
    render_participants(&state)?;

    spawn_local(read_host_messages(state, stream, connection));
    Ok(())
}

async fn handle_host_connection(
    state: Rc<RefCell<AppState>>,
    connection: BrowserWebRtcConnection,
) -> Result<(), JsValue> {
    let stream = connection.accept_bi().await?;
    let mut reader = FrameReader::new(stream.clone());
    let Some(ChatFrame::Join { name }) = reader.read_frame().await? else {
        return Err(JsValue::from_str("peer did not send a join frame"));
    };
    let remote = connection.remote_endpoint_id().to_owned();
    {
        state.borrow_mut().peers.push(Peer {
            endpoint_id: remote.clone(),
            name: name.clone(),
            stream: stream.clone(),
        });
    }
    let host_name = state.borrow().local_name.clone();
    send_frame(&stream, &ChatFrame::Welcome { host_name }).await?;
    append_system(&state, &format!("{name} joined"));
    render_participants(&state)?;
    broadcast_to_clients_except(
        &state,
        &ChatFrame::System {
            text: format!("{name} joined"),
        },
        &remote,
    )
    .await;

    while let Some(frame) = reader.read_frame().await? {
        if let ChatFrame::Chat {
            from_endpoint: _,
            from_name: _,
            text,
        } = frame
        {
            let frame = ChatFrame::Chat {
                from_endpoint: remote.clone(),
                from_name: name.clone(),
                text,
            };
            if let ChatFrame::Chat {
                from_endpoint,
                from_name,
                text,
            } = &frame
            {
                append_chat(&state, from_endpoint, from_name, text);
            }
            broadcast_to_clients(&state, &frame).await;
        }
    }

    remove_peer(&state, &remote)?;
    append_system(&state, &format!("{name} left"));
    broadcast_to_clients_except(
        &state,
        &ChatFrame::System {
            text: format!("{name} left"),
        },
        &remote,
    )
    .await;
    Ok(())
}

async fn read_host_messages(
    state: Rc<RefCell<AppState>>,
    stream: BrowserWebRtcStream,
    connection: BrowserWebRtcConnection,
) {
    let mut reader = FrameReader::new(stream);
    loop {
        match reader.read_frame().await {
            Ok(Some(ChatFrame::Welcome { host_name })) => {
                {
                    let mut state = state.borrow_mut();
                    state.remote_names.clear();
                    state.remote_names.push(format!("{host_name} (host)"));
                }
                let _ = render_participants(&state);
            }
            Ok(Some(ChatFrame::Chat {
                from_endpoint,
                from_name,
                text,
            })) => append_chat(&state, &from_endpoint, &from_name, &text),
            Ok(Some(ChatFrame::System { text })) => append_system(&state, &text),
            Ok(Some(ChatFrame::Join { .. })) => {}
            Ok(None) => {
                append_system(&state, "host closed the room");
                break;
            }
            Err(error) => {
                append_system(&state, &format!("read error: {}", js_error_text(error)));
                break;
            }
        }
    }
    let _ = connection.close("chat reader stopped").await;
}

async fn send_current_message(state: Rc<RefCell<AppState>>) -> Result<(), JsValue> {
    let (mode, from_endpoint, from_name, text, host_stream, peers) = {
        let state = state.borrow();
        let text = state.message_input.value().trim().to_owned();
        if text.is_empty() {
            return Ok(());
        }
        (
            state.mode,
            state.local_endpoint.clone(),
            state.local_name.clone(),
            text,
            state.host_stream.clone(),
            state.peers.clone(),
        )
    };
    state.borrow().message_input.set_value("");

    match mode {
        Some(RoomMode::Hosting) => {
            append_chat(&state, &from_endpoint, &from_name, &text);
            let frame = ChatFrame::Chat {
                from_endpoint,
                from_name,
                text,
            };
            for peer in peers {
                if let Err(error) = send_frame(&peer.stream, &frame).await {
                    append_system(
                        &state,
                        &format!("failed to send to {}: {}", peer.name, js_error_text(error)),
                    );
                }
            }
        }
        Some(RoomMode::Joined) => {
            let Some(stream) = host_stream else {
                return Err(JsValue::from_str("missing host stream"));
            };
            send_frame(
                &stream,
                &ChatFrame::Chat {
                    from_endpoint,
                    from_name,
                    text,
                },
            )
            .await?;
        }
        None => append_system(&state, "host or join a room first"),
    }

    Ok(())
}

async fn broadcast_to_clients(state: &Rc<RefCell<AppState>>, frame: &ChatFrame) {
    broadcast_to_clients_except(state, frame, "").await;
}

async fn broadcast_to_clients_except(
    state: &Rc<RefCell<AppState>>,
    frame: &ChatFrame,
    excluded_endpoint: &str,
) {
    let peers = state.borrow().peers.clone();
    for peer in peers {
        if peer.endpoint_id == excluded_endpoint {
            continue;
        }
        if let Err(error) = send_frame(&peer.stream, frame).await {
            append_system(
                state,
                &format!("failed to send to {}: {}", peer.name, js_error_text(error)),
            );
        }
    }
}

async fn send_frame(stream: &BrowserWebRtcStream, frame: &ChatFrame) -> Result<(), JsValue> {
    let payload = serde_json::to_vec(frame)
        .map_err(|error| JsValue::from_str(&format!("failed to encode chat frame: {error}")))?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(JsValue::from_str("chat frame is too large"));
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    stream.send_all(&out).await
}

struct FrameReader {
    stream: BrowserWebRtcStream,
    buffer: Vec<u8>,
    eof: bool,
}

impl FrameReader {
    fn new(stream: BrowserWebRtcStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            eof: false,
        }
    }

    async fn read_frame(&mut self) -> Result<Option<ChatFrame>, JsValue> {
        if !self.fill_until(4).await? {
            if self.buffer.is_empty() {
                return Ok(None);
            }
            return Err(JsValue::from_str("stream ended in a frame header"));
        }
        let len = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(JsValue::from_str("chat frame exceeds the demo limit"));
        }
        if !self.fill_until(4 + len).await? {
            return Err(JsValue::from_str("stream ended in a frame body"));
        }
        self.buffer.drain(..4);
        let payload: Vec<u8> = self.buffer.drain(..len).collect();
        serde_json::from_slice(&payload)
            .map(Some)
            .map_err(|error| JsValue::from_str(&format!("failed to decode chat frame: {error}")))
    }

    async fn fill_until(&mut self, len: usize) -> Result<bool, JsValue> {
        while self.buffer.len() < len {
            if self.eof {
                return Ok(false);
            }
            match self.stream.read_chunk().await? {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => self.eof = true,
            }
        }
        Ok(true)
    }
}

fn remove_peer(state: &Rc<RefCell<AppState>>, endpoint_id: &str) -> Result<(), JsValue> {
    state
        .borrow_mut()
        .peers
        .retain(|peer| peer.endpoint_id != endpoint_id);
    render_participants(state)
}

fn render_participants(state: &Rc<RefCell<AppState>>) -> Result<(), JsValue> {
    let state = state.borrow();
    state.participants.set_text_content(None);
    if let Some(RoomMode::Hosting) = state.mode {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-primary badge-outline");
        item.set_text_content(Some(&format!("{} (host)", state.local_name)));
        state.participants.append_child(&item)?;
    } else if let Some(RoomMode::Joined) = state.mode {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-primary badge-outline");
        item.set_text_content(Some(&state.local_name));
        state.participants.append_child(&item)?;
    }
    for name in &state.remote_names {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-neutral badge-outline");
        item.set_text_content(Some(name));
        state.participants.append_child(&item)?;
    }
    for peer in &state.peers {
        let item = state.document.create_element("li")?;
        item.set_class_name("badge badge-neutral badge-outline");
        item.set_text_content(Some(&peer.name));
        state.participants.append_child(&item)?;
    }
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

fn append_system_from_state(state: &AppState, text: &str) -> Result<(), JsValue> {
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

fn set_status_from_state(state: &AppState, text: &str) -> Result<(), JsValue> {
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

fn document() -> Result<Document, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("browser document is unavailable"))
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing #{id}")))?
        .dyn_into::<T>()
        .map_err(|_| JsValue::from_str(&format!("#{id} has the wrong element type")))
}

fn js_error_text(error: JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
