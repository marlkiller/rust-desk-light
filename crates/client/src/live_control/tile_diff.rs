use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbImage};
use std::time::{Duration, Instant};

pub(super) const FORMAT: &str = "jpeg_tiles_v1";

const MAGIC: &[u8; 4] = b"RDT1";
const FRAME_TYPE_KEYFRAME: u8 = 1;
const FRAME_TYPE_DELTA: u8 = 2;
const DEFAULT_TILE_SIZE: u32 = 128;
const KEYFRAME_INTERVAL: Duration = Duration::from_secs(5);
const KEYFRAME_BURST_FRAMES: u8 = 2;
const DELTA_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct TileDiffEncoder {
    enabled: bool,
    tile_size: u32,
    base_id: u64,
    base_rgb: Vec<u8>,
    base_width: u32,
    base_height: u32,
    last_keyframe_at: Option<Instant>,
    keyframe_burst_remaining: u8,
    last_sent_rgb: Vec<u8>,
    last_sent_at: Option<Instant>,
}

impl TileDiffEncoder {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            tile_size: DEFAULT_TILE_SIZE,
            base_id: 0,
            base_rgb: Vec::new(),
            base_width: 0,
            base_height: 0,
            last_keyframe_at: None,
            keyframe_burst_remaining: 0,
            last_sent_rgb: Vec::new(),
            last_sent_at: None,
        }
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn encode_rgb_frame(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        quality: u8,
    ) -> Result<Option<Vec<u8>>, String> {
        validate_rgb_frame(rgb, width, height)?;
        let now = Instant::now();
        if let Some(reason) = self.keyframe_reason(width, height, now) {
            return self.encode_keyframe(rgb, width, height, quality, now, reason);
        }

        let changed_since_last_send = self.last_sent_rgb.as_slice() != rgb;
        let refresh_due = self
            .last_sent_at
            .map(|last| last.elapsed() >= DELTA_REFRESH_INTERVAL)
            .unwrap_or(true);
        if !changed_since_last_send && !refresh_due {
            return Ok(None);
        }

        let tiles = collect_delta_tiles(
            rgb,
            &self.base_rgb,
            width,
            height,
            self.tile_size,
            quality,
        )?;
        let payload = encode_payload(
            FRAME_TYPE_DELTA,
            self.base_id,
            self.tile_size,
            width,
            height,
            &tiles,
        )?;
        self.last_sent_rgb.clear();
        self.last_sent_rgb.extend_from_slice(rgb);
        self.last_sent_at = Some(now);
        Ok(Some(payload))
    }

    fn keyframe_reason(&self, width: u32, height: u32, now: Instant) -> Option<KeyframeReason> {
        if self.base_rgb.is_empty()
            || self.base_width != width
            || self.base_height != height
            || self
                .last_keyframe_at
                .map(|last| now.duration_since(last) >= KEYFRAME_INTERVAL)
                .unwrap_or(true)
        {
            return Some(KeyframeReason::Reset);
        }
        if self.keyframe_burst_remaining > 0 {
            return Some(KeyframeReason::Burst);
        }
        None
    }

    fn encode_keyframe(
        &mut self,
        rgb: &[u8],
        width: u32,
        height: u32,
        quality: u8,
        now: Instant,
        reason: KeyframeReason,
    ) -> Result<Option<Vec<u8>>, String> {
        self.base_id = next_base_id(self.base_id);
        let tiles = collect_keyframe_tiles(rgb, width, height, self.tile_size, quality)?;
        let payload = encode_payload(
            FRAME_TYPE_KEYFRAME,
            self.base_id,
            self.tile_size,
            width,
            height,
            &tiles,
        )?;
        self.base_rgb.clear();
        self.base_rgb.extend_from_slice(rgb);
        self.base_width = width;
        self.base_height = height;
        self.last_keyframe_at = Some(now);
        match reason {
            KeyframeReason::Reset => {
                self.keyframe_burst_remaining = KEYFRAME_BURST_FRAMES;
            }
            KeyframeReason::Burst => {
                self.keyframe_burst_remaining = self.keyframe_burst_remaining.saturating_sub(1);
            }
        }
        self.last_sent_rgb.clear();
        self.last_sent_rgb.extend_from_slice(rgb);
        self.last_sent_at = Some(now);
        Ok(Some(payload))
    }
}

#[derive(Clone, Copy)]
enum KeyframeReason {
    Reset,
    Burst,
}

struct EncodedTile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

fn collect_keyframe_tiles(
    rgb: &[u8],
    width: u32,
    height: u32,
    tile_size: u32,
    quality: u8,
) -> Result<Vec<EncodedTile>, String> {
    collect_tiles(rgb, None, width, height, tile_size, quality)
}

fn collect_delta_tiles(
    rgb: &[u8],
    base_rgb: &[u8],
    width: u32,
    height: u32,
    tile_size: u32,
    quality: u8,
) -> Result<Vec<EncodedTile>, String> {
    collect_tiles(rgb, Some(base_rgb), width, height, tile_size, quality)
}

fn collect_tiles(
    rgb: &[u8],
    base_rgb: Option<&[u8]>,
    width: u32,
    height: u32,
    tile_size: u32,
    quality: u8,
) -> Result<Vec<EncodedTile>, String> {
    if tile_size == 0 {
        return Err("tile size is invalid".to_string());
    }
    if let Some(base_rgb) = base_rgb {
        validate_rgb_frame(base_rgb, width, height)?;
    }
    let mut tiles = Vec::new();
    let mut y = 0;
    while y < height {
        let tile_height = tile_size.min(height - y);
        let mut x = 0;
        while x < width {
            let tile_width = tile_size.min(width - x);
            let changed = base_rgb
                .map(|base| tile_differs(rgb, base, width, x, y, tile_width, tile_height))
                .unwrap_or(true);
            if changed {
                let tile_rgb = copy_tile_rgb(rgb, width, x, y, tile_width, tile_height)?;
                tiles.push(EncodedTile {
                    x,
                    y,
                    width: tile_width,
                    height: tile_height,
                    bytes: encode_tile_jpeg(tile_rgb, tile_width, tile_height, quality)?,
                });
            }
            x += tile_size;
        }
        y += tile_size;
    }
    Ok(tiles)
}

fn tile_differs(
    rgb: &[u8],
    base_rgb: &[u8],
    frame_width: u32,
    x: u32,
    y: u32,
    tile_width: u32,
    tile_height: u32,
) -> bool {
    let row_len = tile_width as usize * 3;
    for row in 0..tile_height {
        let start = ((y + row) as usize * frame_width as usize + x as usize) * 3;
        let end = start + row_len;
        if rgb[start..end] != base_rgb[start..end] {
            return true;
        }
    }
    false
}

fn copy_tile_rgb(
    rgb: &[u8],
    frame_width: u32,
    x: u32,
    y: u32,
    tile_width: u32,
    tile_height: u32,
) -> Result<Vec<u8>, String> {
    let row_len = tile_width as usize * 3;
    let mut tile = Vec::with_capacity(
        tile_width
            .checked_mul(tile_height)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| "tile is too large".to_string())? as usize,
    );
    for row in 0..tile_height {
        let start = ((y + row) as usize * frame_width as usize + x as usize) * 3;
        let end = start + row_len;
        tile.extend_from_slice(&rgb[start..end]);
    }
    Ok(tile)
}

fn encode_tile_jpeg(
    rgb: Vec<u8>,
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let image = RgbImage::from_raw(width, height, rgb)
        .ok_or_else(|| "tile buffer has invalid size".to_string())?;
    let image = DynamicImage::ImageRgb8(image);
    let mut encoded = Vec::new();
    JpegEncoder::new_with_quality(&mut encoded, quality)
        .encode_image(&image)
        .map_err(|error| format!("jpeg tile encode failed: {error}"))?;
    Ok(encoded)
}

fn encode_payload(
    frame_type: u8,
    base_id: u64,
    tile_size: u32,
    width: u32,
    height: u32,
    tiles: &[EncodedTile],
) -> Result<Vec<u8>, String> {
    let tile_size = u16::try_from(tile_size).map_err(|_| "tile size is too large".to_string())?;
    let tile_count =
        u32::try_from(tiles.len()).map_err(|_| "tile count is too large".to_string())?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.push(frame_type);
    bytes.extend_from_slice(&base_id.to_be_bytes());
    bytes.extend_from_slice(&tile_size.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&tile_count.to_be_bytes());
    for tile in tiles {
        bytes.extend_from_slice(&tile.x.to_be_bytes());
        bytes.extend_from_slice(&tile.y.to_be_bytes());
        bytes.extend_from_slice(&tile.width.to_be_bytes());
        bytes.extend_from_slice(&tile.height.to_be_bytes());
        let len = u32::try_from(tile.bytes.len())
            .map_err(|_| "encoded tile is too large".to_string())?;
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(&tile.bytes);
    }
    Ok(bytes)
}

fn validate_rgb_frame(rgb: &[u8], width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("captured frame has invalid size".to_string());
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| "captured frame is too large".to_string())? as usize;
    if rgb.len() != expected {
        return Err("captured RGB buffer has invalid size".to_string());
    }
    Ok(())
}

fn next_base_id(current: u64) -> u64 {
    let next = current.wrapping_add(1);
    if next == 0 {
        1
    } else {
        next
    }
}
