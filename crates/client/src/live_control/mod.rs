use rdl_protocol::CommandKind;

mod camera;
mod remote_desktop;

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

pub(crate) fn capture_remote_desktop_video_frame(
    screen_index: usize,
    quality: &str,
) -> Result<RemoteDesktopVideoFrame, String> {
    remote_desktop::capture_video_frame(screen_index, quality)
}
