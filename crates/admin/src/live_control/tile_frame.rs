pub(crate) const FORMAT: &str = "jpeg_tiles_v1";

const MAGIC: &[u8; 4] = b"RDT1";
const FRAME_TYPE_KEYFRAME: u8 = 1;
const FRAME_TYPE_DELTA: u8 = 2;

#[derive(Default)]
pub(crate) struct DecodeState {
    base: Option<TileFrameBase>,
}

impl DecodeState {
    pub(crate) fn reset(&mut self) {
        self.base = None;
    }
}

struct TileFrameBase {
    base_id: u64,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

pub(crate) struct DecodedFrame {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) fn decode_frame(
    state: &mut DecodeState,
    image_width: u32,
    image_height: u32,
    bytes: &[u8],
) -> Result<Option<DecodedFrame>, String> {
    let payload = parse_payload(bytes)?;
    if payload.width != image_width || payload.height != image_height {
        return Err("tile frame metadata does not match payload size".to_string());
    }
    let mut rgba = match payload.frame_type {
        FRAME_TYPE_KEYFRAME => {
            if payload.tiles.len()
                != expected_tile_count(payload.width, payload.height, payload.tile_size)?
            {
                return Err("tile keyframe does not cover the full frame".to_string());
            }
            vec![0; rgba_len(payload.width, payload.height)?]
        }
        FRAME_TYPE_DELTA => {
            let Some(base) = state.base.as_ref() else {
                return Ok(None);
            };
            if base.base_id != payload.base_id
                || base.width != payload.width
                || base.height != payload.height
            {
                return Ok(None);
            }
            base.rgba.clone()
        }
        _ => return Err("unsupported tile frame type".to_string()),
    };

    for tile in &payload.tiles {
        let tile_image = image::load_from_memory(tile.bytes)
            .map_err(|error| format!("load tile failed: {error}"))?
            .to_rgba8();
        if tile_image.width() != tile.width || tile_image.height() != tile.height {
            return Err("decoded tile size does not match payload metadata".to_string());
        }
        patch_rgba_tile(
            &mut rgba,
            payload.width,
            tile.x,
            tile.y,
            tile.width,
            tile.height,
            tile_image.as_raw(),
        )?;
    }

    if payload.frame_type == FRAME_TYPE_KEYFRAME {
        state.base = Some(TileFrameBase {
            base_id: payload.base_id,
            width: payload.width,
            height: payload.height,
            rgba: rgba.clone(),
        });
    }

    Ok(Some(DecodedFrame {
        width: payload.width,
        height: payload.height,
        rgba,
    }))
}

struct TileFramePayload<'a> {
    frame_type: u8,
    base_id: u64,
    tile_size: u32,
    width: u32,
    height: u32,
    tiles: Vec<TilePatch<'a>>,
}

struct TilePatch<'a> {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bytes: &'a [u8],
}

fn parse_payload(bytes: &[u8]) -> Result<TileFramePayload<'_>, String> {
    let mut reader = TilePayloadReader::new(bytes);
    if reader.read_bytes(MAGIC.len())? != MAGIC.as_slice() {
        return Err("invalid tile frame magic".to_string());
    }
    let frame_type = reader.read_u8()?;
    if frame_type != FRAME_TYPE_KEYFRAME && frame_type != FRAME_TYPE_DELTA {
        return Err("unsupported tile frame type".to_string());
    }
    let base_id = reader.read_u64()?;
    let tile_size = u32::from(reader.read_u16()?);
    if tile_size == 0 {
        return Err("invalid tile size".to_string());
    }
    let width = reader.read_u32()?;
    let height = reader.read_u32()?;
    if width == 0 || height == 0 {
        return Err("invalid tile frame size".to_string());
    }
    let tile_count = reader.read_u32()? as usize;
    let max_tiles = expected_tile_count(width, height, tile_size)?;
    if tile_count > max_tiles {
        return Err("tile frame contains too many tiles".to_string());
    }

    let mut tiles = Vec::with_capacity(tile_count);
    for _ in 0..tile_count {
        let x = reader.read_u32()?;
        let y = reader.read_u32()?;
        let tile_width = reader.read_u32()?;
        let tile_height = reader.read_u32()?;
        let len = reader.read_u32()? as usize;
        if tile_width == 0
            || tile_height == 0
            || x >= width
            || y >= height
            || x.saturating_add(tile_width) > width
            || y.saturating_add(tile_height) > height
            || tile_width > tile_size
            || tile_height > tile_size
        {
            return Err("invalid tile bounds".to_string());
        }
        let bytes = reader.read_bytes(len)?;
        if bytes.is_empty() {
            return Err("empty tile payload".to_string());
        }
        tiles.push(TilePatch {
            x,
            y,
            width: tile_width,
            height: tile_height,
            bytes,
        });
    }
    if !reader.is_finished() {
        return Err("tile frame has trailing bytes".to_string());
    }
    Ok(TileFramePayload {
        frame_type,
        base_id,
        tile_size,
        width,
        height,
        tiles,
    })
}

struct TilePayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> TilePayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.read_bytes(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| "tile frame is too large".to_string())?;
        if end > self.bytes.len() {
            return Err("truncated tile frame".to_string());
        }
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }

    fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

fn patch_rgba_tile(
    frame: &mut [u8],
    frame_width: u32,
    x: u32,
    y: u32,
    tile_width: u32,
    tile_height: u32,
    tile_rgba: &[u8],
) -> Result<(), String> {
    let row_len = tile_width as usize * 4;
    let expected_tile_len = row_len
        .checked_mul(tile_height as usize)
        .ok_or_else(|| "tile is too large".to_string())?;
    if tile_rgba.len() != expected_tile_len {
        return Err("decoded tile buffer has invalid size".to_string());
    }
    for row in 0..tile_height {
        let frame_start = ((y + row) as usize * frame_width as usize + x as usize) * 4;
        let frame_end = frame_start + row_len;
        let tile_start = row as usize * row_len;
        let tile_end = tile_start + row_len;
        frame[frame_start..frame_end].copy_from_slice(&tile_rgba[tile_start..tile_end]);
    }
    Ok(())
}

fn expected_tile_count(width: u32, height: u32, tile_size: u32) -> Result<usize, String> {
    if tile_size == 0 {
        return Err("invalid tile size".to_string());
    }
    let columns = width
        .checked_add(tile_size - 1)
        .ok_or_else(|| "tile frame is too large".to_string())?
        / tile_size;
    let rows = height
        .checked_add(tile_size - 1)
        .ok_or_else(|| "tile frame is too large".to_string())?
        / tile_size;
    columns
        .checked_mul(rows)
        .map(|value| value as usize)
        .ok_or_else(|| "tile frame is too large".to_string())
}

fn rgba_len(width: u32, height: u32) -> Result<usize, String> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|value| value as usize)
        .ok_or_else(|| "tile frame is too large".to_string())
}
