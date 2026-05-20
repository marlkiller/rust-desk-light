use rdl_protocol::CommandKind;

#[cfg(feature = "gui")]
mod audio_listen;
#[cfg(feature = "gui")]
mod camera;
pub(crate) mod realtime_video;
#[cfg(feature = "gui")]
mod remote_desktop;

#[cfg(feature = "gui")]
pub(crate) use audio_listen::{
    AudioInputStream, AudioOutputPlayer, AudioOutputSink, CapturedAudioFrame,
};
#[cfg(feature = "gui")]
pub(crate) use camera::{CameraCapture, CameraVideoFrame};
#[cfg(feature = "gui")]
pub(crate) use remote_desktop::{RemoteDesktopCapture, RemoteDesktopVideoFrame};

#[cfg(not(feature = "gui"))]
pub(crate) struct CapturedAudioFrame {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "gui"))]
pub(crate) struct AudioInputStream {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    pub(crate) dropped_callbacks: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(not(feature = "gui"))]
pub(crate) struct AudioOutputPlayer;

#[cfg(not(feature = "gui"))]
pub(crate) struct AudioOutputSink;

#[cfg(not(feature = "gui"))]
pub(crate) struct RemoteDesktopVideoFrame {
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) image_width: u32,
    pub(crate) image_height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "gui"))]
pub(crate) struct RemoteDesktopCapture;

#[cfg(not(feature = "gui"))]
pub(crate) struct CameraVideoFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(not(feature = "gui"))]
impl AudioOutputPlayer {
    pub(crate) fn start() -> Result<Self, String> {
        Err(gui_unavailable_message())
    }

    pub(crate) fn sink(&self) -> AudioOutputSink {
        AudioOutputSink
    }
}

#[cfg(not(feature = "gui"))]
impl AudioOutputSink {
    pub(crate) fn push_frame(
        &self,
        _sample_rate: u32,
        _channels: u16,
        _format: &str,
        _bytes: &[u8],
    ) -> Result<(), String> {
        Err(gui_unavailable_message())
    }
}

#[cfg(feature = "gui")]
pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::RemoteDesktop => remote_desktop::handle(payload),
        CommandKind::Camera => camera::handle(payload),
        CommandKind::AudioListen => audio_listen::handle(payload),
        _ => format!(
            "TODO: {} accepted as planned stub; payload='{}'",
            command.as_str(),
            payload
        ),
    }
}

#[cfg(not(feature = "gui"))]
pub fn handle(command: &CommandKind, _payload: &str) -> String {
    disabled_detail(command)
}

pub(crate) fn disabled_detail(command: &CommandKind) -> String {
    match command {
        CommandKind::RemoteDesktop => {
            "remote_desktop_error\nmessage=client GUI is not available".to_string()
        }
        CommandKind::Camera => "camera_error\nmessage=client GUI is not available".to_string(),
        CommandKind::AudioListen => {
            "audio_listen_error\nmessage=client GUI is not available".to_string()
        }
        _ => format!(
            "{}_disabled\nmessage=client GUI is not available",
            command.as_str()
        ),
    }
}

#[cfg(feature = "gui")]
pub(crate) fn open_remote_desktop_capture(
    screen_index: usize,
    quality: &str,
) -> Result<RemoteDesktopCapture, String> {
    RemoteDesktopCapture::new(screen_index, quality)
}

#[cfg(not(feature = "gui"))]
pub(crate) struct CameraCapture;

#[cfg(not(feature = "gui"))]
pub(crate) fn open_remote_desktop_capture(
    _screen_index: usize,
    _quality: &str,
) -> Result<RemoteDesktopCapture, String> {
    Err(gui_unavailable_message())
}

#[cfg(feature = "gui")]
pub(crate) fn capture_remote_desktop_stream_frame(
    capture: &mut RemoteDesktopCapture,
) -> Result<Option<RemoteDesktopVideoFrame>, String> {
    capture.capture_frame()
}

#[cfg(not(feature = "gui"))]
pub(crate) fn capture_remote_desktop_stream_frame(
    _capture: &mut RemoteDesktopCapture,
) -> Result<Option<RemoteDesktopVideoFrame>, String> {
    Err(gui_unavailable_message())
}

#[cfg(feature = "gui")]
pub(crate) fn open_camera_capture(device: usize, quality: &str) -> Result<CameraCapture, String> {
    CameraCapture::new(device, quality)
}

#[cfg(not(feature = "gui"))]
pub(crate) fn open_camera_capture(_device: usize, _quality: &str) -> Result<CameraCapture, String> {
    Err(gui_unavailable_message())
}

#[cfg(feature = "gui")]
pub(crate) fn capture_camera_stream_frame(
    capture: &mut CameraCapture,
) -> Result<CameraVideoFrame, String> {
    capture.capture_frame()
}

#[cfg(not(feature = "gui"))]
pub(crate) fn capture_camera_stream_frame(
    _capture: &mut CameraCapture,
) -> Result<CameraVideoFrame, String> {
    Err(gui_unavailable_message())
}

#[cfg(feature = "gui")]
#[allow(dead_code)]
pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    audio_listen::confirm_audio_listen()
}

#[cfg(not(feature = "gui"))]
#[allow(dead_code)]
pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    Err(gui_unavailable_message())
}

#[cfg(feature = "gui")]
pub(crate) fn start_audio_input_stream(
    device: usize,
    frame_tx: std::sync::mpsc::SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    audio_listen::start_input_stream(device, frame_tx)
}

#[cfg(not(feature = "gui"))]
pub(crate) fn start_audio_input_stream(
    _device: usize,
    _frame_tx: std::sync::mpsc::SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    Err(gui_unavailable_message())
}

#[cfg(not(feature = "gui"))]
fn gui_unavailable_message() -> String {
    "client GUI is not available".to_string()
}
