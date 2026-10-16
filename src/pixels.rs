use std::io::{Read, Write};
use std::path::Path;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use image::{ImageEncoder, ImageFormat};
use sha2::{Digest, Sha256};

use crate::error::{corrupt, Error, Result};

pub type Id = [u8; 32];

/// Zero gaps shorter than this are folded into the surrounding literal.
const MIN_GAP: usize = 4;

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(buf: &[u8], at: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *buf.get(*at).ok_or_else(|| Error::Corrupt("delta varint truncated".into()))?;
        *at += 1;
        if shift == 63 && b > 1 {
            break;
        }
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    corrupt("delta varint overflows")
}

/// An image normalised to 8-bit RGBA. This is the canonical pixel model of a
/// `.vni` file: whatever you import is converted to it, and every revision is
/// reproduced from it bit for bit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Summary of how two same-sized images differ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DiffStats {
    pub changed_pixels: u64,
    pub total_pixels: u64,
    /// Inclusive bounding box `(x0, y0, x1, y1)` of changed pixels.
    pub bbox: Option<(u32, u32, u32, u32)>,
}

impl Pixels {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Result<Self> {
        if data.len() as u64 != width as u64 * height as u64 * 4 {
            return corrupt("pixel buffer length does not match dimensions");
        }
        Ok(Self { width, height, data })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let rgba = image::open(path)?.to_rgba8();
        Ok(Self { width: rgba.width(), height: rgba.height(), data: rgba.into_raw() })
    }

    /// Save in the format implied by the file extension (png, jpg, ...).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        image::save_buffer(path, &self.data, self.width, self.height, image::ColorType::Rgba8)?;
        Ok(())
    }

    /// Content hash of dimensions + pixels.
    pub fn tree_hash(&self) -> Id {
        let mut h = Sha256::new();
        h.update(self.width.to_le_bytes());
        h.update(self.height.to_le_bytes());
        h.update(&self.data);
        h.finalize().into()
    }

    pub fn encode_png(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut out,
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(&self.data, self.width, self.height, image::ExtendedColorType::Rgba8)?;
        Ok(out)
    }

    pub fn decode_png(bytes: &[u8]) -> Result<Self> {
        let rgba = image::load_from_memory_with_format(bytes, ImageFormat::Png)?.to_rgba8();
        Ok(Self { width: rgba.width(), height: rgba.height(), data: rgba.into_raw() })
    }

    /// Delta that turns `self` into `next` (same dimensions).
    ///
    /// The per-byte wrapping difference is stored sparsely as repeated
    /// `[zero_run: varint][literal_len: varint][literal bytes]` segments, then
    /// zlib-compressed. Cost therefore scales with the edit, not the image area.
    pub fn encode_delta(&self, next: &Pixels) -> Result<Vec<u8>> {
        self.require_same_size(next)?;
        let diff: Vec<u8> =
            self.data.iter().zip(&next.data).map(|(a, b)| b.wrapping_sub(*a)).collect();
        let mut sparse = Vec::new();
        let mut i = 0;
        while i < diff.len() {
            let start = i;
            while i < diff.len() && diff[i] == 0 {
                i += 1;
            }
            if i == diff.len() {
                break; // trailing zeros are implicit
            }
            let lit_start = i;
            let mut lit_end = i;
            // Extend the literal; absorb zero gaps shorter than MIN_GAP.
            while i < diff.len() {
                if diff[i] != 0 {
                    i += 1;
                    lit_end = i;
                } else {
                    let gap = diff[i..].iter().take(MIN_GAP).take_while(|&&b| b == 0).count();
                    if gap >= MIN_GAP || i + gap >= diff.len() {
                        break;
                    }
                    i += gap;
                }
            }
            put_varint(&mut sparse, (lit_start - start) as u64);
            put_varint(&mut sparse, (lit_end - lit_start) as u64);
            sparse.extend_from_slice(&diff[lit_start..lit_end]);
            i = lit_end;
        }
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::best());
        enc.write_all(&sparse)?;
        Ok(enc.finish()?)
    }

    /// Inverse of [`Pixels::encode_delta`].
    pub fn apply_delta(&self, delta: &[u8]) -> Result<Pixels> {
        let len = self.data.len();
        // Worst case (alternating zero/non-zero) is 1.5x the image; cap at 2x so a
        // hostile file cannot make us allocate without bound.
        let cap = 2 * len as u64 + 16;
        let mut sparse = Vec::new();
        ZlibDecoder::new(delta).take(cap + 1).read_to_end(&mut sparse)?;
        if sparse.len() as u64 > cap {
            return corrupt("delta is larger than any valid delta for this image");
        }
        let mut data = self.data.clone();
        let (mut pos, mut at) = (0usize, 0usize);
        while at < sparse.len() {
            let skip = get_varint(&sparse, &mut at)?;
            let lit = get_varint(&sparse, &mut at)?;
            let (skip, lit) = (usize::try_from(skip).ok(), usize::try_from(lit).ok());
            let (Some(skip), Some(lit)) = (skip, lit) else { return corrupt("delta segment too large") };
            let start = pos.checked_add(skip).filter(|&p| p <= len);
            let end = start.and_then(|s| s.checked_add(lit)).filter(|&e| e <= len);
            let (Some(start), Some(end)) = (start, end) else { return corrupt("delta segment out of bounds") };
            let src = sparse.get(at..at + lit).ok_or_else(|| Error::Corrupt("delta literal truncated".into()))?;
            for (d, s) in data[start..end].iter_mut().zip(src) {
                *d = d.wrapping_add(*s);
            }
            at += lit;
            pos = end;
        }
        Ok(Pixels { width: self.width, height: self.height, data })
    }

    pub fn diff_stats(&self, other: &Pixels) -> Result<DiffStats> {
        self.require_same_size(other)?;
        let w = self.width as usize;
        let (mut count, mut x0, mut y0, mut x1, mut y1) = (0u64, u32::MAX, u32::MAX, 0u32, 0u32);
        for (i, (a, b)) in self.data.chunks_exact(4).zip(other.data.chunks_exact(4)).enumerate() {
            if a != b {
                let (x, y) = ((i % w) as u32, (i / w) as u32);
                count += 1;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        Ok(DiffStats {
            changed_pixels: count,
            total_pixels: self.width as u64 * self.height as u64,
            bbox: (count > 0).then_some((x0, y0, x1, y1)),
        })
    }

    /// Visualisation of a diff: unchanged pixels dimmed to grey, changed ones red.
    pub fn diff_image(&self, other: &Pixels) -> Result<Pixels> {
        self.require_same_size(other)?;
        let mut data = Vec::with_capacity(self.data.len());
        for (a, b) in self.data.chunks_exact(4).zip(other.data.chunks_exact(4)) {
            if a != b {
                data.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                let g = ((a[0] as u16 + a[1] as u16 + a[2] as u16) / 3 / 2 + 96) as u8;
                data.extend_from_slice(&[g, g, g, 255]);
            }
        }
        Ok(Pixels { width: self.width, height: self.height, data })
    }

    fn require_same_size(&self, other: &Pixels) -> Result<()> {
        if (self.width, self.height) != (other.width, other.height) {
            return Err(Error::DimensionMismatch(self.width, self.height, other.width, other.height));
        }
        Ok(())
    }
}

