use std::io::ErrorKind;
use std::sync::{mpsc::Sender, Arc};

use anyhow::{anyhow, Context};
use arti_client::{
    config::{onion_service::OnionServiceConfigBuilder, BoolOrAuto, TorClientConfig},
    BootstrapBehavior, StreamPrefs, TorClient,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tor_cell::relaycell::msg::{Connected, End};
use tor_hsservice::{handle_rend_requests, HsNickname, StreamRequest};
use tor_proto::stream::IncomingStreamRequest;
use tor_rtcompat::Runtime;

use crate::tor::TorEvent;
use crate::video;

#[derive(Clone, Debug)]
pub enum P2pEvent {
    Status(super::app::ConnectionState),
    PeerConnected(String),
    Info(String),
    Incoming { from: String, body: String },
    VideoFrame { from: String, png: Vec<u8> },
    VideoState { active: bool, label: String },
}

#[derive(Clone)]
pub struct Handle {
    tx: tokio::sync::mpsc::UnboundedSender<P2pCommand>,
}

#[derive(Debug)]
enum P2pCommand {
    Connect(String),
    Send { to: String, body: String },
    StartVideo(String),
    StopVideo,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WireMessage {
    from: String,
    payload: WirePayload,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
enum WirePayload {
    Handshake,
    Text(String),
    VideoFrame { png: Vec<u8> },
}

const HANDSHAKE_BODY: &str = "__antifa_handshake__";
const SERVICE_NICKNAME: &str = "antifa-messenger";
const SERVICE_PORT: u16 = 17600;
const MAX_WIRE_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

impl Handle {
    pub fn connect(&self, peer: String) {
        let _ = self.tx.send(P2pCommand::Connect(peer));
    }

    pub fn send_text(&self, to: String, body: String) {
        let _ = self.tx.send(P2pCommand::Send { to, body });
    }

    pub fn start_video(&self, to: String) {
        let _ = self.tx.send(P2pCommand::StartVideo(to));
    }

    pub fn stop_video(&self) {
        let _ = self.tx.send(P2pCommand::StopVideo);
    }
}

pub fn spawn(
    event_tx: Sender<P2pEvent>,
    tor_tx: Sender<TorEvent>,
    handle: tokio::runtime::Handle,
) -> Handle {
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<P2pCommand>();

    handle.spawn(async move {
        if let Err(err) = run_transport(event_tx.clone(), tor_tx.clone(), &mut cmd_rx).await {
            let _ = tor_tx.send(TorEvent::Status(format!("Tor failed: {err:#}")));
            let _ = event_tx.send(P2pEvent::Info(format!("Transport error: {err:#}")));
            let _ = event_tx.send(P2pEvent::VideoState {
                active: false,
                label: "Video unavailable".to_owned(),
            });
            let _ = event_tx.send(P2pEvent::Status(super::app::ConnectionState::Disconnected));
        }
    });

    Handle { tx: cmd_tx }
}

async fn run_transport(
    event_tx: Sender<P2pEvent>,
    tor_tx: Sender<TorEvent>,
    cmd_rx: &mut tokio::sync::mpsc::UnboundedReceiver<P2pCommand>,
) -> anyhow::Result<()> {
    let tor_client = Arc::new(
        TorClient::builder()
            .config(TorClientConfig::default())
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped_async()
            .await
            .context("create Tor client")?,
    );

    let mut bootstrap_events = tor_client.bootstrap_events();
    let bootstrap_tor_tx = tor_tx.clone();
    tokio::spawn(async move {
        let mut last_status = None::<String>;
        while let Some(status) = bootstrap_events.next().await {
            let label = if status.ready_for_traffic() {
                "Tor network ready".to_owned()
            } else {
                format!("Bootstrapping Tor: {status}")
            };

            if last_status.as_deref() != Some(label.as_str()) {
                let _ = bootstrap_tor_tx.send(TorEvent::Status(label.clone()));
                last_status = Some(label);
            }
        }
    });

    let _ = tor_tx.send(TorEvent::Status("Bootstrapping Tor network...".to_owned()));
    tor_client
        .bootstrap()
        .await
        .context("bootstrap Tor client")?;

    let nickname: HsNickname = SERVICE_NICKNAME
        .to_owned()
        .try_into()
        .map_err(|_| anyhow!("invalid hidden-service nickname"))?;
    let service_config = OnionServiceConfigBuilder::default()
        .nickname(nickname)
        .build()
        .context("build onion service config")?;

    let (service, rend_requests) = tor_client
        .launch_onion_service(service_config)
        .context("launch onion service")?;

    let local_onion = service
        .onion_name()
        .map(|hsid| hsid.to_string())
        .ok_or_else(|| anyhow!("hidden service did not expose an onion address"))?;

    let _ = tor_tx.send(TorEvent::OnionAddress(local_onion.clone()));
    let _ = tor_tx.send(TorEvent::Status("Publishing onion service...".to_owned()));
    let _ = event_tx.send(P2pEvent::Info(format!(
        "Hidden service configured on {local_onion} (port {SERVICE_PORT})"
    )));
    let _ = event_tx.send(P2pEvent::VideoState {
        active: false,
        label: "Video idle".to_owned(),
    });

    let mut service_events = service.status_events();
    let service_tor_tx = tor_tx.clone();
    tokio::spawn(async move {
        let mut last_status = None::<String>;
        while let Some(status) = service_events.next().await {
            let label = format_onion_service_status(&status);
            if last_status.as_deref() != Some(label.as_str()) {
                let _ = service_tor_tx.send(TorEvent::Status(label.clone()));
                last_status = Some(label);
            }
        }
    });

    let incoming_event_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut stream_requests = Box::pin(handle_rend_requests(rend_requests));
        while let Some(stream_request) = stream_requests.next().await {
            let incoming_event_tx = incoming_event_tx.clone();
            tokio::spawn(async move {
                if let Err(err) =
                    handle_stream_request(stream_request, incoming_event_tx.clone()).await
                {
                    let _ = incoming_event_tx
                        .send(P2pEvent::Info(format!("Incoming connection failed: {err:#}")));
                }
            });
        }
    });

    let _ = event_tx.send(P2pEvent::Status(super::app::ConnectionState::Disconnected));

    let _service = service;
    let mut video_task: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            P2pCommand::Connect(peer) => {
                let peer = normalize_onion(&peer);
                if peer.is_empty() {
                    let _ = event_tx.send(P2pEvent::Info(
                        "Enter a peer .onion address before connecting.".to_owned(),
                    ));
                    continue;
                }

                let _ = event_tx.send(P2pEvent::Status(super::app::ConnectionState::Connecting(
                    peer.clone(),
                )));

                match send_payload(
                    tor_client.as_ref(),
                    &peer,
                    &local_onion,
                    WirePayload::Handshake,
                )
                .await
                {
                    Ok(()) => {
                        let _ = event_tx.send(P2pEvent::Status(
                            super::app::ConnectionState::Connected(peer),
                        ));
                    }
                    Err(err) => {
                        let _ = event_tx.send(P2pEvent::Info(format!(
                            "Failed to connect to {peer}: {err:#}"
                        )));
                        let _ = event_tx
                            .send(P2pEvent::Status(super::app::ConnectionState::Disconnected));
                    }
                }
            }
            P2pCommand::Send { to, body } => {
                let peer = normalize_onion(&to);
                if peer.is_empty() {
                    let _ = event_tx.send(P2pEvent::Info(
                        "Enter a peer .onion address before sending.".to_owned(),
                    ));
                    continue;
                }

                match send_payload(
                    tor_client.as_ref(),
                    &peer,
                    &local_onion,
                    WirePayload::Text(body),
                )
                .await
                {
                    Ok(()) => {
                        let _ = event_tx.send(P2pEvent::Status(
                            super::app::ConnectionState::Connected(peer),
                        ));
                    }
                    Err(err) => {
                        let _ = event_tx.send(P2pEvent::Info(format!(
                            "Failed to send to {peer}: {err:#}"
                        )));
                    }
                }
            }
            P2pCommand::StartVideo(peer) => {
                let peer = normalize_onion(&peer);
                if peer.is_empty() {
                    let _ = event_tx.send(P2pEvent::Info(
                        "Enter a peer .onion address before starting video.".to_owned(),
                    ));
                    continue;
                }

                if let Some(task) = video_task.take() {
                    task.abort();
                }

                let _ = event_tx.send(P2pEvent::VideoState {
                    active: true,
                    label: format!("Starting experimental video stream to {peer}"),
                });

                let video_client = tor_client.clone();
                let video_peer = peer.clone();
                let video_from = local_onion.clone();
                let video_events = event_tx.clone();
                video_task = Some(tokio::spawn(async move {
                    if let Err(err) =
                        run_video_sender(video_client, video_peer.clone(), video_from, video_events.clone())
                            .await
                    {
                        let _ = video_events.send(P2pEvent::Info(format!(
                            "Video stream error for {video_peer}: {err:#}"
                        )));
                    }
                    let _ = video_events.send(P2pEvent::VideoState {
                        active: false,
                        label: "Video idle".to_owned(),
                    });
                }));
            }
            P2pCommand::StopVideo => {
                if let Some(task) = video_task.take() {
                    task.abort();
                }
                let _ = event_tx.send(P2pEvent::VideoState {
                    active: false,
                    label: "Video stopped".to_owned(),
                });
            }
        }
    }

    if let Some(task) = video_task.take() {
        task.abort();
    }
    let _ = tor_tx.send(TorEvent::Status("Onion service stopped".to_owned()));
    Ok(())
}

async fn handle_stream_request(
    stream_request: StreamRequest,
    event_tx: Sender<P2pEvent>,
) -> anyhow::Result<()> {
    let is_supported_port = matches!(
        stream_request.request(),
        IncomingStreamRequest::Begin(begin) if begin.port() == SERVICE_PORT
    );

    if !is_supported_port {
        let _ = stream_request.reject(End::new_misc()).await;
        return Ok(());
    }

    let mut stream = stream_request
        .accept(Connected::new_empty())
        .await
        .context("accept incoming stream")?;

    loop {
        let Some(message) = read_wire_message(&mut stream).await? else {
            return Ok(());
        };

        let from = normalize_onion(&message.from);
        match message.payload {
            WirePayload::Handshake => {
                let _ = event_tx.send(P2pEvent::PeerConnected(from));
            }
            WirePayload::Text(body) => {
                let body = if body == HANDSHAKE_BODY {
                    String::new()
                } else {
                    body
                };

                if body.is_empty() {
                    let _ = event_tx.send(P2pEvent::PeerConnected(from));
                } else {
                    let _ = event_tx.send(P2pEvent::Incoming { from, body });
                }
            }
            WirePayload::VideoFrame { png } => {
                let _ = event_tx.send(P2pEvent::VideoFrame { from, png });
            }
        }
    }
}

async fn run_video_sender<R: Runtime>(
    tor_client: Arc<TorClient<R>>,
    target_onion: String,
    from_onion: String,
    event_tx: Sender<P2pEvent>,
) -> anyhow::Result<()> {
    let mut prefs = StreamPrefs::new();
    prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));

    let mut stream = tor_client
        .connect_with_prefs((target_onion.as_str(), SERVICE_PORT), &prefs)
        .await
        .with_context(|| format!("connect to {target_onion}:{SERVICE_PORT} for video over Tor"))?;

    let _ = event_tx.send(P2pEvent::Info(format!(
        "Video stream open to {target_onion}; expect high latency over Tor"
    )));

    let mut child = video::spawn_capture_process()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg capture process did not provide stdout"))?;

    while let Some(png) = video::read_png_frame(&mut stdout).await? {
        let message = WireMessage {
            from: normalize_onion(&from_onion),
            payload: WirePayload::VideoFrame { png },
        };
        write_wire_message(&mut stream, &message).await?;
        stream.flush().await.context("flush Tor video stream")?;
    }

    let status = child.wait().await.context("wait for ffmpeg capture process")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("ffmpeg capture process exited with status {status}"))
    }
}

async fn send_payload<R: Runtime>(
    tor_client: &TorClient<R>,
    target_onion: &str,
    from_onion: &str,
    payload: WirePayload,
) -> anyhow::Result<()> {
    let mut prefs = StreamPrefs::new();
    prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));

    let mut stream = tor_client
        .connect_with_prefs((target_onion, SERVICE_PORT), &prefs)
        .await
        .with_context(|| format!("connect to {target_onion}:{SERVICE_PORT} over Tor"))?;

    let message = WireMessage {
        from: normalize_onion(from_onion),
        payload,
    };
    write_wire_message(&mut stream, &message).await?;
    stream.flush().await.context("flush Tor stream")?;
    stream.shutdown().await.context("shutdown Tor stream")?;
    Ok(())
}

async fn write_wire_message<W>(stream: &mut W, message: &WireMessage) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let payload = bincode::serialize(message).context("serialize message")?;
    let len = u32::try_from(payload.len()).context("message too large")?;

    stream
        .write_all(&len.to_be_bytes())
        .await
        .context("write message length")?;
    stream
        .write_all(&payload)
        .await
        .context("write message payload")?;
    Ok(())
}

async fn read_wire_message<R>(stream: &mut R) -> anyhow::Result<Option<WireMessage>>
where
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err).context("read message length"),
    }

    let payload_len = u32::from_be_bytes(len_buf) as usize;
    if payload_len > MAX_WIRE_PAYLOAD_BYTES {
        return Err(anyhow!("wire payload exceeded {MAX_WIRE_PAYLOAD_BYTES} bytes"));
    }

    let mut payload = vec![0u8; payload_len];
    stream
        .read_exact(&mut payload)
        .await
        .context("read message payload")?;

    let message = bincode::deserialize(&payload).context("decode message payload")?;
    Ok(Some(message))
}

fn normalize_onion(value: &str) -> String {
    let mut normalized = value.trim().trim_end_matches('/').to_lowercase();
    while normalized.ends_with(".onion") {
        normalized.truncate(normalized.len() - ".onion".len());
    }

    if normalized.is_empty() {
        String::new()
    } else {
        format!("{normalized}.onion")
    }
}

fn format_onion_service_status(status: &tor_hsservice::status::OnionServiceStatus) -> String {
    use tor_hsservice::status::State;

    match status.state() {
        State::Shutdown => "Onion service stopped".to_owned(),
        State::Bootstrapping => "Publishing onion service...".to_owned(),
        State::Degraded => "Onion service reachable with limited capacity".to_owned(),
        State::Running => "Onion service reachable".to_owned(),
        State::Recovering => "Onion service recovering".to_owned(),
        State::Broken => match status.current_problem() {
            Some(problem) => format!("Onion service failed: {problem:?}"),
            None => "Onion service failed".to_owned(),
        },
        _ => "Onion service state changed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;
    use tokio::io::duplex;

    use super::{normalize_onion, read_wire_message, write_wire_message, WireMessage, WirePayload};

    #[test]
    fn normalizes_onion_addresses() {
        assert_eq!(normalize_onion("Example.Onion"), "example.onion");
        assert_eq!(normalize_onion("example.onion/"), "example.onion");
        assert_eq!(normalize_onion(""), "");
    }

    #[tokio::test]
    async fn wire_messages_round_trip() -> anyhow::Result<()> {
        let message = WireMessage {
            from: "alice.onion".to_owned(),
            payload: WirePayload::Text("hello".to_owned()),
        };
        let (mut writer, mut reader) = duplex(256);

        let expected = WireMessage {
            from: message.from.clone(),
            payload: WirePayload::Text("hello".to_owned()),
        };
        let writer_task = tokio::spawn(async move {
            write_wire_message(&mut writer, &expected).await?;
            writer.shutdown().await?;
            Ok::<(), anyhow::Error>(())
        });

        let actual = read_wire_message(&mut reader).await?;
        writer_task.await??;

        assert_eq!(actual, Some(message));
        Ok(())
    }
}
