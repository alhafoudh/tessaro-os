//! The block map bmaptool writes next to an image (format 2.0).
//!
//! It lists the ranges of blocks that hold data - partition tables,
//! filesystem metadata, file contents - each with a SHA-256. Everything not
//! listed is free space in some filesystem and does not need writing, which
//! is what keeps an update to what actually changed on disk, not the whole
//! partition.

use sha2::{Digest, Sha256};

use crate::hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bmap {
    pub image_size: u64,
    pub block_size: u64,
    pub blocks: u64,
    pub ranges: Vec<Range>,
}

/// Blocks `first..=last`, and the SHA-256 of their bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub first: u64,
    pub last: u64,
    pub sha256: String,
}

impl Bmap {
    /// Byte offset where `range` starts.
    pub fn start(&self, range: &Range) -> u64 {
        range.first * self.block_size
    }

    /// Byte offset just past `range`. The last block of an image may be
    /// short, and bmaptool hashes only the bytes that exist.
    pub fn end(&self, range: &Range) -> u64 {
        ((range.last + 1) * self.block_size).min(self.image_size)
    }

    /// Mapped bytes inside `start..end`.
    pub fn mapped_within(&self, start: u64, end: u64) -> u64 {
        self.ranges
            .iter()
            .map(|range| {
                let from = self.start(range).max(start);
                let to = self.end(range).min(end);
                to.saturating_sub(from)
            })
            .sum()
    }
}

pub fn parse(text: &str) -> Result<Bmap, String> {
    let doc =
        roxmltree::Document::parse(text).map_err(|err| format!("the bmap is not XML: {err}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("bmap") {
        return Err("this is not a bmap file".to_string());
    }
    let version = root.attribute("version").unwrap_or("");
    if version.split('.').next() != Some("2") {
        return Err(format!(
            "bmap format {version:?} is not supported; bmaptool 3 writes 2.0"
        ));
    }

    let field = |name: &str| -> Result<&str, String> {
        root.children()
            .find(|node| node.has_tag_name(name))
            .and_then(|node| node.text())
            .map(str::trim)
            .ok_or_else(|| format!("the bmap has no {name}"))
    };
    let number = |name: &str| -> Result<u64, String> {
        field(name)?
            .parse::<u64>()
            .map_err(|_| format!("the bmap's {name} is not a number"))
    };

    let checksum_type = field("ChecksumType")?;
    if checksum_type != "sha256" {
        return Err(format!(
            "the bmap uses {checksum_type} checksums; only sha256 is supported"
        ));
    }

    // bmaptool hashes the whole file with this field set to zeros. A bmap
    // that was truncated or edited on the way fails here, before it can
    // describe the wrong blocks.
    let declared = field("BmapFileChecksum")?;
    if !is_sha256(declared) {
        return Err("the bmap's BmapFileChecksum is not a SHA-256".to_string());
    }
    let zeroed = text.replacen(declared, &"0".repeat(64), 1);
    if hex(&Sha256::digest(zeroed.as_bytes())) != declared.to_ascii_lowercase() {
        return Err("the bmap file is damaged: its own checksum does not match".to_string());
    }

    let image_size = number("ImageSize")?;
    let block_size = number("BlockSize")?;
    let blocks = number("BlocksCount")?;
    if block_size == 0 || !block_size.is_power_of_two() {
        return Err(format!("the bmap's block size {block_size} is not usable"));
    }
    if image_size > blocks * block_size {
        return Err("the bmap's ImageSize exceeds its blocks".to_string());
    }

    let map = root
        .children()
        .find(|node| node.has_tag_name("BlockMap"))
        .ok_or("the bmap has no BlockMap")?;
    let mut ranges: Vec<Range> = Vec::new();
    for node in map.children().filter(|node| node.has_tag_name("Range")) {
        let text = node.text().unwrap_or("").trim();
        let (first, last) = match text.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (text, text),
        };
        let parse_block = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| format!("the bmap range {text:?} is not a block range"))
        };
        let (first, last) = (parse_block(first)?, parse_block(last)?);
        let sha256 = node
            .attribute("chksum")
            .ok_or_else(|| format!("the bmap range {text} has no checksum"))?
            .to_ascii_lowercase();
        if !is_sha256(&sha256) {
            return Err(format!("the bmap range {text} has a malformed checksum"));
        }
        if first > last || last >= blocks {
            return Err(format!("the bmap range {text} is out of bounds"));
        }
        if ranges.last().is_some_and(|previous| previous.last >= first) {
            return Err(format!("the bmap range {text} is out of order"));
        }
        ranges.push(Range {
            first,
            last,
            sha256,
        });
    }

    Ok(Bmap {
        image_size,
        block_size,
        blocks,
        ranges,
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::render_bmap as render;

    #[test]
    fn a_bmap_round_trips() {
        let image: Vec<u8> = (0..40_000u32).map(|n| n as u8).collect();
        let text = render(&image, 4096, &[(0, 1), (5, 5), (8, 9)]);
        let bmap = parse(&text).unwrap();

        assert_eq!(bmap.image_size, 40_000);
        assert_eq!(bmap.block_size, 4096);
        assert_eq!(bmap.blocks, 10);
        assert_eq!(bmap.ranges.len(), 3);
        assert_eq!((bmap.ranges[1].first, bmap.ranges[1].last), (5, 5));
        // The last block is short: 40000 - 9*4096.
        assert_eq!(bmap.end(&bmap.ranges[2]), 40_000);
        assert_eq!(
            bmap.mapped_within(0, 40_000),
            2 * 4096 + 4096 + (40_000 - 8 * 4096)
        );
        assert_eq!(bmap.mapped_within(4096, 5 * 4096 + 10), 4096 + 10);
    }

    #[test]
    fn an_edited_bmap_is_refused() {
        let image = vec![7u8; 16_384];
        let text = render(&image, 4096, &[(0, 3)]);
        let edited = text.replace("> 0-3 <", "> 0-2 <");
        assert!(parse(&edited).unwrap_err().contains("damaged"));
    }

    #[test]
    fn the_real_format_parses() {
        // The shape bmaptool 3.9 writes, comments and all.
        let text = r#"<?xml version="1.0" ?>
<!-- comment -->
<bmap version="2.0">
    <!-- Image size in bytes: 8.0 KiB -->
    <ImageSize> 8192 </ImageSize>
    <BlockSize> 4096 </BlockSize>
    <BlocksCount> 2 </BlocksCount>
    <MappedBlocksCount> 1  </MappedBlocksCount>
    <ChecksumType> sha256 </ChecksumType>
    <BmapFileChecksum> 0000000000000000000000000000000000000000000000000000000000000000 </BmapFileChecksum>
    <BlockMap>
        <Range chksum="ad7facb2586fc6e966c004d7d1d16b024f5805ff7cb47c7a85dabd8b48892ca7"> 1 </Range>
    </BlockMap>
</bmap>
"#;
        let checksum = crate::sha256(text.as_bytes());
        let text = text.replacen(&"0".repeat(64), &checksum, 1);
        let bmap = parse(&text).unwrap();
        assert_eq!(bmap.ranges[0].first, 1);
        assert_eq!(bmap.ranges[0].last, 1);
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(parse("not xml").is_err());
        assert!(parse("<other/>").is_err());
        assert!(parse("<bmap version=\"1.4\"/>")
            .unwrap_err()
            .contains("1.4"));
    }
}
