//! Salvage helpers: filesystem geometry, block mapping for a single RAID0 member,
//! and directory-entry harvesting.
//!
//! The model: the filesystem lives on a 2-disk RAID0, we hold one member (every
//! other 512 KiB chunk). Because an AG is an exact multiple of the chunk size,
//! each allocation group sits entirely on one disk — so a present AG is *complete*
//! and files whose extents stay inside present AGs are recoverable in full.

use crate::raid::Raid0Half;
use crate::xfs::error::XfsError;
use crate::xfs::inode::Extent;
use crate::xfs::sb::XFS_SB_MAGIC;

/// Geometry read from a superblock (primary or a surviving secondary copy).
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub blocksize: u32,
    pub agblocks: u32,
    pub agcount: u32,
    pub dblocks: u64,
    pub inodesize: u16,
    pub inopblock: u16,
    pub agblklog: u8,
    pub inopblog: u8,
}

impl Geometry {
    pub fn from_sb(buf: &[u8]) -> Result<Self, XfsError> {
        if buf.len() < 208 {
            return Err(XfsError::Decoder("superblock: short buffer".into()));
        }
        let magic = u32::from_be_bytes(buf[0..4].try_into().unwrap());
        if magic != XFS_SB_MAGIC {
            return Err(XfsError::BadMagic {
                kind: "superblock",
                found: magic,
                expected: XFS_SB_MAGIC,
            });
        }
        let geometry = Self {
            blocksize: u32::from_be_bytes(buf[4..8].try_into().unwrap()),
            dblocks: u64::from_be_bytes(buf[8..16].try_into().unwrap()),
            agblocks: u32::from_be_bytes(buf[84..88].try_into().unwrap()),
            agcount: u32::from_be_bytes(buf[88..92].try_into().unwrap()),
            inodesize: u16::from_be_bytes(buf[104..106].try_into().unwrap()),
            inopblock: u16::from_be_bytes(buf[106..108].try_into().unwrap()),
            inopblog: buf[123],
            agblklog: buf[124],
        };
        if geometry.blocksize == 0
            || geometry.agblocks == 0
            || geometry.agcount == 0
            || geometry.inodesize == 0
        {
            return Err(XfsError::Decoder("superblock: zero geometry field".into()));
        }
        Ok(geometry)
    }

    pub fn ag_size(&self) -> u64 {
        self.agblocks as u64 * self.blocksize as u64
    }

    /// Inode number of `agino` inside allocation group `ag`.
    pub fn inode_number(&self, ag: u32, agino: u64) -> u64 {
        ((ag as u64) << (self.agblklog as u64 + self.inopblog as u64)) | agino
    }
}

/// Geometry plus the stripe mapping of the member we hold.
#[derive(Debug, Clone, Copy)]
pub struct Salvage {
    pub geometry: Geometry,
    pub stripe: Raid0Half,
}

impl Salvage {
    pub fn new(geometry: Geometry, stripe: Raid0Half) -> Self {
        Self { geometry, stripe }
    }

    /// Whether allocation group `ag` is entirely on the member we hold.
    pub fn ag_present(&self, ag: u32) -> bool {
        let start = ag as u64 * self.geometry.ag_size();
        self.stripe.disk_offset(start).is_some()
    }

    /// Byte offset (in the image) of logical filesystem offset `logical`.
    pub fn disk_offset(&self, logical: u64) -> Option<u64> {
        self.stripe.disk_offset(logical)
    }

    /// Byte offset of filesystem block `fsblock`, when its AG is on this member.
    pub fn block_offset(&self, fsblock: u64) -> Option<u64> {
        self.disk_offset(fsblock * self.geometry.blocksize as u64)
    }

    /// Every extent of `extents` translated to image offsets, or `None` when any
    /// block of any extent lives on the other member.
    pub fn readable_runs(&self, extents: &[Extent]) -> Option<Vec<(u64, u64, u64)>> {
        let bs = self.geometry.blocksize as u64;
        let mut runs = Vec::with_capacity(extents.len());
        for extent in extents {
            if extent.unwritten || extent.blockcount == 0 {
                continue;
            }
            for i in 0..extent.blockcount {
                let fsblock = extent.startblock + i;
                let offset = self.block_offset(fsblock)?;
                runs.push((extent.startoff * bs + i * bs, offset, bs));
            }
        }
        Some(runs)
    }

    /// Allocation groups present on this member, in order.
    pub fn present_ags(&self) -> Vec<u32> {
        (0..self.geometry.agcount)
            .filter(|ag| self.ag_present(*ag))
            .collect()
    }
}

/// A directory entry harvested from a directory data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub ino: u64,
    pub name: String,
}

const DIR_MAGICS: [[u8; 4]; 4] = [
    *b"XD2B", // dir2 block
    *b"XD2D", // dir2 data
    *b"XDB3", // dir3 block
    *b"XDD3", // dir3 data
];

/// Parse the entries of one directory data block (`dir2`/`dir3`, block or data format).
///
/// Entry layout (v4, no ftype byte): `inumber(8 BE) | namelen(1) | name | tag(2)`,
/// aligned to 8 bytes. v3 adds a file-type byte between name and tag. The block
/// tail (`count`, `stale`) sits in the last 8 bytes and bounds the data region.
pub fn dir_entries(block: &[u8], max_inode: u64) -> Vec<DirEntry> {
    let mut out = Vec::new();
    if block.len() < 64 || !DIR_MAGICS.iter().any(|m| block.starts_with(m)) {
        return out;
    }
    let is_v3 = block.starts_with(b"XDB3") || block.starts_with(b"XDD3");
    let header = if is_v3 { 64 } else { 16 };
    let count = u32::from_be_bytes(block[block.len() - 8..block.len() - 4].try_into().unwrap()) as usize;
    let data_end = block
        .len()
        .saturating_sub(8)
        .saturating_sub(count.saturating_mul(8))
        .max(header);

    let mut pos = header;
    while pos + 11 <= data_end {
        let ino = u64::from_be_bytes(block[pos..pos + 8].try_into().unwrap());
        let namelen = block[pos + 8] as usize;
        if ino == 0 || ino == u64::MAX || ino > max_inode || namelen == 0 || namelen > 255 {
            pos += 8;
            continue;
        }
        let name_start = pos + 9;
        let name_end = name_start + namelen;
        if name_end > data_end {
            break;
        }
        let name = &block[name_start..name_end];
        if name.iter().all(|c| (32..127).contains(c)) {
            if let Ok(text) = std::str::from_utf8(name) {
                out.push(DirEntry { ino, name: text.to_string() });
            }
        }
        let entry_len = 9 + namelen + if is_v3 { 1 } else { 0 } + 2;
        pos += (entry_len + 7) / 8 * 8;
    }
    out
}

/// Parse a shortform directory (small directories stored inside the inode).
///
/// `xfs_dir2_sf_hdr`: count(1), i8count(1), parent(4 or 8). Each entry is
/// `namelen(1) | offset(2) | name | [ftype(1) in v3] | inode(4 or 8)` — the inode
/// number comes *after* the name.
pub fn sf_dir_entries(data: &[u8], max_inode: u64) -> Vec<DirEntry> {
    let mut out = Vec::new();
    if data.len() < 6 {
        return out;
    }
    let count = data[0] as usize;
    let i8count = data[1] != 0;
    let ino_size = if i8count { 8 } else { 4 };
    let mut pos = 2 + ino_size; // header: count, i8count, parent
    for _ in 0..count {
        if pos + 3 > data.len() {
            break;
        }
        let namelen = data[pos] as usize;
        let name_start = pos + 3;
        let name_end = name_start + namelen;
        if namelen == 0 || namelen > 255 || name_end + ino_size > data.len() {
            break;
        }
        let ino = if i8count {
            u64::from_be_bytes(data[name_end..name_end + 8].try_into().unwrap())
        } else {
            u32::from_be_bytes(data[name_end..name_end + 4].try_into().unwrap()) as u64
        };
        let name = &data[name_start..name_end];
        if ino != 0 && ino <= max_inode && name.iter().all(|c| (32..127).contains(c)) {
            if let Ok(text) = std::str::from_utf8(name) {
                out.push(DirEntry { ino, name: text.to_string() });
            }
        }
        pos = name_end + ino_size;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sb(blocksize: u32, agblocks: u32, agcount: u32) -> Vec<u8> {
        let mut b = vec![0u8; 208];
        b[0..4].copy_from_slice(&XFS_SB_MAGIC.to_be_bytes());
        b[4..8].copy_from_slice(&blocksize.to_be_bytes());
        b[8..16].copy_from_slice(&(agblocks as u64 * agcount as u64).to_be_bytes());
        b[84..88].copy_from_slice(&agblocks.to_be_bytes());
        b[88..92].copy_from_slice(&agcount.to_be_bytes());
        b[104..106].copy_from_slice(&256u16.to_be_bytes());
        b[106..108].copy_from_slice(&16u16.to_be_bytes());
        b[123] = 4; // inopblog
        b[124] = 24; // agblklog
        b
    }

    #[test]
    fn geometry_matches_the_recovered_superblock() {
        let g = Geometry::from_sb(&sb(4096, 15_211_392, 32)).unwrap();
        assert_eq!(g.blocksize, 4096);
        assert_eq!(g.agcount, 32);
        assert_eq!(g.ag_size(), 15_211_392 * 4096);
        assert_eq!(g.inode_number(01, 0), 1 << (24 + 4));
    }

    #[test]
    fn rejects_foreign_superblock() {
        let mut b = sb(4096, 1, 1);
        b[0..4].copy_from_slice(&0xa92b4efcu32.to_be_bytes());
        assert!(Geometry::from_sb(&b).is_err());
    }

    #[test]
    fn whole_ags_land_on_exactly_one_member() {
        // 512 KiB chunks, AG size a multiple of the chunk → AG parity decides the member
        let g = Geometry::from_sb(&sb(4096, 15_211_392, 32)).unwrap();
        let s = Salvage::new(g, Raid0Half::new(8192, 524_288, 2, 1).unwrap());
        let present = s.present_ags();
        assert_eq!(present.len(), 16);
        assert!(present.iter().all(|ag| ag % 2 == 1), "{present:?}");
    }

    #[test]
    fn extent_runs_are_translated_to_image_offsets() {
        let g = Geometry::from_sb(&sb(4096, 15_211_392, 32)).unwrap();
        let s = Salvage::new(g, Raid0Half::new(8192, 524_288, 2, 1).unwrap());
        // a block inside AG1 is present, one inside AG2 is not
        let ag1_block = 15_211_392u64 + 10;
        let ag2_block = 2 * 15_211_392u64 + 10;
        assert!(s.block_offset(ag1_block).is_some());
        assert!(s.block_offset(ag2_block).is_none());
        let extents = [Extent { unwritten: false, startoff: 0, startblock: ag1_block, blockcount: 2 }];
        assert_eq!(s.readable_runs(&extents).unwrap().len(), 2);
        let missing = [Extent { unwritten: false, startoff: 0, startblock: ag2_block, blockcount: 1 }];
        assert!(s.readable_runs(&missing).is_none());
    }

    fn dir_block(names: &[(&str, u64)]) -> Vec<u8> {
        let mut b = vec![0u8; 4096];
        b[0..4].copy_from_slice(b"XD2B");
        let mut pos = 16;
        for (name, ino) in names {
            b[pos..pos + 8].copy_from_slice(&ino.to_be_bytes());
            b[pos + 8] = name.len() as u8;
            b[pos + 9..pos + 9 + name.len()].copy_from_slice(name.as_bytes());
            let entry = 9 + name.len() + 2;
            let aligned = (entry + 7) / 8 * 8;
            let tag_at = pos + aligned - 2;
            b[tag_at..tag_at + 2].copy_from_slice(&(pos as u16).to_be_bytes());
            pos += aligned;
        }
        // tail: no leaf entries
        // (count stays 0)
        b
    }

    #[test]
    fn harvests_names_from_a_dir_block() {
        let block = dir_block(&[(".", 128), ("..", 129), ("report.txt", 6195)]);
        let entries = dir_entries(&block, 1 << 33);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".", "..", "report.txt"]);
        assert_eq!(entries[2].ino, 6195);
    }

    #[test]
    fn harvests_shortform_entries() {
        // count=3, i8count=0, parent=0x1832, then three entries with 4-byte inodes
        let mut d = vec![3u8, 0];
        d.extend_from_slice(&0x1832u32.to_be_bytes());
        for (name, ino) in [(".", 6194u32), ("..", 6194), ("notes.txt", 6200)] {
            d.push(name.len() as u8);
            d.extend_from_slice(&[0, 0x30]);
            d.extend_from_slice(name.as_bytes());
            d.extend_from_slice(&ino.to_be_bytes());
        }
        let entries = sf_dir_entries(&d, 1 << 33);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".", "..", "notes.txt"]);
        assert_eq!(entries[2].ino, 6200);
    }

    #[test]
    fn shortform_with_64bit_inodes() {
        let mut d = vec![1u8, 1];
        d.extend_from_slice(&0x1832u64.to_be_bytes());
        d.push(3);
        d.extend_from_slice(&[0, 0x30]);
        d.extend_from_slice(b"abc");
        d.extend_from_slice(&(1u64 << 33).to_be_bytes());
        let entries = sf_dir_entries(&d, u64::MAX);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ino, 1 << 33);
    }

    #[test]
    fn ignores_blocks_that_are_not_directories() {
        let mut block = vec![0u8; 4096];
        block[0..4].copy_from_slice(b"IN\x81\xa4");
        assert!(dir_entries(&block, 1 << 40).is_empty());
    }
}
