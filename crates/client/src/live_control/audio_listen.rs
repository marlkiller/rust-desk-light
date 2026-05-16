use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::process::{Command, Stdio};
use std::sync::{mpsc::SyncSender, Arc, Mutex};

const MAX_AUDIO_BUFFER_MS: usize = 500;
const MIN_AUDIO_PREBUFFER_MS: usize = 80;

pub(crate) struct CapturedAudioFrame {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) struct AudioInputStream {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
    pub(crate) format: String,
    _stream: cpal::Stream,
}

pub(crate) struct AudioOutputPlayer {
    buffer: Arc<Mutex<AudioPlaybackState>>,
    output_sample_rate: u32,
    output_channels: u16,
    _stream: cpal::Stream,
}

struct AudioPlaybackState {
    samples: VecDeque<f32>,
    started: bool,
    prebuffer_samples: usize,
    max_samples: usize,
}

pub fn handle(payload: &str) -> String {
    let request = AudioListenRequest::parse(payload);
    match request.action.as_str() {
        "devices" => list_devices(request.scan_full),
        "stop" => "audio_listen_stopped\nmessage=stopped".to_string(),
        "start" | "" => {
            "audio_listen_ready\nmessage=use audio control stream to start listening".to_string()
        }
        _ => format!(
            "audio_listen_error\nmessage=unsupported action {}",
            request.action
        ),
    }
}

#[derive(Default)]
struct AudioListenRequest {
    action: String,
    scan_full: bool,
}

impl AudioListenRequest {
    fn parse(payload: &str) -> Self {
        let mut request = Self {
            action: "start".to_string(),
            scan_full: false,
        };
        for line in payload.lines() {
            if let Some(rest) = line.strip_prefix("action=") {
                request.action = rest.trim().to_ascii_lowercase();
            } else if let Some(rest) = line.strip_prefix("scan=") {
                request.scan_full = rest.trim().eq_ignore_ascii_case("full");
            }
        }
        request
    }
}

fn list_devices(scan_full: bool) -> String {
    match enumerate_input_devices(scan_full) {
        Ok(devices) => {
            let mut text = String::from("audio_listen_devices");
            for device in devices {
                text.push_str(&format!(
                    "\ndevice\t{}\t{}\t{}",
                    device.index,
                    sanitize_field(&device.name),
                    sanitize_field(&device.description)
                ));
            }
            text
        }
        Err(error) => format!("audio_listen_error\nmessage={error}"),
    }
}

pub(crate) fn confirm_audio_listen() -> Result<(), String> {
    let title = "Rust Desk Light";
    let message = "An administrator requested live microphone listening. Allow this session?";
    platform_confirm(title, message)
}

pub(crate) fn start_input_stream(
    device_index: usize,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<AudioInputStream, String> {
    let device = input_device(device_index)?;
    let supported_config = device
        .default_input_config()
        .map_err(|error| format!("default input config failed: {error}"))?;
    let sample_format = supported_config.sample_format();
    let config = supported_config.config();
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    let format = "pcm_s16le".to_string();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => build_f32_input_stream(&device, &config, frame_tx),
        cpal::SampleFormat::I16 => build_i16_input_stream(&device, &config, frame_tx),
        cpal::SampleFormat::U16 => build_u16_input_stream(&device, &config, frame_tx),
        other => Err(format!("unsupported input sample format: {other:?}")),
    }?;
    stream
        .play()
        .map_err(|error| format!("start input stream failed: {error}"))?;
    Ok(AudioInputStream {
        sample_rate,
        channels,
        format,
        _stream: stream,
    })
}

impl AudioOutputPlayer {
    pub(crate) fn start() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default audio output device found".to_string())?;
        let supported_config = device
            .default_output_config()
            .map_err(|error| format!("default output config failed: {error}"))?;
        let sample_format = supported_config.sample_format();
        let config = supported_config.config();
        let output_sample_rate = config.sample_rate.0;
        let output_channels = config.channels;
        let buffer = Arc::new(Mutex::new(AudioPlaybackState::new(
            output_sample_rate,
            output_channels,
        )));
        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_f32_output_stream(&device, &config, buffer.clone()),
            cpal::SampleFormat::I16 => build_i16_output_stream(&device, &config, buffer.clone()),
            cpal::SampleFormat::U16 => build_u16_output_stream(&device, &config, buffer.clone()),
            other => Err(format!("unsupported output sample format: {other:?}")),
        }?;
        stream
            .play()
            .map_err(|error| format!("start output stream failed: {error}"))?;
        Ok(Self {
            buffer,
            output_sample_rate,
            output_channels,
            _stream: stream,
        })
    }

    pub(crate) fn push_frame(
        &self,
        sample_rate: u32,
        channels: u16,
        format: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if format != "pcm_s16le" {
            return Err(format!("unsupported audio frame format: {format}"));
        }
        let samples = pcm_s16le_to_f32(bytes);
        let converted = resample_and_map_channels(
            &samples,
            sample_rate,
            channels,
            self.output_sample_rate,
            self.output_channels,
        );
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push_samples(converted);
        }
        Ok(())
    }
}

impl AudioPlaybackState {
    fn new(sample_rate: u32, channels: u16) -> Self {
        let samples_per_ms = sample_rate as usize * channels.max(1) as usize;
        Self {
            samples: VecDeque::new(),
            started: false,
            prebuffer_samples: (samples_per_ms * MIN_AUDIO_PREBUFFER_MS / 1000).max(1),
            max_samples: (samples_per_ms * MAX_AUDIO_BUFFER_MS / 1000).max(1),
        }
    }

    fn push_samples(&mut self, samples: Vec<f32>) {
        self.samples.extend(samples);
        while self.samples.len() > self.max_samples {
            let _ = self.samples.pop_front();
            self.started = true;
        }
    }

    fn next_sample(&mut self) -> f32 {
        if !self.started {
            if self.samples.len() >= self.prebuffer_samples {
                self.started = true;
            } else {
                return 0.0;
            }
        }
        match self.samples.pop_front() {
            Some(sample) => sample,
            None => {
                self.started = false;
                0.0
            }
        }
    }
}

fn build_f32_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    device
        .build_input_stream(
            config,
            move |data: &[f32], _| {
                let bytes = f32_to_pcm_s16(data);
                send_frame(&frame_tx, sample_rate, channels, bytes);
            },
            |error| eprintln!("audio input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

fn build_i16_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    device
        .build_input_stream(
            config,
            move |data: &[i16], _| {
                let bytes = i16_to_pcm_s16(data);
                send_frame(&frame_tx, sample_rate, channels, bytes);
            },
            |error| eprintln!("audio input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

fn build_u16_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    frame_tx: SyncSender<CapturedAudioFrame>,
) -> Result<cpal::Stream, String> {
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;
    device
        .build_input_stream(
            config,
            move |data: &[u16], _| {
                let bytes = u16_to_pcm_s16(data);
                send_frame(&frame_tx, sample_rate, channels, bytes);
            },
            |error| eprintln!("audio input stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build input stream failed: {error}"))
}

fn build_f32_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [f32], _| fill_f32_output(data, &buffer),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_i16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [i16], _| fill_i16_output(data, &buffer),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn build_u16_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<AudioPlaybackState>>,
) -> Result<cpal::Stream, String> {
    device
        .build_output_stream(
            config,
            move |data: &mut [u16], _| fill_u16_output(data, &buffer),
            |error| eprintln!("audio output stream error: {error}"),
            None,
        )
        .map_err(|error| format!("build output stream failed: {error}"))
}

fn fill_f32_output(data: &mut [f32], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            *sample = buffer.next_sample();
        }
    } else {
        data.fill(0.0);
    }
}

fn fill_i16_output(data: &mut [i16], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample = (value * i16::MAX as f32).round() as i16;
        }
    } else {
        data.fill(0);
    }
}

fn fill_u16_output(data: &mut [u16], buffer: &Arc<Mutex<AudioPlaybackState>>) {
    if let Ok(mut buffer) = buffer.lock() {
        for sample in data {
            let value = buffer.next_sample().clamp(-1.0, 1.0);
            *sample =
                ((value * i16::MAX as f32).round() as i32 + 32768).clamp(0, u16::MAX as i32) as u16;
        }
    } else {
        data.fill(32768);
    }
}

fn send_frame(
    frame_tx: &SyncSender<CapturedAudioFrame>,
    sample_rate: u32,
    channels: u16,
    bytes: Vec<u8>,
) {
    if bytes.is_empty() {
        return;
    }
    let _ = frame_tx.try_send(CapturedAudioFrame {
        sample_rate,
        channels,
        format: "pcm_s16le".to_string(),
        bytes,
    });
}

fn f32_to_pcm_s16(data: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 2);
    for sample in data {
        let sample = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn i16_to_pcm_s16(data: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 2);
    for sample in data {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn u16_to_pcm_s16(data: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(data.len() * 2);
    for sample in data {
        let centered = (*sample as i32 - 32768).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        bytes.extend_from_slice(&centered.to_le_bytes());
    }
    bytes
}

fn pcm_s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
        .collect()
}

fn resample_and_map_channels(
    input: &[f32],
    input_rate: u32,
    input_channels: u16,
    output_rate: u32,
    output_channels: u16,
) -> Vec<f32> {
    let input_channels = input_channels.max(1) as usize;
    let output_channels = output_channels.max(1) as usize;
    let input_frames = input.len() / input_channels;
    if input_frames == 0 || input_rate == 0 || output_rate == 0 {
        return Vec::new();
    }
    let output_frames =
        ((input_frames as f64 * output_rate as f64) / input_rate as f64).ceil() as usize;
    let mut output = Vec::with_capacity(output_frames * output_channels);
    let rate_ratio = input_rate as f64 / output_rate as f64;
    for output_frame in 0..output_frames {
        let source_pos = output_frame as f64 * rate_ratio;
        let input_frame = (source_pos.floor() as usize).min(input_frames - 1);
        let next_frame = input_frame.saturating_add(1).min(input_frames - 1);
        let mix = (source_pos - input_frame as f64) as f32;
        for output_channel in 0..output_channels {
            let current = mapped_channel_sample(input, input_frame, input_channels, output_channel);
            let next = mapped_channel_sample(input, next_frame, input_channels, output_channel);
            output.push(current + (next - current) * mix);
        }
    }
    output
}

fn mapped_channel_sample(
    input: &[f32],
    frame: usize,
    input_channels: usize,
    output_channel: usize,
) -> f32 {
    if input_channels == 1 {
        return input[frame * input_channels];
    }
    if output_channel < input_channels {
        return input[frame * input_channels + output_channel];
    }
    let start = frame * input_channels;
    let sum: f32 = input[start..start + input_channels].iter().copied().sum();
    sum / input_channels as f32
}

#[derive(Clone)]
struct AudioDeviceInfo {
    index: usize,
    name: String,
    description: String,
}

fn enumerate_input_devices(scan_full: bool) -> Result<Vec<AudioDeviceInfo>, String> {
    let host = cpal::default_host();
    let default_device = host.default_input_device();
    let default_name = default_device
        .as_ref()
        .and_then(|device| device.name().ok());
    let mut output = Vec::new();
    if let Some(device) = default_device {
        let name = default_name
            .clone()
            .unwrap_or_else(|| "Default input device".to_string());
        output.push(AudioDeviceInfo {
            index: 0,
            name,
            description: default_audio_config_label(&device, true),
        });
    }
    if default_device_fast_path(scan_full) && !output.is_empty() {
        return Ok(output);
    }
    let devices = host
        .input_devices()
        .map_err(|error| format!("list audio input devices failed: {error}"))?;
    for (device_number, device) in devices.enumerate() {
        let name = device
            .name()
            .unwrap_or_else(|_| format!("Input device {device_number}"));
        if default_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        output.push(AudioDeviceInfo {
            index: output.len(),
            name,
            description: default_audio_config_label(&device, false),
        });
    }
    if output.is_empty() {
        return Err("no audio input devices found".to_string());
    }
    Ok(output)
}

#[cfg(target_os = "macos")]
fn default_device_fast_path(scan_full: bool) -> bool {
    !scan_full
}

#[cfg(not(target_os = "macos"))]
fn default_device_fast_path(_scan_full: bool) -> bool {
    false
}

fn input_device(device_index: usize) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    let default_device = host.default_input_device();
    let has_default_device = default_device.is_some();
    let default_name = default_device
        .as_ref()
        .and_then(|device| device.name().ok());
    if device_index == 0 {
        if let Some(device) = default_device {
            return Ok(device);
        }
    }
    let devices = host
        .input_devices()
        .map_err(|error| format!("list audio input devices failed: {error}"))?;
    let mut index = if has_default_device { 1 } else { 0 };
    for device in devices {
        if let Ok(name) = device.name() {
            if default_name.as_deref() == Some(name.as_str()) {
                continue;
            }
        }
        if index == device_index {
            return Ok(device);
        }
        index += 1;
    }
    Err(format!(
        "audio input device index {device_index} is not available"
    ))
}

#[cfg(target_os = "macos")]
fn default_audio_config_label(_device: &cpal::Device, is_default: bool) -> String {
    if is_default {
        "default".to_string()
    } else {
        "input".to_string()
    }
}

#[cfg(not(target_os = "macos"))]
fn default_audio_config_label(device: &cpal::Device, is_default: bool) -> String {
    let label = device
        .default_input_config()
        .map(|config| {
            format!(
                "{} Hz, {} ch, {:?}",
                config.sample_rate().0,
                config.channels(),
                config.sample_format()
            )
        })
        .unwrap_or_default();
    if is_default {
        if label.is_empty() {
            "default".to_string()
        } else {
            format!("default, {label}")
        }
    } else {
        label
    }
}

fn sanitize_field(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

#[cfg(target_os = "windows")]
fn platform_confirm(title: &str, message: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONWARNING, MB_YESNO,
    };

    let title = wide_null(title);
    let message = wide_null(message);
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONWARNING,
        )
    };
    if result == IDYES {
        Ok(())
    } else {
        Err("audio listening was denied on the client".to_string())
    }
}

#[cfg(target_os = "macos")]
fn platform_confirm(title: &str, message: &str) -> Result<(), String> {
    let script = format!(
        "display dialog \"{}\" with title \"{}\" buttons {{\"Deny\", \"Allow\"}} default button \"Deny\" with icon caution",
        applescript_string(message),
        applescript_string(title)
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("osascript failed: {error}"))?;
    if output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("button returned:Allow")
    {
        Ok(())
    } else {
        Err("audio listening was denied on the client".to_string())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_confirm(title: &str, message: &str) -> Result<(), String> {
    run_first_success(&[
        (
            "zenity",
            vec!["--question", "--title", title, "--text", message],
        ),
        ("kdialog", vec!["--title", title, "--yesno", message]),
        (
            "xmessage",
            vec![
                "-center",
                "-title",
                title,
                "-buttons",
                "Allow:0,Deny:1",
                message,
            ],
        ),
    ])
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn platform_confirm(_title: &str, _message: &str) -> Result<(), String> {
    Err("audio listening confirmation is not supported on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_first_success(commands: &[(&str, Vec<&str>)]) -> Result<(), String> {
    let mut errors = Vec::new();
    for (program, args) in commands {
        match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => return Err("audio listening was denied on the client".to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                errors.push(format!("{program} was not found"));
            }
            Err(error) => return Err(format!("{program} failed: {error}")),
        }
    }
    Err(errors
        .last()
        .cloned()
        .unwrap_or_else(|| "no supported GUI command found".to_string()))
}
