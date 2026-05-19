use crate::live_control::realtime_video::RealtimeVideoSender;
use crate::outbound::{queue_message, ClientOutbound};
use crate::payload::{
    remote_desktop_value, stream_sequence_base, video_control_value, video_fps_from_payload,
    video_source_command,
};
use crate::stream_state::DesktopStreamState;
use rdl_protocol::{Message, VideoSource};
use std::io;
use std::sync::{
    atomic::Ordering,
    mpsc::{self, SyncSender},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
pub(crate) fn video_stream_loop(
    client_id: String,
    source: VideoSource,
    start_payload: String,
    realtime_tx: RealtimeVideoSender<ClientOutbound>,
    out_tx: SyncSender<ClientOutbound>,
    session_token: String,
    stream_state: Arc<DesktopStreamState>,
    generation: u64,
) {
    let quality = remote_desktop_value(&start_payload, "quality")
        .or_else(|| video_control_value(&start_payload, "quality"))
        .unwrap_or_else(|| "medium".to_string());
    let fps = video_fps_from_payload(&start_payload, &quality);
    let interval = Duration::from_millis((1000 / fps).max(1));
    let mut remote_desktop_capture = None;
    let mut camera_capture = None;
    let camera_device = video_control_value(&start_payload, "device")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    if matches!(source, VideoSource::RemoteDesktop) {
        let remote_desktop_screen = video_control_value(&start_payload, "screen")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or_default();
        match crate::live_control::open_remote_desktop_capture(remote_desktop_screen, &quality) {
            Ok(capture) => {
                remote_desktop_capture = Some(capture);
            }
            Err(error) => {
                stream_state.running.store(false, Ordering::Relaxed);
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id,
                        command: video_source_command(&source),
                        accepted: false,
                        detail: error,
                    },
                );
                return;
            }
        }
    }
    if matches!(source, VideoSource::Camera) {
        match crate::live_control::open_camera_capture(camera_device, &quality) {
            Ok(capture) => {
                camera_capture = Some(capture);
            }
            Err(error) => {
                stream_state.running.store(false, Ordering::Relaxed);
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id,
                        command: video_source_command(&source),
                        accepted: false,
                        detail: error,
                    },
                );
                return;
            }
        }
    }
    let mut seq = stream_sequence_base(generation);
    while stream_state.running.load(Ordering::Relaxed)
        && stream_state.generation.load(Ordering::Relaxed) == generation
    {
        let started = Instant::now();
        let frame = match &source {
            VideoSource::RemoteDesktop => {
                let capture = remote_desktop_capture
                    .as_mut()
                    .ok_or_else(|| "remote desktop capture is not open".to_string());
                capture.and_then(|capture| {
                    crate::live_control::capture_remote_desktop_stream_frame(capture).map(|frame| {
                        Message::VideoFrame {
                            client_id: client_id.clone(),
                            source: VideoSource::RemoteDesktop,
                            seq,
                            source_width: frame.source_width,
                            source_height: frame.source_height,
                            image_width: frame.image_width,
                            image_height: frame.image_height,
                            format: frame.format,
                            bytes: frame.bytes,
                        }
                    })
                })
            }
            VideoSource::Camera => {
                let capture = camera_capture
                    .as_mut()
                    .ok_or_else(|| "camera capture is not open".to_string());
                capture.and_then(|capture| {
                    crate::live_control::capture_camera_stream_frame(capture).map(|frame| {
                        Message::VideoFrame {
                            client_id: client_id.clone(),
                            source: VideoSource::Camera,
                            seq,
                            source_width: frame.width,
                            source_height: frame.height,
                            image_width: frame.width,
                            image_height: frame.height,
                            format: frame.format,
                            bytes: frame.bytes,
                        }
                    })
                })
            }
        };
        match frame {
            Ok(message) => {
                if try_queue_realtime_message(&realtime_tx, &session_token, message).is_err() {
                    stream_state.running.store(false, Ordering::Relaxed);
                    break;
                }
                seq = seq.saturating_add(1);
            }
            Err(error) => {
                let _ = queue_message(
                    &out_tx,
                    &session_token,
                    Message::CommandAck {
                        client_id: client_id.clone(),
                        command: video_source_command(&source),
                        accepted: false,
                        detail: error,
                    },
                );
                stream_state.running.store(false, Ordering::Relaxed);
                break;
            }
        }
        let elapsed = started.elapsed();
        if elapsed < interval {
            thread::sleep(interval - elapsed);
        }
    }
}

fn try_queue_realtime_message(
    out_tx: &RealtimeVideoSender<ClientOutbound>,
    session_token: &str,
    message: Message,
) -> io::Result<bool> {
    match out_tx.send_latest(ClientOutbound {
        session_token: session_token.to_string(),
        message,
    }) {
        Ok(()) => Ok(true),
        Err(mpsc::SendError(_)) => Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "outbound queue disconnected",
        )),
    }
}
