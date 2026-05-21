use super::{encode_camera_bytes, CameraVideoFrame};
use libc::{c_int, c_ulong, c_void};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::ptr;
use std::time::Duration;

const V4L2_FRAME_TIMEOUT: Duration = Duration::from_millis(900);
const V4L2_BUFFER_COUNT: u32 = 4;
const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const V4L2_MEMORY_MMAP: u32 = 1;
const V4L2_FIELD_ANY: u32 = 0;
const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
const V4L2_CAP_STREAMING: u32 = 0x0400_0000;
const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const V4L2_PIX_FMT_MJPEG: u32 = fourcc(b"MJPG");
const V4L2_PIX_FMT_JPEG: u32 = fourcc(b"JPEG");
const V4L2_PIX_FMT_YUYV: u32 = fourcc(b"YUYV");

const VIDIOC_QUERYCAP: c_ulong = ioctl_read::<V4l2Capability>(b'V', 0);
const VIDIOC_S_FMT: c_ulong = ioctl_readwrite::<V4l2Format>(b'V', 5);
const VIDIOC_REQBUFS: c_ulong = ioctl_readwrite::<V4l2RequestBuffers>(b'V', 8);
const VIDIOC_QUERYBUF: c_ulong = ioctl_readwrite::<V4l2Buffer>(b'V', 9);
const VIDIOC_QBUF: c_ulong = ioctl_readwrite::<V4l2Buffer>(b'V', 15);
const VIDIOC_DQBUF: c_ulong = ioctl_readwrite::<V4l2Buffer>(b'V', 17);
const VIDIOC_STREAMON: c_ulong = ioctl_write::<u32>(b'V', 18);
const VIDIOC_STREAMOFF: c_ulong = ioctl_write::<u32>(b'V', 19);

pub(super) struct V4l2CameraStream {
    file: File,
    buffers: Vec<MappedBuffer>,
    format: V4l2FrameFormat,
    width: u32,
    height: u32,
    streaming: bool,
}

impl V4l2CameraStream {
    pub(super) fn open(device_path: &str, quality: &str) -> Result<Self, String> {
        let file = open_device(device_path)?;
        let fd = file.as_raw_fd();
        validate_capture_device(fd)?;
        let (target_width, target_height) = requested_dimensions(quality);
        let capture_format = set_capture_format(fd, target_width, target_height)?;
        let buffers = prepare_mmap_buffers(fd)?;
        let mut stream_type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe_ioctl_mut(fd, VIDIOC_STREAMON, &mut stream_type, "start v4l2 camera stream")?;
        Ok(Self {
            file,
            buffers,
            format: capture_format.format,
            width: capture_format.width,
            height: capture_format.height,
            streaming: true,
        })
    }

    pub(super) fn read_frame(&mut self, quality: &str) -> Result<CameraVideoFrame, String> {
        poll_frame(self.file.as_raw_fd())?;
        let mut buffer = new_buffer();
        unsafe_ioctl_mut(
            self.file.as_raw_fd(),
            VIDIOC_DQBUF,
            &mut buffer,
            "dequeue v4l2 camera buffer",
        )?;
        let index = buffer.index as usize;
        let bytes_used = buffer.bytesused as usize;
        let frame_bytes = match self.buffers.get(index) {
            Some(mapped) => match mapped.bytes(bytes_used) {
                Ok(bytes) => bytes.to_vec(),
                Err(error) => {
                    let _ = requeue_buffer(self.file.as_raw_fd(), &mut buffer);
                    return Err(error);
                }
            },
            None => {
                let _ = requeue_buffer(self.file.as_raw_fd(), &mut buffer);
                return Err(format!(
                    "v4l2 returned an invalid buffer index {}",
                    buffer.index
                ));
            }
        };
        requeue_buffer(self.file.as_raw_fd(), &mut buffer)?;
        encode_frame(self.format, self.width, self.height, frame_bytes, quality)
    }
}

fn requeue_buffer(fd: RawFd, buffer: &mut V4l2Buffer) -> Result<(), String> {
        unsafe_ioctl_mut(
            fd,
            VIDIOC_QBUF,
            buffer,
            "requeue v4l2 camera buffer",
        )
}

impl Drop for V4l2CameraStream {
    fn drop(&mut self) {
        if self.streaming {
            let mut stream_type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            let _ = unsafe_ioctl_mut(
                self.file.as_raw_fd(),
                VIDIOC_STREAMOFF,
                &mut stream_type,
                "stop v4l2 camera stream",
            );
            self.streaming = false;
        }
    }
}

struct CaptureFormat {
    format: V4l2FrameFormat,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
enum V4l2FrameFormat {
    Jpeg,
    Mjpeg,
    Yuyv,
}

struct MappedBuffer {
    ptr: *mut c_void,
    len: usize,
}

impl MappedBuffer {
    fn bytes(&self, bytes_used: usize) -> Result<&[u8], String> {
        if self.ptr.is_null() {
            return Err("v4l2 camera buffer is not mapped".to_string());
        }
        if bytes_used == 0 || bytes_used > self.len {
            return Err(format!(
                "v4l2 camera buffer has invalid payload size {bytes_used}"
            ));
        }
        Ok(unsafe { std::slice::from_raw_parts(self.ptr.cast::<u8>(), bytes_used) })
    }
}

impl Drop for MappedBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.len > 0 {
            unsafe {
                libc::munmap(self.ptr, self.len);
            }
        }
    }
}

fn open_device(device_path: &str) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(device_path)
        .map_err(|error| format!("open v4l2 camera {device_path} failed: {error}"))
}

fn validate_capture_device(fd: RawFd) -> Result<(), String> {
    let mut capability: V4l2Capability = unsafe { std::mem::zeroed() };
    unsafe_ioctl_mut(
        fd,
        VIDIOC_QUERYCAP,
        &mut capability,
        "query v4l2 camera capabilities",
    )?;
    let capabilities = if capability.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
        capability.device_caps
    } else {
        capability.capabilities
    };
    if capabilities & V4L2_CAP_VIDEO_CAPTURE == 0 {
        return Err("v4l2 device does not support single-plane video capture".to_string());
    }
    if capabilities & V4L2_CAP_STREAMING == 0 {
        return Err("v4l2 device does not support streaming mmap capture".to_string());
    }
    Ok(())
}

fn set_capture_format(fd: RawFd, width: u32, height: u32) -> Result<CaptureFormat, String> {
    let mut last_error = String::new();
    for pixel_format in [V4L2_PIX_FMT_MJPEG, V4L2_PIX_FMT_JPEG, V4L2_PIX_FMT_YUYV] {
        let mut format = V4l2Format::video_capture(width, height, pixel_format);
        match unsafe_ioctl_mut(fd, VIDIOC_S_FMT, &mut format, "set v4l2 camera format") {
            Ok(()) => {
                let pix = unsafe { format.fmt.pix };
                if let Some(format) = V4l2FrameFormat::from_fourcc(pix.pixelformat) {
                    return Ok(CaptureFormat {
                        format,
                        width: pix.width.max(1),
                        height: pix.height.max(1),
                    });
                }
                last_error = format!(
                    "v4l2 camera selected unsupported pixel format {}",
                    fourcc_label(pix.pixelformat)
                );
            }
            Err(error) => {
                last_error = error;
            }
        }
    }
    Err(if last_error.trim().is_empty() {
        "v4l2 camera did not accept MJPEG, JPEG, or YUYV capture".to_string()
    } else {
        last_error
    })
}

fn prepare_mmap_buffers(fd: RawFd) -> Result<Vec<MappedBuffer>, String> {
    let mut request = V4l2RequestBuffers {
        count: V4L2_BUFFER_COUNT,
        type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
        memory: V4L2_MEMORY_MMAP,
        reserved: [0; 2],
    };
    unsafe_ioctl_mut(
        fd,
        VIDIOC_REQBUFS,
        &mut request,
        "request v4l2 mmap buffers",
    )?;
    if request.count == 0 {
        return Err("v4l2 camera did not allocate mmap buffers".to_string());
    }

    let mut buffers = Vec::with_capacity(request.count as usize);
    for index in 0..request.count {
        let mut buffer = new_buffer();
        buffer.index = index;
        unsafe_ioctl_mut(
            fd,
            VIDIOC_QUERYBUF,
            &mut buffer,
            "query v4l2 mmap buffer",
        )?;
        let length = buffer.length as usize;
        let offset = unsafe { buffer.m.offset } as libc::off_t;
        if length == 0 {
            return Err(format!("v4l2 camera buffer {index} has zero length"));
        }
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(format!(
                "map v4l2 camera buffer failed: {}",
                io::Error::last_os_error()
            ));
        }
        buffers.push(MappedBuffer { ptr, len: length });
        unsafe_ioctl_mut(fd, VIDIOC_QBUF, &mut buffer, "queue v4l2 camera buffer")?;
    }
    Ok(buffers)
}

fn poll_frame(fd: RawFd) -> Result<(), String> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        pollfd.revents = 0;
        let result = unsafe {
            libc::poll(
                &mut pollfd,
                1,
                V4L2_FRAME_TIMEOUT.as_millis().min(c_int::MAX as u128) as c_int,
            )
        };
        if result > 0 {
            if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(format!("v4l2 camera poll failed: revents={}", pollfd.revents));
            }
            if pollfd.revents & libc::POLLIN != 0 {
                return Ok(());
            }
            continue;
        }
        if result == 0 {
            return Err("v4l2 camera timed out waiting for a frame".to_string());
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(format!("poll v4l2 camera failed: {error}"));
    }
}

fn encode_frame(
    format: V4l2FrameFormat,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    match format {
        V4l2FrameFormat::Jpeg | V4l2FrameFormat::Mjpeg => encode_camera_bytes(bytes, quality),
        V4l2FrameFormat::Yuyv => encode_yuyv_frame(width, height, &bytes, quality),
    }
}

fn encode_yuyv_frame(
    width: u32,
    height: u32,
    bytes: &[u8],
    quality: &str,
) -> Result<CameraVideoFrame, String> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "v4l2 camera frame dimensions are too large".to_string())?;
    let expected = pixel_count
        .checked_mul(2)
        .ok_or_else(|| "v4l2 camera frame is too large".to_string())?;
    if bytes.len() < expected {
        return Err(format!(
            "v4l2 YUYV frame is truncated: {} of {expected} bytes",
            bytes.len()
        ));
    }
    let mut rgb = Vec::with_capacity(pixel_count.saturating_mul(3));
    for chunk in bytes[..expected].chunks_exact(4) {
        let y0 = i32::from(chunk[0]);
        let u = i32::from(chunk[1]) - 128;
        let y1 = i32::from(chunk[2]);
        let v = i32::from(chunk[3]) - 128;
        push_yuv_as_rgb(&mut rgb, y0, u, v);
        push_yuv_as_rgb(&mut rgb, y1, u, v);
    }
    let image = image::RgbImage::from_raw(width, height, rgb)
        .map(image::DynamicImage::ImageRgb8)
        .ok_or_else(|| "build v4l2 RGB frame failed".to_string())?;
    let (bytes, width, height) = super::super::encode_camera_image(image, quality)
        .map_err(|error| format!("encode v4l2 camera frame failed: {error}"))?;
    Ok(CameraVideoFrame {
        width,
        height,
        format: "jpeg".to_string(),
        bytes,
    })
}

fn push_yuv_as_rgb(rgb: &mut Vec<u8>, y: i32, u: i32, v: i32) {
    let c = (y - 16).max(0);
    let r = (298 * c + 409 * v + 128) >> 8;
    let g = (298 * c - 100 * u - 208 * v + 128) >> 8;
    let b = (298 * c + 516 * u + 128) >> 8;
    rgb.push(clamp_u8(r));
    rgb.push(clamp_u8(g));
    rgb.push(clamp_u8(b));
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn new_buffer() -> V4l2Buffer {
    let mut buffer: V4l2Buffer = unsafe { std::mem::zeroed() };
    buffer.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    buffer.memory = V4L2_MEMORY_MMAP;
    buffer
}

fn requested_dimensions(quality: &str) -> (u32, u32) {
    match quality {
        "low" => (640, 480),
        "high" => (1920, 1080),
        _ => (1280, 720),
    }
}

fn unsafe_ioctl_mut<T>(
    fd: RawFd,
    request: c_ulong,
    value: &mut T,
    action: &str,
) -> Result<(), String> {
    let result = unsafe { libc::ioctl(fd, request, value as *mut T) };
    if result == -1 {
        Err(format!("{action} failed: {}", io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

impl V4l2FrameFormat {
    fn from_fourcc(value: u32) -> Option<Self> {
        match value {
            V4L2_PIX_FMT_JPEG => Some(Self::Jpeg),
            V4L2_PIX_FMT_MJPEG => Some(Self::Mjpeg),
            V4L2_PIX_FMT_YUYV => Some(Self::Yuyv),
            _ => None,
        }
    }
}

impl V4l2Format {
    fn video_capture(width: u32, height: u32, pixel_format: u32) -> Self {
        Self {
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            fmt: V4l2FormatUnion {
                pix: V4l2PixFormat {
                    width,
                    height,
                    pixelformat: pixel_format,
                    field: V4L2_FIELD_ANY,
                    bytesperline: 0,
                    sizeimage: 0,
                    colorspace: 0,
                    priv_: 0,
                    flags: 0,
                    ycbcr_enc: 0,
                    quantization: 0,
                    xfer_func: 0,
                },
            },
        }
    }
}

#[repr(C)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
union V4l2FormatUnion {
    pix: V4l2PixFormat,
    raw_data: [u8; 200],
}

#[repr(C)]
struct V4l2Format {
    type_: u32,
    fmt: V4l2FormatUnion,
}

#[repr(C)]
struct V4l2RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    reserved: [u32; 2],
}

#[repr(C)]
struct V4l2Timecode {
    type_: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[repr(C)]
union V4l2BufferMemory {
    offset: u32,
    userptr: c_ulong,
    planes: *mut c_void,
    fd: i32,
}

#[repr(C)]
union V4l2BufferRequest {
    request_fd: i32,
    reserved: u32,
}

#[repr(C)]
struct V4l2Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: V4l2Timecode,
    sequence: u32,
    memory: u32,
    m: V4l2BufferMemory,
    length: u32,
    reserved2: u32,
    request: V4l2BufferRequest,
}

const fn fourcc(value: &[u8; 4]) -> u32 {
    value[0] as u32
        | ((value[1] as u32) << 8)
        | ((value[2] as u32) << 16)
        | ((value[3] as u32) << 24)
}

fn fourcc_label(value: u32) -> String {
    let bytes = [
        (value & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        ((value >> 16) & 0xff) as u8,
        ((value >> 24) & 0xff) as u8,
    ];
    String::from_utf8_lossy(&bytes).to_string()
}

const fn ioctl_read<T>(type_: u8, nr: u8) -> c_ulong {
    ioctl_code(IOC_READ, type_, nr, std::mem::size_of::<T>())
}

const fn ioctl_write<T>(type_: u8, nr: u8) -> c_ulong {
    ioctl_code(IOC_WRITE, type_, nr, std::mem::size_of::<T>())
}

const fn ioctl_readwrite<T>(type_: u8, nr: u8) -> c_ulong {
    ioctl_code(IOC_READ | IOC_WRITE, type_, nr, std::mem::size_of::<T>())
}

const fn ioctl_code(dir: u32, type_: u8, nr: u8, size: usize) -> c_ulong {
    ((dir as c_ulong) << IOC_DIRSHIFT)
        | ((type_ as c_ulong) << IOC_TYPESHIFT)
        | ((nr as c_ulong) << IOC_NRSHIFT)
        | ((size as c_ulong) << IOC_SIZESHIFT)
}

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;
const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
