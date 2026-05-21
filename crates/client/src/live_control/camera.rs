use base64::Engine;
use image::codecs::jpeg::JpegEncoder;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

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
    backends: Vec<linux::CameraBackend>,
    #[cfg(target_os = "linux")]
    active_backend: usize,
    #[cfg(target_os = "linux")]
    ffmpeg_stream: Option<linux::FfmpegMjpegStream>,
    #[cfg(target_os = "linux")]
    ffmpeg_stream_failed: bool,
}

impl CameraCapture {
    pub(crate) fn new(device: usize, quality: &str) -> Result<Self, String> {
        let quality = normalize_quality(quality);
        #[cfg(target_os = "linux")]
        {
            return Ok(Self {
                quality,
                device_path: linux::device_path(device),
                backends: linux::camera_backends()?,
                active_backend: 0,
                ffmpeg_stream: None,
                ffmpeg_stream_failed: false,
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
            return linux::capture_stream_frame(self);
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
        windows::stop_camera()
    }
    #[cfg(target_os = "macos")]
    {
        macos::stop_camera()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    "camera_stopped".to_string()
}

fn list_devices() -> String {
    #[cfg(target_os = "windows")]
    {
        return windows::list_devices();
    }
    #[cfg(target_os = "linux")]
    {
        return linux::list_devices();
    }
    #[cfg(target_os = "macos")]
    {
        return macos::list_devices();
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
        return linux::capture_frame(&request);
    }
    #[cfg(target_os = "macos")]
    {
        return macos::capture_frame(&request);
    }
    #[cfg(target_os = "windows")]
    {
        return windows::capture_frame(&request);
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
