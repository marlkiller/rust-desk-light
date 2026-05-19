use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use image::DynamicImage;
#[cfg(target_os = "macos")]
use nokhwa::{nokhwa_check, nokhwa_initialize};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use nokhwa::{
    pixel_format::RgbFormat,
    query,
    utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType},
    Camera,
};
#[cfg(target_os = "linux")]
use std::env;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::{mpsc, Mutex, OnceLock};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::time::Duration;

pub(crate) struct CameraVideoFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct CameraCapture {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    device: usize,
    quality: String,
    #[cfg(target_os = "linux")]
    device_path: String,
    #[cfg(target_os = "linux")]
    backends: Vec<LinuxCameraBackend>,
    #[cfg(target_os = "linux")]
    active_backend: usize,
}

impl CameraCapture {
    pub(crate) fn new(device: usize, quality: &str) -> Result<Self, String> {
        let quality = normalize_quality(quality);
        #[cfg(target_os = "linux")]
        {
            return Ok(Self {
                quality,
                device_path: linux_device_path(device),
                backends: linux_camera_backends()?,
                active_backend: 0,
            });
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            return Ok(Self { device, quality });
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        {
            let _ = device;
            Ok(Self { quality })
        }
    }

    pub(crate) fn capture_frame(&mut self) -> Result<CameraVideoFrame, String> {
        #[cfg(target_os = "linux")]
        {
            return linux_capture_stream_frame(self);
        }
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            return capture_video_frame(self.device, &self.quality);
        }
        #[allow(unreachable_code)]
        Err("camera capture is not implemented for this platform".to_string())
    }
}

pub fn handle(payload: &str) -> String {
    let request = CameraRequest::parse(payload);
    match request.action.as_str() {
        "devices" => list_devices(),
        "stop" => stop_camera(),
        "capture" | "start" | "" => capture_default_camera(&request),
        _ => format!(
            "camera_error\nmessage=unsupported action {}",
            request.action
        ),
    }
}

#[derive(Default)]
struct CameraRequest {
    action: String,
    device: usize,
    quality: String,
}

impl CameraRequest {
    fn parse(payload: &str) -> Self {
        let mut request = Self {
            action: "capture".to_string(),
            device: 0,
            quality: "medium".to_string(),
        };
        for line in payload.lines() {
            if let Some(rest) = line.strip_prefix("action=") {
                request.action = rest.trim().to_ascii_lowercase();
            } else if let Some(rest) = line.strip_prefix("device=") {
                request.device = rest.trim().parse().unwrap_or_default();
            } else if let Some(rest) = line.strip_prefix("quality=") {
                request.quality = match rest.trim().to_ascii_lowercase().as_str() {
                    "low" => "low".to_string(),
                    "high" => "high".to_string(),
                    _ => "medium".to_string(),
                };
            }
        }
        request
    }
}

fn stop_camera() -> String {
    #[cfg(target_os = "windows")]
    {
        windows_stop_camera()
    }
    #[cfg(target_os = "macos")]
    {
        macos_stop_camera()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    "camera_stopped".to_string()
}

fn list_devices() -> String {
    #[cfg(target_os = "windows")]
    {
        return windows_list_devices();
    }
    #[cfg(target_os = "linux")]
    {
        return linux_list_devices();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_list_devices();
    }
    #[allow(unreachable_code)]
    "camera_error\nmessage=camera device listing is not implemented for this platform".to_string()
}

fn capture_default_camera(request: &CameraRequest) -> String {
    match capture_video_frame(request.device, &request.quality) {
        Ok(frame) => format_camera_frame_payload(request.device, frame),
        Err(error) => format!("camera_error\nmessage={error}"),
    }
}

pub(crate) fn capture_video_frame(
    device: usize,
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    let request = CameraRequest {
        action: "capture".to_string(),
        device,
        quality: normalize_quality(quality),
    };
    #[cfg(target_os = "linux")]
    {
        return linux_capture_frame(&request);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_capture_frame(&request);
    }
    #[cfg(target_os = "windows")]
    {
        return windows_capture_frame(&request);
    }
    #[allow(unreachable_code)]
    {
        let _ = request;
        Err("camera capture is not implemented for this platform".to_string())
    }
}

fn normalize_quality(value: &str) -> String {
    match value {
        "low" => "low".to_string(),
        "high" => "high".to_string(),
        _ => "medium".to_string(),
    }
}

fn format_camera_frame_payload(device: usize, frame: CameraVideoFrame) -> String {
    format!(
        "camera_frame\ndevice={}\nformat={}\nwidth={}\nheight={}\nbytes={}\nimage_base64={}",
        device,
        frame.format,
        frame.width,
        frame.height,
        frame.bytes.len(),
        base64::engine::general_purpose::STANDARD.encode(frame.bytes)
    )
}

#[cfg(target_os = "linux")]
fn linux_capture_frame(request: &CameraRequest) -> Result<CameraVideoFrame, String> {
    let mut capture = CameraCapture::new(request.device, &request.quality)?;
    capture.capture_frame()
}

#[cfg(target_os = "macos")]
fn macos_capture_frame(request: &CameraRequest) -> Result<CameraVideoFrame, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    let request_message = MacosCameraRequest::Capture {
        device: request.device,
        quality: request.quality.clone(),
        reply: reply_tx,
    };
    match send_macos_camera_request(request_message) {
        Ok(()) => reply_rx
            .recv()
            .unwrap_or_else(|error| Err(format!("camera worker stopped: {error}"))),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn linux_capture_stream_frame(capture: &mut CameraCapture) -> Result<CameraVideoFrame, String> {
    let mut last_error = String::new();
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
#[derive(Clone, Copy)]
enum LinuxCameraBackend {
    FfmpegStdout,
    FfmpegFile,
    FswebcamFile,
    StreamerFile,
}

#[cfg(target_os = "linux")]
impl LinuxCameraBackend {
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

#[cfg(target_os = "macos")]
fn macos_list_devices() -> String {
    let devices = match query(ApiBackend::AVFoundation) {
        Ok(devices) => devices,
        Err(error) => return format!("camera_error\nmessage=list camera devices failed: {error}"),
    };
    let mut text = String::from("camera_devices");
    for (index, device) in devices.iter().enumerate() {
        text.push_str(&format!(
            "\ndevice\t{}\t{}\t{}",
            index,
            sanitize_field(&device.human_name()),
            macos_device_description(device.description())
        ));
    }
    text
}

#[cfg(target_os = "macos")]
fn macos_device_description(description: &str) -> String {
    let description = sanitize_field(description);
    description
        .split_once(':')
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or(description)
}

#[cfg(target_os = "windows")]
fn windows_list_devices() -> String {
    let devices = match query(ApiBackend::MediaFoundation) {
        Ok(devices) => devices,
        Err(error) => return format!("camera_error\nmessage=list camera devices failed: {error}"),
    };
    let mut text = String::from("camera_devices");
    for (index, device) in devices.iter().enumerate() {
        text.push_str(&format!(
            "\ndevice\t{}\t{}\t{}",
            index,
            sanitize_field(&device.human_name()),
            sanitize_field(device.description())
        ));
    }
    text
}

#[cfg(target_os = "macos")]
fn macos_ensure_camera_permission() -> Result<(), String> {
    if nokhwa_check() {
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();
    nokhwa_initialize(move |granted| {
        let _ = tx.send(granted);
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(true) => Ok(()),
        Ok(false) => Err("camera permission denied; grant Camera permission to this app in macOS System Settings".to_string()),
        Err(_) => Err("camera permission request timed out; grant Camera permission to this app in macOS System Settings".to_string()),
    }
}

#[cfg(target_os = "macos")]
fn macos_stop_camera() -> String {
    let (reply_tx, reply_rx) = mpsc::channel();
    match send_macos_camera_request(MacosCameraRequest::Stop { reply: reply_tx }) {
        Ok(()) => reply_rx.recv().unwrap_or_else(|error| {
            format!("camera_error\nmessage=camera worker stopped: {error}")
        }),
        Err(error) => format!("camera_error\nmessage={error}"),
    }
}

#[cfg(target_os = "macos")]
struct MacosCameraSession {
    device: usize,
    camera: Camera,
}

#[cfg(target_os = "macos")]
enum MacosCameraRequest {
    Capture {
        device: usize,
        quality: String,
        reply: mpsc::Sender<Result<CameraVideoFrame, String>>,
    },
    Stop {
        reply: mpsc::Sender<String>,
    },
}

#[cfg(target_os = "macos")]
struct MacosCameraWorker {
    tx: mpsc::Sender<MacosCameraRequest>,
}

#[cfg(target_os = "macos")]
fn send_macos_camera_request(request: MacosCameraRequest) -> Result<(), String> {
    let tx = macos_camera_worker()?;
    if tx.send(request).is_ok() {
        return Ok(());
    }
    reset_macos_camera_worker();
    Err("camera worker stopped before accepting request".to_string())
}

#[cfg(target_os = "macos")]
fn macos_camera_worker() -> Result<mpsc::Sender<MacosCameraRequest>, String> {
    let mut guard = macos_camera_worker_slot()
        .lock()
        .map_err(|_| "camera worker lock is poisoned".to_string())?;
    if guard.is_none() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || macos_camera_worker_loop(rx));
        *guard = Some(MacosCameraWorker { tx });
    }
    guard
        .as_ref()
        .map(|worker| worker.tx.clone())
        .ok_or_else(|| "camera worker is not available".to_string())
}

#[cfg(target_os = "macos")]
fn reset_macos_camera_worker() {
    if let Ok(mut guard) = macos_camera_worker_slot().lock() {
        *guard = None;
    }
}

#[cfg(target_os = "macos")]
fn macos_camera_worker_slot() -> &'static Mutex<Option<MacosCameraWorker>> {
    static WORKER: OnceLock<Mutex<Option<MacosCameraWorker>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

#[cfg(target_os = "macos")]
fn macos_camera_worker_loop(rx: mpsc::Receiver<MacosCameraRequest>) {
    let mut session: Option<MacosCameraSession> = None;
    while let Ok(request) = rx.recv() {
        match request {
            MacosCameraRequest::Capture {
                device,
                quality,
                reply,
            } => {
                let _ = reply.send(macos_capture_on_worker_frame(
                    &mut session,
                    device,
                    &quality,
                ));
            }
            MacosCameraRequest::Stop { reply } => {
                session = None;
                let _ = reply.send("camera_stopped".to_string());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_capture_on_worker_frame(
    session: &mut Option<MacosCameraSession>,
    device: usize,
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    if session
        .as_ref()
        .is_none_or(|session| session.device != device)
    {
        match open_macos_camera_session(device) {
            Ok(new_session) => *session = Some(new_session),
            Err(error) => return Err(error),
        }
    }
    let Some(session) = session.as_mut() else {
        return Err("camera session is not available".to_string());
    };
    let frame = match session.camera.frame() {
        Ok(frame) => frame,
        Err(error) => return Err(format!("capture camera frame failed: {error}")),
    };
    let decoded = match frame.decode_image::<RgbFormat>() {
        Ok(decoded) => decoded,
        Err(error) => return Err(format!("decode camera frame failed: {error}")),
    };
    let image = DynamicImage::ImageRgb8(decoded);
    let (bytes, width, height) = match encode_camera_image(image, quality) {
        Ok(encoded) => encoded,
        Err(error) => return Err(format!("encode camera frame failed: {error}")),
    };
    Ok(CameraVideoFrame {
        width,
        height,
        format: "jpeg".to_string(),
        bytes,
    })
}

#[cfg(target_os = "macos")]
fn open_macos_camera_session(device: usize) -> Result<MacosCameraSession, String> {
    macos_ensure_camera_permission()?;
    let camera_index = macos_camera_index(device)?;
    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = Camera::with_backend(camera_index, requested, ApiBackend::AVFoundation)
        .map_err(|error| format!("open camera failed: {error}"))?;
    camera
        .open_stream()
        .map_err(|error| format!("open camera stream failed: {error}"))?;

    std::thread::sleep(Duration::from_millis(220));
    for _ in 0..5 {
        let _ = camera.frame();
        std::thread::sleep(Duration::from_millis(30));
    }

    Ok(MacosCameraSession { device, camera })
}

#[cfg(target_os = "macos")]
fn macos_camera_index(device_index: usize) -> Result<CameraIndex, String> {
    let devices = query(ApiBackend::AVFoundation)
        .map_err(|error| format!("list camera devices failed: {error}"))?;
    let Some(device) = devices.get(device_index).or_else(|| devices.first()) else {
        return Err("no macOS camera device found".to_string());
    };
    Ok(device.index().clone())
}

#[cfg(target_os = "windows")]
fn windows_capture_frame(request: &CameraRequest) -> Result<CameraVideoFrame, String> {
    let (reply_tx, reply_rx) = mpsc::channel();
    let request_message = WindowsCameraRequest::Capture {
        device: request.device,
        quality: request.quality.clone(),
        reply: reply_tx,
    };
    match send_windows_camera_request(request_message) {
        Ok(()) => reply_rx
            .recv()
            .unwrap_or_else(|error| Err(format!("camera worker stopped: {error}"))),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "windows")]
fn windows_stop_camera() -> String {
    let (reply_tx, reply_rx) = mpsc::channel();
    match send_windows_camera_request(WindowsCameraRequest::Stop { reply: reply_tx }) {
        Ok(()) => reply_rx.recv().unwrap_or_else(|error| {
            format!("camera_error\nmessage=camera worker stopped: {error}")
        }),
        Err(error) => format!("camera_error\nmessage={error}"),
    }
}

#[cfg(target_os = "windows")]
struct WindowsCameraSession {
    device: usize,
    camera: Camera,
}

#[cfg(target_os = "windows")]
enum WindowsCameraRequest {
    Capture {
        device: usize,
        quality: String,
        reply: mpsc::Sender<Result<CameraVideoFrame, String>>,
    },
    Stop {
        reply: mpsc::Sender<String>,
    },
}

#[cfg(target_os = "windows")]
struct WindowsCameraWorker {
    tx: mpsc::Sender<WindowsCameraRequest>,
}

#[cfg(target_os = "windows")]
fn send_windows_camera_request(request: WindowsCameraRequest) -> Result<(), String> {
    let tx = windows_camera_worker()?;
    if tx.send(request).is_ok() {
        return Ok(());
    }
    reset_windows_camera_worker();
    Err("camera worker stopped before accepting request".to_string())
}

#[cfg(target_os = "windows")]
fn windows_camera_index(device_index: usize) -> Result<CameraIndex, String> {
    let devices = query(ApiBackend::MediaFoundation)
        .map_err(|error| format!("list camera devices failed: {error}"))?;
    let Some(device) = devices.get(device_index).or_else(|| devices.first()) else {
        return Err("no Windows camera device found".to_string());
    };
    Ok(device.index().clone())
}

#[cfg(target_os = "windows")]
fn windows_camera_worker() -> Result<mpsc::Sender<WindowsCameraRequest>, String> {
    let mut guard = windows_camera_worker_slot()
        .lock()
        .map_err(|_| "camera worker lock is poisoned".to_string())?;
    if guard.is_none() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || windows_camera_worker_loop(rx));
        *guard = Some(WindowsCameraWorker { tx });
    }
    Ok(guard.as_ref().expect("worker initialized").tx.clone())
}

#[cfg(target_os = "windows")]
fn reset_windows_camera_worker() {
    if let Ok(mut guard) = windows_camera_worker_slot().lock() {
        *guard = None;
    }
}

#[cfg(target_os = "windows")]
fn windows_camera_worker_slot() -> &'static Mutex<Option<WindowsCameraWorker>> {
    static WORKER: OnceLock<Mutex<Option<WindowsCameraWorker>>> = OnceLock::new();
    WORKER.get_or_init(|| Mutex::new(None))
}

#[cfg(target_os = "windows")]
fn windows_camera_worker_loop(rx: mpsc::Receiver<WindowsCameraRequest>) {
    let mut session: Option<WindowsCameraSession> = None;
    for request in rx {
        match request {
            WindowsCameraRequest::Capture {
                device,
                quality,
                reply,
            } => {
                let result = windows_capture_on_worker_frame(&mut session, device, &quality);
                let _ = reply.send(result);
            }
            WindowsCameraRequest::Stop { reply } => {
                session = None;
                let _ = reply.send("camera_stopped".to_string());
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_capture_on_worker_frame(
    session: &mut Option<WindowsCameraSession>,
    device: usize,
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    if session
        .as_ref()
        .is_none_or(|session| session.device != device)
    {
        match open_windows_camera_session(device) {
            Ok(new_session) => *session = Some(new_session),
            Err(error) => return Err(error),
        }
    }
    let Some(session) = session.as_mut() else {
        return Err("camera session is not available".to_string());
    };
    let frame = match session.camera.frame() {
        Ok(frame) => frame,
        Err(error) => return Err(format!("capture camera frame failed: {error}")),
    };
    let decoded = match frame.decode_image::<RgbFormat>() {
        Ok(decoded) => decoded,
        Err(error) => return Err(format!("decode camera frame failed: {error}")),
    };
    let image = DynamicImage::ImageRgb8(decoded);
    let (bytes, width, height) = match encode_camera_image(image, quality) {
        Ok(encoded) => encoded,
        Err(error) => return Err(format!("encode camera frame failed: {error}")),
    };
    Ok(CameraVideoFrame {
        width,
        height,
        format: "jpeg".to_string(),
        bytes,
    })
}

#[cfg(target_os = "windows")]
fn open_windows_camera_session(device: usize) -> Result<WindowsCameraSession, String> {
    let camera_index = windows_camera_index(device)?;
    let requested =
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
    let mut camera = Camera::new(camera_index, requested)
        .map_err(|error| format!("open camera failed: {error}"))?;
    camera
        .open_stream()
        .map_err(|error| format!("open camera stream failed: {error}"))?;

    std::thread::sleep(Duration::from_millis(220));
    for _ in 0..5 {
        let _ = camera.frame();
        std::thread::sleep(Duration::from_millis(30));
    }

    Ok(WindowsCameraSession { device, camera })
}

fn encode_camera_image(
    image: image::DynamicImage,
    quality: &str,
) -> Result<(Vec<u8>, u32, u32), image::ImageError> {
    let (max_width, jpeg_quality) = quality_profile(quality);
    let image = if image.width() > max_width {
        image.resize(max_width, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let width = image.width();
    let height = image.height();
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, jpeg_quality);
    encoder.encode_image(&image)?;
    Ok((bytes, width, height))
}

fn quality_profile(value: &str) -> (u32, u8) {
    match value {
        "low" => (640, 42),
        "high" => (1920, 88),
        _ => (1280, 72),
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn sanitize_field(value: &str) -> String {
    value.replace(['\r', '\n', '\t'], " ").trim().to_string()
}

#[cfg(target_os = "linux")]
fn linux_list_devices() -> String {
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
fn linux_device_path(device: usize) -> String {
    let path = format!("/dev/video{device}");
    if Path::new(&path).exists() {
        path
    } else {
        "/dev/video0".to_string()
    }
}

#[cfg(target_os = "linux")]
fn linux_camera_backends() -> Result<Vec<LinuxCameraBackend>, String> {
    let mut backends = Vec::new();
    if command_in_path("ffmpeg") {
        backends.push(LinuxCameraBackend::FfmpegStdout);
        backends.push(LinuxCameraBackend::FfmpegFile);
    }
    if command_in_path("fswebcam") {
        backends.push(LinuxCameraBackend::FswebcamFile);
    }
    if command_in_path("streamer") {
        backends.push(LinuxCameraBackend::StreamerFile);
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
