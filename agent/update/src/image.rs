//! The uploaded image as a stream of disk bytes: decompressed if it is bz2,
//! and hashed as it is read, so whoever reads it learns the SHA-256 of the
//! file that was uploaded without a pass of its own.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

pub struct Image {
    decoded: Decoded,
}

enum Decoded {
    /// MultiBzDecoder, because oe-core compresses with pbzip2, which writes
    /// one stream per block.
    Bz2(bzip2::read::MultiBzDecoder<BufReader<Hashed>>),
    Plain(BufReader<Hashed>),
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

    /// bz2 or not is decided by the first bytes: pbzip2 output starts `BZh`.
    pub fn new(source: impl Read + Send + 'static) -> io::Result<Self> {
        let mut raw = BufReader::with_capacity(
            1 << 20,
            Hashed {
                inner: Box::new(source),
                hasher: Sha256::new(),
                len: 0,
            },
        );
        let decoded = if raw.fill_buf()?.starts_with(b"BZh") {
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
    fn a_bz2_image_is_decompressed_and_the_file_hashed() {
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
    fn a_plain_image_is_read_as_it_is() {
        let data = vec![9u8; 5000];
        let mut image = Image::new(io::Cursor::new(data.clone())).unwrap();
        let mut all = Vec::new();
        image.read_to_end(&mut all).unwrap();
        assert_eq!(all, data);
        assert_eq!(image.finish().unwrap().1, crate::sha256(&data));
    }
}
