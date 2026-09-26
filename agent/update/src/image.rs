//! The uploaded image as a stream of disk bytes: decompressed if it is zstd
//! or bz2, and hashed as it is read, so whoever reads it learns the SHA-256
//! of the file that was uploaded without a pass of its own.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};
use sha2::{Digest, Sha256};

/// The first bytes of a zstd frame, 0xFD2FB528 little-endian.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

pub struct Image {
    decoded: Decoded,
}

enum Decoded {
    /// Boxed: ruzstd's FrameDecoder is several times the other variants.
    Zst(Box<MultiZstDecoder>),
    /// MultiBzDecoder, because oe-core compresses with pbzip2, which writes
    /// one stream per block.
    Bz2(bzip2::read::MultiBzDecoder<BufReader<Hashed>>),
    Plain(BufReader<Hashed>),
}

/// Every zstd frame in the file, one after another. oe-core's `zstd -T`
/// writes a single frame, but pzstd writes one per chunk and the format
/// allows any number, so the file is read to its end rather than to the end
/// of the first frame - the counterpart of MultiBzDecoder. Skippable frames
/// are skipped, and each frame's checksum is checked when it has one.
struct MultiZstDecoder {
    source: BufReader<Hashed>,
    frame: FrameDecoder,
    /// `frame` holds a frame that has not been read to its end.
    open: bool,
}

impl MultiZstDecoder {
    fn new(source: BufReader<Hashed>) -> Self {
        Self {
            source,
            frame: FrameDecoder::new(),
            open: false,
        }
    }

    fn into_inner(self) -> BufReader<Hashed> {
        self.source
    }
}

impl Read for MultiZstDecoder {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.open {
                if self.source.fill_buf()?.is_empty() {
                    return Ok(0);
                }
                match self.frame.reset(&mut self.source) {
                    Ok(()) => self.open = true,
                    Err(FrameDecoderError::ReadFrameHeaderError(
                        ReadFrameHeaderError::SkipFrame { length, .. },
                    )) => {
                        let length = u64::from(length);
                        let skipped =
                            io::copy(&mut (&mut self.source).take(length), &mut io::sink())?;
                        if skipped < length {
                            return Err(io::ErrorKind::UnexpectedEof.into());
                        }
                        continue;
                    }
                    Err(err) => return Err(io::Error::other(err)),
                }
            }
            // The same loop as ruzstd's StreamingDecoder, which reads one
            // frame only and drops its source on a skippable one.
            while self.frame.can_collect() < buf.len() && !self.frame.is_finished() {
                let wanted = buf.len() - self.frame.can_collect();
                let decoded = self
                    .frame
                    .decode_blocks(&mut self.source, BlockDecodingStrategy::UptoBytes(wanted));
                if let Err(err) = decoded {
                    // ruzstd reports a file cut off mid-frame as a failed
                    // block read; say so the way a short plain file does.
                    if self.source.fill_buf()?.is_empty() {
                        return Err(io::ErrorKind::UnexpectedEof.into());
                    }
                    return Err(io::Error::other(err));
                }
            }
            let read = self.frame.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            // Finished and drained: the calculated checksum covers all of it.
            if let (Some(stored), Some(calculated)) = (
                self.frame.get_checksum_from_data(),
                self.frame.get_calculated_checksum(),
            ) {
                if stored != calculated {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "a zstd frame does not match its checksum",
                    ));
                }
            }
            self.open = false;
        }
    }
}

/// The compressed bytes, hashed on their way to the decoder.
struct Hashed {
    inner: Box<dyn Read + Send>,
    hasher: Sha256,
    len: u64,
}

impl Read for Hashed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.update(&buf[..read]);
        self.len += read as u64;
        Ok(read)
    }
}

impl Image {
    pub fn open(path: &Path) -> io::Result<Self> {
        Self::new(File::open(path)?)
    }

    /// The format is decided by the first bytes, not the file name: a zstd
    /// frame starts with its magic number, pbzip2 output with `BZh`.
    pub fn new(source: impl Read + Send + 'static) -> io::Result<Self> {
        let mut raw = BufReader::with_capacity(
            1 << 20,
            Hashed {
                inner: Box::new(source),
                hasher: Sha256::new(),
                len: 0,
            },
        );
        let head = raw.fill_buf()?;
        let decoded = if head.starts_with(&ZSTD_MAGIC) {
            Decoded::Zst(Box::new(MultiZstDecoder::new(raw)))
        } else if head.starts_with(b"BZh") {
            Decoded::Bz2(bzip2::read::MultiBzDecoder::new(raw))
        } else {
            Decoded::Plain(raw)
        };
        Ok(Self { decoded })
    }

    /// Read what is left of the file, without decompressing it, and return
    /// the size and SHA-256 of all of it.
    pub fn finish(self) -> io::Result<(u64, String)> {
        let mut raw = match self.decoded {
            Decoded::Zst(decoder) => decoder.into_inner(),
            Decoded::Bz2(decoder) => decoder.into_inner(),
            Decoded::Plain(raw) => raw,
        };
        io::copy(&mut raw, &mut io::sink())?;
        let hashed = raw.into_inner();
        Ok((hashed.len, crate::hex(&hashed.hasher.finalize())))
    }
}

impl Read for Image {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.decoded {
            Decoded::Zst(decoder) => decoder.read(buf),
            Decoded::Bz2(decoder) => decoder.read(buf),
            Decoded::Plain(raw) => raw.read(buf),
        }
    }
}

/// Discard `bytes` of `image`.
pub fn skip(image: &mut impl Read, bytes: u64) -> Result<(), String> {
    let copied = io::copy(&mut image.take(bytes), &mut io::sink()).map_err(short)?;
    if copied < bytes {
        return Err("the image ends before its bmap does".to_string());
    }
    Ok(())
}

pub fn short(err: io::Error) -> String {
    if err.kind() == io::ErrorKind::UnexpectedEof {
        "the image ends before its bmap does".to_string()
    } else {
        format!("reading the image: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zst_image_is_decompressed_and_the_file_hashed() {
        let data: Vec<u8> = (0..300_000u32).map(|n| (n % 7) as u8).collect();
        let file = crate::testing::compress(&data);
        let mut image = Image::new(io::Cursor::new(file.clone())).unwrap();
        let mut head = vec![0u8; 1000];
        image.read_exact(&mut head).unwrap();
        assert_eq!(head, data[..1000]);
        // Stopping early still hashes the whole file.
        let (size, sha256) = image.finish().unwrap();
        assert_eq!(size, file.len() as u64);
        assert_eq!(sha256, crate::sha256(&file));
    }

    #[test]
    fn every_zst_frame_is_read() {
        let data: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let mut image = Image::new(io::Cursor::new(crate::testing::compress(&data))).unwrap();
        let mut all = Vec::new();
        image.read_to_end(&mut all).unwrap();
        assert_eq!(all.len(), data.len());
        assert!(all == data);
    }

    #[test]
    fn a_damaged_zst_frame_is_an_error() {
        let data: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
        let mut file = ruzstd::encoding::compress_to_vec(
            &data[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        // The frame checksum, last.
        let last = file.len() - 1;
        file[last] ^= 0xff;
        let mut image = Image::new(io::Cursor::new(file)).unwrap();
        let err = image.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
    }

    #[test]
    fn a_bz2_image_is_decompressed_and_the_file_hashed() {
        let data: Vec<u8> = (0..300_000u32).map(|n| (n % 7) as u8).collect();
        let file = crate::testing::compress_bz2(&data);
        let mut image = Image::new(io::Cursor::new(file.clone())).unwrap();
        let mut head = vec![0u8; 1000];
        image.read_exact(&mut head).unwrap();
        assert_eq!(head, data[..1000]);
        // Stopping early still hashes the whole file.
        let (size, sha256) = image.finish().unwrap();
        assert_eq!(size, file.len() as u64);
        assert_eq!(sha256, crate::sha256(&file));
    }

    #[test]
    fn a_plain_image_is_read_as_it_is() {
        let data = vec![9u8; 5000];
        let mut image = Image::new(io::Cursor::new(data.clone())).unwrap();
        let mut all = Vec::new();
        image.read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        assert_eq!(image.finish().unwrap().1, crate::sha256(&data));
    }
}
