use crate::app_event::{ClientEvent, ClientEventSink};
use crate::live_control::payload::{stream_sequence_base, video_control_value};
use crate::live_control::DesktopStreamState;
use crate::outbound::{queue_message, ClientOutbound};
use rdl_protocol::{audio_udp, now_epoch_ms, CommandKind, Message};
use std::io;
use std::net::UdpSocket;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender},
    Arc,
};
use std::time::{Duration, Instant};

const AUDIO_CAPTURE_FRAME_MS: u32 = 10;
const AUDIO_CAPTURE_RECV_TIMEOUT_MS: u64 = 20;
const AUDIO_STREAM_REPORT_INTERVAL_MS: u64 = 1_000;
const AUDIO_UDP_REGISTER_INTERVAL_MS: u64 = 250;
const AUDIO_UDP_MAX_PAYLOAD_BYTES: usize = 1_200;

pub(crate) const AUDIO_STREAM_STOP_SETTLE_MS: u64 = 180;
pub(crate) const AUDIO_UDP_RECV_TIMEOUT_MS: u64 = 20;

#[derive(Default)]
struct AudioFramePacketizer {
    sample_rate: u32,
    channels: u16,
    format: String,
    frame_bytes: usize,
    pending: Vec<u8>,
}

impl AudioFramePacketizer {
    fn clear_pending(&mut self) {
        self.pending.clear();
    }

    fn push(
        &mut self,
        frame: crate::live_control::CapturedAudioFrame,
    ) -> Vec<crate::live_control::CapturedAudioFrame> {
        if frame.bytes.is_empty() {
            return Vec::new();
        }
        if self.sample_rate != frame.sample_rate
            || self.channels != frame.channels
            || self.format != frame.format
        {
            self.sample_rate = frame.sample_rate;
            self.channels = frame.channels;
            self.format = frame.format.clone();
            self.frame_bytes = audio_capture_frame_bytes(frame.sample_rate, frame.channels);
            self.pending.clear();
        }
        self.pending.extend(frame.bytes);

        let mut frames = Vec::new();
        while self.pending.len() >= self.frame_bytes {
            let bytes: Vec<u8> = self.pending.drain(..self.frame_bytes).collect();
            frames.push(crate::live_control::CapturedAudioFrame {
                sample_rate: self.sample_rate,
                channels: self.channels,
                format: self.format.clone(),
                bytes,
            });
        }
        frames
    }
}

fn audio_capture_frame_bytes(sample_rate: u32, channels: u16) -> usize {
    let samples_per_channel =
        ((sample_rate.max(1) as u64 * AUDIO_CAPTURE_FRAME_MS as u64) / 1000).max(1) as usize;
    let target_bytes = samples_per_channel * channels.max(1) as usize * 2;
    target_bytes.min(max_pcm_s16le_udp_payload_bytes(channels))
}

fn max_pcm_s16le_udp_payload_bytes(channels: u16) -> usize {
    let sample_frame_bytes = channels.max(1) as usize * 2;
    (AUDIO_UDP_MAX_PAYLOAD_BYTES / sample_frame_bytes).max(1) * sample_frame_bytes
}

pub(crate) struct AudioUdpSender {
    socket: UdpSocket,
    stream_id: u64,
    packet: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct AudioUdpEndpoint {
    pub(crate) host: String,
    pub(crate) port: u16,
    stream_id: u64,
    pub(crate) return_stream_id: Option<u64>,
}

impl AudioUdpSender {
    fn from_payload(payload: &str) -> Result<Option<Self>, String> {
        AudioUdpEndpoint::from_payload(payload)
            .map(|endpoint| endpoint.map(Self::connect))
            .and_then(|sender| sender.transpose())
    }

    pub(crate) fn connect(endpoint: AudioUdpEndpoint) -> Result<Self, String> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("bind udp failed: {error}"))?;
        socket
            .connect(endpoint.addr())
            .map_err(|error| format!("connect udp relay failed: {error}"))?;
        Ok(Self {
            socket,
            stream_id: endpoint.stream_id,
            packet: Vec::with_capacity(audio_udp::MAX_PACKET_BYTES),
        })
    }

    fn send_frame(
        &mut self,
        seq: u64,
        frame: &crate::live_control::CapturedAudioFrame,
    ) -> io::Result<()> {
        audio_udp::encode_audio(
            self.stream_id,
            seq,
            now_epoch_ms() as u64,
            frame.sample_rate,
            frame.channels,
            &frame.format,
            &frame.bytes,
            &mut self.packet,
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.socket.send(&self.packet).map(|_| ())
    }
}

impl AudioUdpEndpoint {
    pub(crate) fn from_payload(payload: &str) -> Result<Option<Self>, String> {
        if video_control_value(payload, "transport").as_deref() != Some("udp") {
            return Ok(None);
        }
        let host = video_control_value(payload, "udp_host")
            .ok_or_else(|| "missing audio udp host".to_string())?;
        let port = video_control_value(payload, "udp_port")
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| "missing audio udp port".to_string())?;
        let stream_id = video_control_value(payload, "udp_stream")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| "missing audio udp stream".to_string())?;
        let return_stream_id =
            video_control_value(payload, "udp_return_stream").and_then(|value| value.parse().ok());
        Ok(Some(Self {
            host,
            port,
            stream_id,
            return_stream_id,
        }))
    }

    pub(crate) fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub(crate) fn new_audio_udp_stream_id(tag: u64) -> u64 {
    ((now_epoch_ms() as u64).saturating_mul(1024))
        .saturating_add(tag)
        .max(1)
}

pub(crate) fn audio_udp_receive_loop(
    socket: UdpSocket,
    server_addr: String,
    stream_id: u64,
    stop: Arc<AtomicBool>,
    sink: crate::live_control::AudioOutputSink,
    speaker_muted: Arc<AtomicBool>,
    event_sink: ClientEventSink,
) {
    let mut register_packet = Vec::new();
    let mut unregister_packet = Vec::new();
    audio_udp::encode_register(stream_id, &mut register_packet);
    audio_udp::encode_unregister(stream_id, &mut unregister_packet);
    let mut last_register = Instant::now() - Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS);
    let mut buf = [0_u8; audio_udp::MAX_PACKET_BYTES];
    let mut last_seq = 0_u64;
    let mut received_packets = 0_u64;
    let mut received_bytes = 0_u64;
    let mut duplicate_drops = 0_u64;
    let mut muted_drops = 0_u64;
    let mut playback_errors = 0_u64;
    let mut last_report = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        if last_register.elapsed() >= Duration::from_millis(AUDIO_UDP_REGISTER_INTERVAL_MS) {
            if let Err(error) = socket.send_to(&register_packet, &server_addr) {
                event_sink.send(ClientEvent::Log(format!(
                    "voice udp register failed: {error}"
                )));
                break;
            }
            last_register = Instant::now();
        }

        match socket.recv_from(&mut buf) {
            Ok((len, _)) => match audio_udp::decode(&buf[..len]) {
                Ok(audio_udp::Packet::Audio {
                    stream_id: packet_stream_id,
                    seq,
                    sample_rate,
                    channels,
                    format,
                    bytes,
                    ..
                }) if packet_stream_id == stream_id => {
                    if seq <= last_seq {
                        duplicate_drops = duplicate_drops.saturating_add(1);
                        continue;
                    }
                    last_seq = seq;
                    received_packets = received_packets.saturating_add(1);
                    received_bytes = received_bytes.saturating_add(bytes.len() as u64);
                    if speaker_muted.load(Ordering::Relaxed) {
                        muted_drops = muted_drops.saturating_add(1);
                        continue;
                    }
                    if let Err(error) = sink.push_frame(sample_rate, channels, format, bytes) {
                        playback_errors = playback_errors.saturating_add(1);
                        event_sink.send(ClientEvent::Log(format!(
                            "voice udp playback failed: {error}"
                        )));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    event_sink.send(ClientEvent::Log(format!(
                        "voice udp packet ignored: {error}"
                    )));
                }
            },
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => {
                event_sink.send(ClientEvent::Log(format!(
                    "voice udp receive failed: {error}"
                )));
                break;
            }
        }

        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=voice_chat_rx transport=udp stream={} packets={} bytes={} muted_drops={} duplicate_drops={} playback_errors={} last_seq={}",
                stream_id,
                received_packets,
                received_bytes,
                muted_drops,
                duplicate_drops,
                playback_errors,
                last_seq
            );
            last_report = Instant::now();
        }
    }

    let _ = socket.send_to(&unregister_packet, server_addr);
}

pub(crate) fn audio_stream_loop(
    client_id: String,
    start_payload: String,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let mut udp_sender = match AudioUdpSender::from_payload(&start_payload) {
        Ok(Some(sender)) => sender,
        Ok(None) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: "audio_listen_error\nmessage=udp transport required".to_string(),
                },
            );
            return;
        }
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: format!("audio_listen_error\nmessage={error}"),
                },
            );
            return;
        }
    };

    let device = video_control_value(&start_payload, "device")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    let input_stream = match crate::live_control::start_audio_input_stream(device, frame_tx) {
        Ok(stream) => stream,
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::AudioListen,
                    accepted: false,
                    detail: format!("audio_listen_error\nmessage={error}"),
                },
            );
            return;
        }
    };
    let _ = queue_message(
        &out_tx,
        &session_token,
        Message::CommandAck {
            client_id: client_id.clone(),
            command: CommandKind::AudioListen,
            accepted: true,
            detail: format!(
                "audio_listen_started\nsample_rate={}\nchannels={}\nformat={}\ngeneration={generation}\ntransport=udp",
                input_stream.sample_rate, input_stream.channels, input_stream.format
            ),
        },
    );

    let mut seq = stream_sequence_base(generation);
    let mut packetizer = AudioFramePacketizer::default();
    let mut sent_packets = 0_u64;
    let mut sent_bytes = 0_u64;
    let mut last_report = Instant::now();
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let frame = match frame_rx
            .recv_timeout(Duration::from_millis(AUDIO_CAPTURE_RECV_TIMEOUT_MS))
        {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let detail = "audio_listen_error\nmessage=audio input stream stopped unexpectedly"
                    .to_string();
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id: client_id.clone(),
                        command: CommandKind::AudioListen,
                        accepted: false,
                        detail,
                    },
                );
                break;
            }
        };
        for frame in packetizer.push(frame) {
            let frame_bytes = frame.bytes.len() as u64;
            match udp_sender.send_frame(seq, &frame) {
                Ok(()) => {
                    sent_packets = sent_packets.saturating_add(1);
                    sent_bytes = sent_bytes.saturating_add(frame_bytes);
                }
                Err(error) => {
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::AudioListen,
                            accepted: false,
                            detail: format!("audio_listen_error\nmessage=udp send failed: {error}"),
                        },
                    );
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
            }
            seq = seq.saturating_add(1);
        }
        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=audio_listen_tx client={} transport={} packets={} bytes={} queue_drops={} capture_drops={} pending_bytes={}",
                client_id,
                "udp",
                sent_packets,
                sent_bytes,
                0,
                input_stream.dropped_callbacks.load(Ordering::Relaxed),
                packetizer.pending.len()
            );
            last_report = Instant::now();
        }
    }
    drop(input_stream);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn voice_chat_capture_loop(
    client_id: String,
    mut udp_sender: AudioUdpSender,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
    mic_muted: Arc<AtomicBool>,
    event_sink: ClientEventSink,
) {
    let (frame_tx, frame_rx) = mpsc::sync_channel(8);
    let input_stream = match crate::live_control::start_audio_input_stream(0, frame_tx) {
        Ok(stream) => stream,
        Err(error) => {
            stream_state.running.store(false, Ordering::Relaxed);
            let _ = queue_message(
                &out_tx,
                &session_token,
                Message::CommandAck {
                    client_id,
                    command: CommandKind::VoiceChat,
                    accepted: false,
                    detail: format!("voice_chat_error\nmessage={error}"),
                },
            );
            event_sink.send(ClientEvent::VoiceChatFailed { message: error });
            return;
        }
    };

    let mut seq = stream_sequence_base(generation);
    let mut packetizer = AudioFramePacketizer::default();
    let mut sent_packets = 0_u64;
    let mut sent_bytes = 0_u64;
    let mut last_report = Instant::now();
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let frame =
            match frame_rx.recv_timeout(Duration::from_millis(AUDIO_CAPTURE_RECV_TIMEOUT_MS)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let message = "audio input stream stopped unexpectedly".to_string();
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: false,
                            detail: format!("voice_chat_error\nmessage={message}"),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatFailed { message });
                    break;
                }
            };
        if mic_muted.load(Ordering::Relaxed) {
            packetizer.clear_pending();
            continue;
        }
        for frame in packetizer.push(frame) {
            let frame_bytes = frame.bytes.len() as u64;
            match udp_sender.send_frame(seq, &frame) {
                Ok(()) => {
                    sent_packets = sent_packets.saturating_add(1);
                    sent_bytes = sent_bytes.saturating_add(frame_bytes);
                }
                Err(error) => {
                    let message = format!("udp send failed: {error}");
                    let _ = queue_message(
                        &out_tx,
                        &session_token,
                        Message::CommandAck {
                            client_id: client_id.clone(),
                            command: CommandKind::VoiceChat,
                            accepted: false,
                            detail: format!("voice_chat_error\nmessage={message}"),
                        },
                    );
                    event_sink.send(ClientEvent::VoiceChatFailed { message });
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
            }
            seq = seq.saturating_add(1);
        }
        if last_report.elapsed() >= Duration::from_millis(AUDIO_STREAM_REPORT_INTERVAL_MS) {
            debug_log!(
                "debug event=voice_chat_tx client={} transport=udp packets={} bytes={} capture_drops={} pending_bytes={}",
                client_id,
                sent_packets,
                sent_bytes,
                input_stream.dropped_callbacks.load(Ordering::Relaxed),
                packetizer.pending.len()
            );
            last_report = Instant::now();
        }
    }
    drop(input_stream);
}
