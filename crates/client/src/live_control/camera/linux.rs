use super::{encode_camera_image, CameraCapture, CameraRequest, CameraVideoFrame};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

const FFMPEG_STREAM_FRAME_TIMEOUT: Duration = Duration::from_millis(900);
const MAX_MJPEG_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[cfg(target_os = "linux")]
pub(super) fn capture_frame(request: &CameraRequest) -> Result<CameraVideoFrame, String> {
    let mut capture = CameraCapture::new(request.device, &request.quality)?;
    capture_single_frame(&mut capture, String::new())
}

#[cfg(target_os = "linux")]
pub(super) fn capture_stream_frame(
    capture: &mut CameraCapture,
) -> Result<CameraVideoFrame, String> {
    let mut last_error = String::new();
    if !capture.ffmpeg_stream_failed {
        match capture_ffmpeg_stream_frame(capture) {
            Ok(frame) => return Ok(frame),
            Err(error) => {
                capture.ffmpeg_stream = None;
                capture.ffmpeg_stream_failed = true;
                last_error = format!("ffmpeg stream failed: {error}");
            }
        }
    }
    capture_single_frame(capture, last_error)
}

#[cfg(target_os = "linux")]
fn capture_single_frame(
    capture: &mut CameraCapture,
    mut last_error: String,
) -> Result<CameraVideoFrame, String> {
    for offset in 0..capture.backends.len() {
        let index = (capture.active_backend + offset) % capture.backends.len();
        match capture.backends[index]
            .capture(&capture.device_path)
            .and_then(|bytes| encode_camera_bytes(bytes, &capture.quality))
        {
            Ok(frame) => {
                capture.active_backend = index;
                return Ok(frame);
            }
            Err(error) => {
                last_error = error;
            }
        }
    }
    Err(if last_error.trim().is_empty() {
        "Linux camera capture requires ffmpeg, fswebcam, or streamer".to_string()
    } else {
        last_error
    })
}

#[cfg(target_os = "linux")]
pub(super) struct FfmpegMjpegStream {
    child: Child,
    frame_rx: Receiver<Result<Vec<u8>, String>>,
}

#[cfg(target_os = "linux")]
impl FfmpegMjpegStream {
    fn open(device_path: &str) -> Result<Self, String> {
        if !command_in_path("ffmpeg") {
            return Err("ffmpeg was not found in PATH".to_string());
        }
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-f",
                "video4linux2",
                "-i",
                device_path,
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("start ffmpeg stream failed: {error}"))?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("ffmpeg stream stdout is not available".to_string());
        };
        let (frame_tx, frame_rx) = mpsc::sync_channel(2);
        std::thread::spawn(move || read_mjpeg_stream(stdout, frame_tx));
        Ok(Self { child, frame_rx })
    }

    fn read_frame(&mut self) -> Result<Vec<u8>, String> {
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|error| format!("poll ffmpeg stream failed: {error}"))?
        {
            return Err(format!("ffmpeg camera stream exited: {status}"));
        }

        let mut latest = None;
        loop {
            match self.frame_rx.try_recv() {
                Ok(Ok(bytes)) => latest = Some(bytes),
                Ok(Err(error)) => return Err(error),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    return Err("ffmpeg camera stream reader stopped".to_string())
                }
            }
        }
        if let Some(bytes) = latest {
            return Ok(bytes);
        }
        match self.frame_rx.recv_timeout(FFMPEG_STREAM_FRAME_TIMEOUT) {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err("ffmpeg camera stream timed out waiting for a frame".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("ffmpeg camera stream reader stopped".to_string())
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for FfmpegMjpegStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
fn capture_ffmpeg_stream_frame(capture: &mut CameraCapture) -> Result<CameraVideoFrame, String> {
    if capture.ffmpeg_stream.is_none() {
        capture.ffmpeg_stream = Some(FfmpegMjpegStream::open(&capture.device_path)?);
    }
    let bytes = capture
        .ffmpeg_stream
        .as_mut()
        .ok_or_else(|| "ffmpeg camera stream is not open".to_string())?
        .read_frame()?;
    encode_camera_bytes(bytes, &capture.quality)
}

#[cfg(target_os = "linux")]
fn read_mjpeg_stream<R: Read>(mut reader: R, frame_tx: SyncSender<Result<Vec<u8>, String>>) {
    let mut buffer = [0_u8; 8192];
    let mut frame = Vec::new();
    let mut in_frame = false;
    let mut previous = 0_u8;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => {
                let message = if in_frame {
                    "ffmpeg camera stream ended mid-frame"
                } else {
                    "ffmpeg camera stream ended"
                };
                let _ = send_stream_result(&frame_tx, Err(message.to_string()));
                return;
            }
            Ok(read) => read,
            Err(error) => {
                let _ = send_stream_result(
                    &frame_tx,
                    Err(format!("read ffmpeg camera stream failed: {error}")),
                );
                return;
            }
        };
        for byte in &buffer[..read] {
            if in_frame {
                frame.push(*byte);
                if previous == 0xFF && *byte == 0xD9 {
                    let completed = std::mem::take(&mut frame);
                    in_frame = false;
                    if !send_stream_result(&frame_tx, Ok(completed)) {
                        return;
                    }
                } else if frame.len() > MAX_MJPEG_FRAME_BYTES {
                    let _ = send_stream_result(
                        &frame_tx,
                        Err("ffmpeg camera frame is too large".to_string()),
                    );
                    return;
                }
            } else if previous == 0xFF && *byte == 0xD8 {
                in_frame = true;
                frame.clear();
                frame.push(0xFF);
                frame.push(0xD8);
            }
            previous = *byte;
        }
    }
}

#[cfg(target_os = "linux")]
fn send_stream_result(
    frame_tx: &SyncSender<Result<Vec<u8>, String>>,
    result: Result<Vec<u8>, String>,
) -> bool {
    match frame_tx.try_send(result) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub(super) enum CameraBackend {
    FfmpegStdout,
    FfmpegFile,
    FswebcamFile,
    StreamerFile,
}

#[cfg(target_os = "linux")]
impl CameraBackend {
    fn capture(self, device_path: &str) -> Result<Vec<u8>, String> {
        match self {
            Self::FfmpegStdout => run_capture_stdout(
                "ffmpeg",
                &[
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "video4linux2",
                    "-i",
                    device_path,
                    "-frames:v",
                    "1",
                    "-f",
                    "image2pipe",
                    "-vcodec",
                    "mjpeg",
                    "-",
                ],
            ),
            Self::FfmpegFile => {
                let path = temp_path("rdl-camera", "jpg");
                let path_text = path.to_string_lossy().to_string();
                run_capture_file(
                    "ffmpeg",
                    &[
                        "-y",
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-f",
                        "video4linux2",
                        "-i",
                        device_path,
                        "-frames:v",
                        "1",
                        &path_text,
                    ],
                    &path,
                )
            }
            Self::FswebcamFile => {
                let path = temp_path("rdl-camera", "jpg");
                let path_text = path.to_string_lossy().to_string();
                run_capture_file(
                    "fswebcam",
                    &[
                        "--no-banner",
                        "--device",
                        device_path,
                        "-r",
                        "1280x720",
                        &path_text,
                    ],
                    &path,
                )
            }
            Self::StreamerFile => {
                let path = temp_path("rdl-camera", "jpg");
                let path_text = path.to_string_lossy().to_string();
                run_capture_file(
                    "streamer",
                    &["-c", device_path, "-f", "jpeg", "-o", &path_text],
                    &path,
                )
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn encode_camera_bytes(bytes: Vec<u8>, quality: &str) -> Result<CameraVideoFrame, String> {
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("load camera frame failed: {error}"))?;
    let (bytes, width, height) = encode_camera_image(image, quality)
        .map_err(|error| format!("encode camera frame failed: {error}"))?;
    Ok(CameraVideoFrame {
        width,
        height,
        format: "jpeg".to_string(),
        bytes,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn list_devices() -> String {
    let mut output = String::from("camera_devices");
    let devices = linux_camera_devices();
    if devices.is_empty() {
        output.push_str("\ndevice\t0\tDefault camera\t/dev/video0");
        return output;
    }
    for (index, path) in devices {
        output.push_str(&format!(
            "\ndevice\t{}\t{}\t{}",
            index,
            sanitize_linux_field(&format!("Camera {index}")),
            sanitize_linux_field(&path)
        ));
    }
    output
}

#[cfg(target_os = "linux")]
fn linux_camera_devices() -> Vec<(usize, String)> {
    let mut devices = fs::read_dir("/dev")
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let index = name.strip_prefix("video")?.parse::<usize>().ok()?;
            Some((index, format!("/dev/{name}")))
        })
        .collect::<Vec<_>>();
    devices.sort_by_key(|(index, _)| *index);
    devices
}

#[cfg(target_os = "linux")]
pub(super) fn device_path(device: usize) -> String {
    let path = format!("/dev/video{device}");
    if Path::new(&path).exists() {
        path
    } else {
        "/dev/video0".to_string()
    }
}

#[cfg(target_os = "linux")]
pub(super) fn camera_backends() -> Result<Vec<CameraBackend>, String> {
    let mut backends = Vec::new();
    if command_in_path("ffmpeg") {
        backends.push(CameraBackend::FfmpegStdout);
        backends.push(CameraBackend::FfmpegFile);
    }
    if command_in_path("fswebcam") {
        backends.push(CameraBackend::FswebcamFile);
    }
    if command_in_path("streamer") {
        backends.push(CameraBackend::StreamerFile);
    }
    if backends.is_empty() {
        Err("Linux camera capture requires ffmpeg, fswebcam, or streamer".to_string())
    } else {
        Ok(backends)
    }
}

#[cfg(target_os = "linux")]
fn command_in_path(program: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(program).is_file())
}

#[cfg(target_os = "linux")]
fn run_capture_stdout(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program} failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    if output.stdout.is_empty() {
        return Err(format!("{program} produced an empty camera frame"));
    }
    Ok(output.stdout)
}

#[cfg(target_os = "linux")]
fn run_capture_file(program: &str, args: &[&str], path: &Path) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("{program} failed: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_file(path);
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("read camera frame failed: {error}"))?;
    let _ = fs::remove_file(path);
    if bytes.is_empty() {
        return Err(format!("{program} produced an empty camera frame"));
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn sanitize_linux_field(value: &str) -> String {
    value.replace(['\r', '\n', '\t'], " ").trim().to_string()
}

#[cfg(target_os = "linux")]
fn temp_path(prefix: &str, ext: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}.{}",
        std::process::id(),
        rdl_protocol::now_epoch_ms(),
        ext
    ))
}
