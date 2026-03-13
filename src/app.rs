use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Stroke, Vec2};

use crate::messaging::{ChatMessage, Peer};
use crate::p2p;
use crate::tor::TorEvent;

#[derive(Clone, Debug)]
pub enum ConnectionState {
    Disconnected,
    Connecting(String),
    Connected(String),
}

#[derive(Clone, Debug)]
struct Conversation {
    peer_label: String,
    onion: String,
    messages: Vec<ChatMessage>,
}

pub struct MessengerApp {
    onion_address: String,
    tor_status: String,
    connection: ConnectionState,
    friend_address: String,
    friend_alias: String,
    message_input: String,
    chat: Conversation,
    status_feed: Vec<String>,
    tor_rx: Receiver<TorEvent>,
    p2p_rx: Receiver<p2p::P2pEvent>,
    p2p_handle: p2p::Handle,
    #[allow(dead_code)]
    runtime: tokio::runtime::Runtime,
}

impl MessengerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts_and_theme(cc);

        let (tor_tx, tor_rx) = mpsc::channel();
        let (p2p_tx, p2p_rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");

        let handle = runtime.handle().clone();
        let p2p_handle = p2p::spawn(p2p_tx, tor_tx, handle);

        Self {
            onion_address: String::new(),
            tor_status: "Bootstrapping Tor network...".to_owned(),
            connection: ConnectionState::Disconnected,
            friend_address: String::new(),
            friend_alias: "Direct Session".to_owned(),
            message_input: String::new(),
            chat: Conversation {
                peer_label: "Direct Session".into(),
                onion: String::new(),
                messages: vec![ChatMessage::system(
                    "Welcome. Wait for Tor to publish your .onion, then share it with a contact.",
                )],
            },
            status_feed: vec!["App launched".into()],
            tor_rx,
            p2p_rx,
            p2p_handle,
            runtime,
        }
    }

    fn poll_tor_events(&mut self) {
        while let Ok(event) = self.tor_rx.try_recv() {
            match event {
                TorEvent::Status(s) => {
                    self.tor_status = s.clone();
                    self.status_feed.push(format!("Tor: {s}"));
                }
                TorEvent::OnionAddress(addr) => {
                    self.onion_address = addr.clone();
                    self.status_feed
                        .push(format!("Hidden service identity ready: {addr}"));
                }
            }
        }
    }

    fn poll_p2p_events(&mut self) {
        while let Ok(event) = self.p2p_rx.try_recv() {
            match event {
                p2p::P2pEvent::Status(state) => {
                    self.connection = state.clone();
                    self.status_feed.push(match state {
                        ConnectionState::Disconnected => "Disconnected".to_string(),
                        ConnectionState::Connecting(p) => format!("Connecting to {p}"),
                        ConnectionState::Connected(p) => format!("Connected to {p}"),
                    });
                }
                p2p::P2pEvent::PeerConnected(from) => {
                    if self.friend_address.trim().is_empty() {
                        self.friend_address = from.clone();
                    }
                    self.chat.onion = from.clone();
                    if self.friend_alias == "Direct Session" {
                        self.friend_alias = from.clone();
                    }
                    self.chat.peer_label = self.friend_alias.clone();
                    self.connection = ConnectionState::Connected(from.clone());
                    self.status_feed.push(format!("Peer connected: {from}"));
                }
                p2p::P2pEvent::Info(info) => {
                    self.status_feed.push(format!("Transport: {info}"));
                }
                p2p::P2pEvent::Incoming { from, body } => {
                    if self.friend_address.trim().is_empty() {
                        self.friend_address = from.clone();
                    }
                    self.chat.onion = from.clone();
                    self.connection = ConnectionState::Connected(from.clone());
                    self.chat.messages.push(ChatMessage {
                        from: Peer::Remote(from),
                        body,
                        timestamp: crate::messaging::now(),
                    });
                }
            }
        }
    }

    fn send_current_message(&mut self) {
        if self.friend_address.trim().is_empty() {
            self.chat.messages.push(ChatMessage::system(
                "Enter a friend's .onion before sending.",
            ));
            return;
        }
        if self.message_input.trim().is_empty() {
            return;
        }
        let body = self.message_input.trim().to_owned();
        self.chat.messages.push(ChatMessage::local(body.clone()));
        self.p2p_handle
            .send_text(self.friend_address.clone(), body.clone());
        self.message_input.clear();
    }

    fn connection_label(&self) -> String {
        match &self.connection {
            ConnectionState::Disconnected => "Disconnected".to_owned(),
            ConnectionState::Connecting(peer) => format!("Connecting to {}", peer),
            ConnectionState::Connected(peer) => format!("Connected to {}", peer),
        }
    }

    fn active_chat_title(&self) -> String {
        match &self.connection {
            ConnectionState::Connected(peer) => format!("Chat with {}", peer),
            ConnectionState::Connecting(peer) => format!("Connecting to {}", peer),
            ConnectionState::Disconnected => "Chat (offline)".to_owned(),
        }
    }
}

impl eframe::App for MessengerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(100));
        self.poll_tor_events();
        self.poll_p2p_events();

        egui::TopBottomPanel::top("top_bar")
            .frame(egui::Frame::none().fill(Color32::from_rgb(16, 18, 23)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(RichText::new("Session-style Tor Messenger").color(Color32::WHITE));
                    ui.add_space(12.0);
                    ui.label(chip(
                        "Tor",
                        &self.tor_status,
                        Color32::from_rgb(80, 180, 255),
                    ));
                    ui.label(chip(
                        "Session",
                        &self.connection_label(),
                        Color32::from_rgb(120, 230, 180),
                    ));
                });
            });

        egui::SidePanel::left("sidebar")
            .resizable(false)
            .min_width(320.0)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(24, 26, 32))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(35, 40, 50))),
            )
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.heading(RichText::new("Identity").color(Color32::WHITE));
                    ui.label("Your .onion address");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.onion_address)
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                    if ui.button("Copy onion").clicked() {
                        ui.output_mut(|o| o.copied_text = self.onion_address.clone());
                    }
                    ui.separator();

                    ui.heading(RichText::new("Connect").color(Color32::WHITE));
                    ui.label("Friend .onion");
                    ui.text_edit_singleline(&mut self.friend_address);
                    ui.label("Label");
                    ui.text_edit_singleline(&mut self.friend_alias);

                    let connect_clicked = ui
                        .add_sized(
                            Vec2::new(ui.available_width(), 32.0),
                            egui::Button::new("Start Session"),
                        )
                        .clicked();

                    if connect_clicked && !self.friend_address.is_empty() {
                        self.connection = ConnectionState::Connecting(self.friend_address.clone());
                        self.chat.peer_label = self.friend_alias.clone();
                        self.chat.onion = self.friend_address.clone();
                        self.p2p_handle.connect(self.friend_address.clone());
                        self.chat.messages.push(ChatMessage::system(format!(
                            "Attempting connection to {} ({})",
                            self.friend_alias, self.friend_address
                        )));
                    }

                    ui.separator();
                    ui.heading(RichText::new("Status").color(Color32::WHITE));
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for line in self.status_feed.iter().rev() {
                                ui.label(RichText::new(line).color(Color32::from_gray(200)));
                            }
                        });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(14, 16, 22)))
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.heading(RichText::new(self.active_chat_title()).color(Color32::WHITE));
                        ui.add_space(8.0);
                        if !self.chat.onion.is_empty() {
                            ui.label(
                                RichText::new(&self.chat.onion)
                                    .color(Color32::from_rgb(150, 180, 210))
                                    .italics(),
                            );
                        }
                    });
                    ui.add_space(8.0);

                    let composer_height = 44.0;
                    let spacing = ui.spacing().item_spacing.y * 2.0 + 6.0;
                    let chat_height = (ui.available_height() - composer_height - spacing).max(140.0);

                    egui::ScrollArea::vertical()
                        .id_source("chat_scroll")
                        .auto_shrink([false; 2])
                        .stick_to_bottom(true)
                        .max_height(chat_height)
                        .show(ui, |ui| {
                            for message in &self.chat.messages {
                                render_message(ui, message);
                                ui.add_space(6.0);
                            }
                        });

                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let input_width = (ui.available_width() - 90.0).max(120.0);
                        let send_resp = ui.add_sized(
                            Vec2::new(input_width, 36.0),
                            egui::TextEdit::singleline(&mut self.message_input)
                                .hint_text("Type a message"),
                        );

                        let pressed_enter =
                            send_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                        if ui
                            .add_sized(Vec2::new(80.0, 36.0), egui::Button::new("Send"))
                            .clicked()
                            || pressed_enter
                        {
                            self.send_current_message();
                        }
                    });
                });
            });
    }
}

fn chip(title: &str, text: &str, color: Color32) -> RichText {
    RichText::new(format!("{title}: {text}"))
        .color(color)
        .strong()
}

fn render_message(ui: &mut egui::Ui, message: &ChatMessage) {
    let (align, bubble_color, text_color) = match message.from {
        Peer::Local => (
            egui::Align2::RIGHT_TOP,
            Color32::from_rgb(60, 120, 220),
            Color32::WHITE,
        ),
        Peer::Remote(_) => (
            egui::Align2::LEFT_TOP,
            Color32::from_rgb(40, 45, 55),
            Color32::from_rgb(230, 235, 240),
        ),
        Peer::System => (
            egui::Align2::CENTER_TOP,
            Color32::from_rgb(30, 35, 40),
            Color32::from_rgb(200, 200, 200),
        ),
    };

    let valign = egui::Align::Min;
    let main_align = match align {
        egui::Align2::LEFT_TOP => egui::Align::Min,
        egui::Align2::RIGHT_TOP => egui::Align::Max,
        _ => egui::Align::Center,
    };

    ui.with_layout(
        egui::Layout::left_to_right(valign).with_main_align(main_align),
        |ui| {
            egui::Frame::none()
                .fill(bubble_color)
                .stroke(Stroke::new(1.0, bubble_color))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .show(ui, |ui| {
                    ui.label(RichText::new(&message.body).color(text_color));
                });
        },
    );
}

fn install_fonts_and_theme(cc: &eframe::CreationContext<'_>) {
    // Typography: use default fonts but tighten spacing for a denser chat layout.
    let fonts = egui::FontDefinitions::default();
    cc.egui_ctx.set_fonts(fonts);

    let mut style = (*cc.egui_ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.text_styles.insert(
        egui::TextStyle::Body,
        FontId::new(15.0, FontFamily::Proportional),
    );
    cc.egui_ctx.set_style(style);
}
