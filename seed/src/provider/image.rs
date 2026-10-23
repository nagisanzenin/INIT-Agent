//! Image token estimate (body/provider.md §4.1; log-context X4 `tokens_est`): dimensions from the
//! image header (PNG IHDR, GIF, WebP VP8/VP8L/VP8X, JPEG SOF0/SOF2), std only, no decoder.

use crate::model::Api;

/// log-context's default when no formula is published (DeepSeek, OpenAI) or the header is unknown.
pub const DEFAULT_IMAGE_TOKENS: u64 = 1_600;
/// Anthropic scales the long edge down to this before counting (UNVERIFIED against current docs).
pub const ANTHROPIC_LONG_EDGE: u64 = 1_568;

fn be16(b: &[u8], i: usize) -> Option<u64> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u64)
}
fn le16(b: &[u8], i: usize) -> Option<u64> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u64)
}
fn be32(b: &[u8], i: usize) -> Option<u64> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as u64)
}
fn le24(b: &[u8], i: usize) -> Option<u64> {
    let s = b.get(i..i + 3)?;
    Some(s[0] as u64 | (s[1] as u64) << 8 | (s[2] as u64) << 16)
}

/// `(width, height)` from the header bytes, or None.
pub fn dimensions(b: &[u8]) -> Option<(u64, u64)> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        return Some((be32(b, 16)?, be32(b, 20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some((le16(b, 6)?, le16(b, 8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        return match b.get(12..16)? {
            b"VP8 " => Some((le16(b, 26)? & 0x3fff, le16(b, 28)? & 0x3fff)),
            b"VP8L" => {
                let s = b.get(21..25)?;
                let w = 1 + (s[0] as u64 | ((s[1] as u64 & 0x3f) << 8));
                let h =
                    1 + ((s[1] as u64 >> 6) | (s[2] as u64) << 2 | ((s[3] as u64 & 0x0f) << 10));
                Some((w, h))
            }
            b"VP8X" => Some((le24(b, 24)? + 1, le24(b, 27)? + 1)),
            _ => None,
        };
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 4 <= b.len() {
            if b[i] != 0xFF {
                return None;
            }
            let marker = b[i + 1];
            if marker == 0xFF {
                i += 1; // fill byte
                continue;
            }
            if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
                i += 2; // no length
                continue;
            }
            let len = be16(b, i + 2)? as usize;
            if matches!(marker, 0xC0 | 0xC2) {
                return Some((be16(b, i + 7)?, be16(b, i + 5)?));
            }
            if len < 2 {
                return None;
            }
            i += 2 + len;
        }
    }
    None
}

/// `tokens_est` for one image sent to `api`.
pub fn image_tokens(api: Api, bytes: &[u8]) -> u64 {
    match (api, dimensions(bytes)) {
        (Api::Messages, Some((w, h))) if w > 0 && h > 0 => {
            let long = w.max(h);
            let (w, h) = if long > ANTHROPIC_LONG_EDGE {
                (
                    w * ANTHROPIC_LONG_EDGE / long,
                    h * ANTHROPIC_LONG_EDGE / long,
                )
            } else {
                (w, h)
            };
            (w.max(1) * h.max(1)).div_ceil(750)
        }
        _ => DEFAULT_IMAGE_TOKENS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0]);
        b
    }

    #[test]
    fn headers() {
        assert_eq!(dimensions(&png(640, 480)), Some((640, 480)));
        assert_eq!(dimensions(b"GIF89a\x40\x01\xf0\x00rest"), Some((320, 240)));
        // JPEG: SOI, APP0 (len 16), SOF0 with height 0x01E0 = 480 and width 0x0280 = 640.
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        j.extend_from_slice(&[0; 14]);
        j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x03]);
        assert_eq!(dimensions(&j), Some((640, 480)));
        // WebP VP8X: canvas 1000 x 500 stored minus one, 24-bit little endian.
        let mut w = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        w.extend_from_slice(&[0xE7, 0x03, 0x00, 0xF3, 0x01, 0x00]);
        assert_eq!(dimensions(&w), Some((1000, 500)));
        // WebP VP8L: 14-bit fields, minus one.
        let (wm1, hm1) = (99u32, 49u32);
        let bits = wm1 | (hm1 << 14);
        let mut l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(dimensions(&l), Some((100, 50)));
        assert_eq!(dimensions(b"not an image"), None);
        assert_eq!(dimensions(&[0xFF, 0xD8, 0xFF]), None);
    }

    #[test]
    fn token_estimates() {
        assert_eq!(image_tokens(Api::Messages, &png(1000, 1000)), 1334); // ceil(1e6 / 750)
        // 3136 x 1568 scales to 1568 x 784.
        assert_eq!(
            image_tokens(Api::Messages, &png(3136, 1568)),
            (1568u64 * 784).div_ceil(750)
        );
        assert_eq!(
            image_tokens(Api::ChatCompletions, &png(1000, 1000)),
            DEFAULT_IMAGE_TOKENS
        );
        assert_eq!(
            image_tokens(Api::Responses, &png(10, 10)),
            DEFAULT_IMAGE_TOKENS
        );
        assert_eq!(image_tokens(Api::Messages, b"junk"), DEFAULT_IMAGE_TOKENS);
    }
}
