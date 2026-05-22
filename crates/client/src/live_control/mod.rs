use rdl_protocol::{CommandKind, VideoSource};

mod audio_stream;
#[cfg(feature = "audio-control")]
mod audio_listen;
#[cfg(feature = "video-control")]
mod camera;
mod live_video_stream;
pub(crate) mod payload;
pub(crate) mod realtime_video;
#[cfg(feature = "video-control")]
mod remote_desktop;
mod stream_state;
#[cfg(all(
    feature = "video-control",
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
mod tile_diff;

pub(crate) use audio_stream::{
    audio_stream_loop, audio_udp_receive_loop, new_audio_udp_stream_id, voice_chat_capture_loop,
    AudioUdpEndpoint, AudioUdpSender, AUDIO_STREAM_STOP_SETTLE_MS, AUDIO_UDP_RECV_TIMEOUT_MS,
};
#[cfg(feature = "audio-control")]
pub(crate) use audio_listen::{
    AudioInputStream, AudioOutputPlayer, AudioOutputSink, CapturedAudioFrame,
};
#[cfg(feature = "video-control")]
pub(crate) use camera::{CameraCapture, CameraVideoFrame};
pub(crate) use live_video_stream::video_stream_loop;
#[cfg(feature = "video-control")]
pub(crate) use remote_desktop::{RemoteDesktopCapture, RemoteDesktopVideoFrame};
pub(crate) use stream_state::DesktopStreamState;

#[cfg(not(feature = "audio-control"))]
pub(crate) struct CapturedAudioFrame {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "audio-control"))]
pub(crate) struct AudioInputStream {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    pub(crate) dropped_callbacks: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(not(feature = "audio-control"))]
pub(crate) struct AudioOutputPlayer;

#[cfg(not(feature = "audio-control"))]
pub(crate) struct AudioOutputSink;

#[cfg(not(feature = "video-control"))]
pub(crate) struct RemoteDesktopVideoFrame {
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "video-control"))]
pub(crate) struct RemoteDesktopCapture;

#[cfg(not(feature = "video-control"))]
pub(crate) struct CameraVideoFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "audio-control"))]
impl AudioOutputPlayer {
    pub(crate) fn start() -> Result<Self, String> {
        Err(audio_unavailable_message())
    }

    pub(crate) fn sink(&self) -> AudioOutputSink {
        AudioOutputSink
    }
}

#[cfg(not(feature = "audio-control"))]
impl AudioOutputSink {
    pub(crate) fn push_frame(
        &self,
        _sample_rate: u32,
        _channels: u16,
        _format: &str,
        _bytes: &[u8],
    ) -> Result<(), String> {
        Err(audio_unavailable_message())
    }
}

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::RemoteDesktop => handle_remote_desktop(payload),
        CommandKind::Camera => handle_camera(payload),
        CommandKind::AudioListen => handle_audio_listen(payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

#[cfg(feature = "video-control")]
fn handle_remote_desktop(payload: &str) -> String {
    remote_desktop::handle(payload)
}

#[cfg(not(feature = "video-control"))]
fn handle_remote_desktop(_payload: &str) -> String {
    disabled_detail(&CommandKind::RemoteDesktop)
}

#[cfg(feature = "video-control")]
fn handle_camera(payload: &str) -> String {
    camera::handle(payload)
}

#[cfg(not(feature = "video-control"))]
fn handle_camera(_payload: &str) -> String {
    disabled_detail(&CommandKind::Camera)
}

#[cfg(feature = "audio-control")]
fn handle_audio_listen(payload: &str) -> String {
    audio_listen::handle(payload)
}

#[cfg(not(feature = "audio-control"))]
fn handle_audio_listen(_payload: &str) -> String {
    disabled_detail(&CommandKind::AudioListen)
}

pub(crate) fn command_available(command: &CommandKind) -> bool {
    match command {
        CommandKind::RemoteDesktop | CommandKind::Camera => video_control_available(),
        CommandKind::AudioListen => audio_control_available(),
        _ => true,
    }
}

pub(crate) fn video_source_available(source: &VideoSource) -> bool {
    match source {
        VideoSource::RemoteDesktop | VideoSource::Camera => video_control_available(),
    }
}

pub(crate) fn video_control_available() -> bool {
    cfg!(feature = "video-control")
}

pub(crate) fn audio_control_available() -> bool {
    cfg!(feature = "audio-control")
}

pub(crate) fn disabled_detail(command: &CommandKind) -> String {
    match command {
        CommandKind::RemoteDesktop => {
            "remote_desktop_error\nmessage=client video control is not available".to_string()
        }
        CommandKind::Camera => {
            "camera_error\nmessage=client video control is not available".to_string()
        }
        CommandKind::AudioListen => {
            "audio_listen_error\nmessage=client audio control is not available".to_string()
        }
        _ => format!(
            "{}_disabled\nmessage=client GUI is not available",
            command.as_str()
        ),
    }
}

#[cfg(feature = "video-control")]
pub(crate) fn open_remote_desktop_capture(
    screen_index: usize,
    quality: &str,
    tile_diff_enabled: bool,
) -> Result<RemoteDesktopCapture, String> {
    RemoteDesktopCapture::new(screen_index, quality, tile_diff_enabled)
}

#[cfg(not(feature = "video-control"))]
pub(crate) struct CameraCapture;

#[cfg(not(feature = "video-control"))]
pub(crate) fn open_remote_desktop_capture(
    _screen_index: usize,
    _quality: &str,
    _tile_diff_enabled: bool,
) -> Result<RemoteDesktopCapture, String> {
    Err(video_unavailable_message())
}

#[cfg(feature = "video-control")]
pub(crate) fn capture_remote_desktop_stream_frame(
    capture: &mut RemoteDesktopCapture,
) -> Result<Option<RemoteDesktopVideoFrame>, String> {
    capture.capture_frame()
}

#[cfg(not(feature = "video-control"))]
pub(crate) fn capture_remote_desktop_stream_frame(
    _capture: &mut RemoteDesktopCapture,
) -> Result<Option<RemoteDesktopVideoFrame>, String> {
    Err(video_unavailable_message())
}

#[cfg(feature = "video-control")]
pub(crate) fn open_camera_capture(device: usize, quality: &str) -> Result<CameraCapture, String> {
    CameraCapture::new(device, quality)
}

#[cfg(not(feature = "video-control"))]
pub(crate) fn open_camera_capture(_device: usize, _quality: &str) -> Result<CameraCapture, String> {
    Err(video_unavailable_message())
}

#[cfg(feature = "video-control")]
pub(crate) fn capture_camera_stream_frame(
    capture: &mut CameraCapture,
) -> Result<CameraVideoFrame, String> {
    capture.capture_frame()
}

#[cfg(not(feature = "video-control"))]
pub(crate) fn capture_camera_stream_frame(
    _capture: &mut CameraCapture,
) -> Result<CameraVideoFrame, String> {
    Err(video_unavailable_message())
}

#[cfg(feature = "audio-control")]
#[allow(dead_code)]
pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    audio_listen::confirm_audio_listen()
}

#[cfg(not(feature = "audio-control"))]
#[allow(dead_code)]
pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    Err(audio_unavailable_message())
}

#[cfg(feature = "audio-control")]
pub(crate) fn start_audio_input_stream(
    device: usize,
    frame_tx: std::sync::mpsc::SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    audio_listen::start_input_stream(device, frame_tx)
}

#[cfg(not(feature = "audio-control"))]
pub(crate) fn start_audio_input_stream(
    _device: usize,
    _frame_tx: std::sync::mpsc::SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    Err(audio_unavailable_message())
}

#[cfg(not(feature = "audio-control"))]
fn audio_unavailable_message() -> String {
    "client audio control is not available".to_string()
}

#[cfg(not(feature = "video-control"))]
fn video_unavailable_message() -> String {
    "client video control is not available".to_string()
}
