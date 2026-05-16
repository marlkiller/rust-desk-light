use rdl_protocol::CommandKind;

mod camera;
mod remote_desktop;

pub(crate) use camera::CameraVideoFrame;
pub(crate) use remote_desktop::RemoteDesktopVideoFrame;

pub fn handle(command: &CommandKind, payload: &str) -> String {
    match command {
        CommandKind::RemoteDesktop => remote_desktop::handle(payload),
        CommandKind::Camera => camera::handle(payload),
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
