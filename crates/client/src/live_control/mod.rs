use rdl_protocol::CommandKind;

mod audio_listen;
mod camera;
mod remote_desktop;

pub(crate) use audio_listen::{AudioInputStream, AudioOutputPlayer, CapturedAudioFrame};
pub(crate) use camera::CameraVideoFrame;
pub(crate) use remote_desktop::RemoteDesktopVideoFrame;

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

pub(crate) fn capture_remote_desktop_video_frame(
    screen_index: usize,
    quality: &str,
) -> Result<RemoteDesktopVideoFrame, String> {
    remote_desktop::capture_video_frame(screen_index, quality)
}

pub(crate) fn capture_camera_video_frame(
    device: usize,
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    camera::capture_video_frame(device, quality)
}

pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    audio_listen::confirm_audio_listen()
}

pub(crate) fn start_audio_input_stream(
    device: usize,
    frame_tx: std::sync::mpsc::SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    audio_listen::start_input_stream(device, frame_tx)
}
