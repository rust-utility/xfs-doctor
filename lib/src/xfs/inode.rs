//! XFS inode decoding (v4 filesystems: 256-byte inodes, v1/v2 core).
//!
//! Field offsets were calibrated against the surviving 2011 `MyBookWorld` image:
//! `di_size` at 56, `di_nextents` at 76, data fork at 100.

use crate::xfs::error::XfsError;

pub const XFS_DINODE_MAGIC: u16 = 0x494e; // "IN"
/// Bytes of the v1/v2 inode core; the data fork starts here.
pub const INODE_CORE_SIZE: usize = 100;

/// Data fork format (`di_format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkFormat {
    /// Device inode, no data.
    Dev,
    /// Data stored inline in the inode.
    Local,
    /// Extent list in the inode.
    Extents,
    /// Extent btree, rooted in the inode.
    Btree,
}

impl ForkFormat {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Dev),
            1 => Some(Self::Local),
            2 => Some(Self::Extents),
            3 => Some(Self::Btree),
            _ => None,
        }
    }
}

/// One extent of a file's data fork.
///
/// 128-bit big-endian records: flag(1) | startoff(54) | startblock(52) | count(21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub unwritten: bool,
    /// Logical block offset within the file.
    pub startoff: u64,
    /// Filesystem block number of the first block.
    pub startblock: u64,
    /// Number of filesystem blocks.
    pub blockcount: u64,
}

impl Extent {
    pub fn decode(buf: &[u8; 16]) -> Self {
        let l0 = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        let l1 = u64::from_be_bytes(buf[8..16].try_into().unwrap());
        Self {
            unwritten: l0 >> 63 == 1,
            startoff: (l0 >> 9) & ((1 << 54) - 1),
            startblock: ((l0 & 0x1ff) << 43) | (l1 >> 21),
            blockcount: l1 & ((1 << 21) - 1),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Inode {
    /// Inode number (`agno << (agblklog + inopblog) | agino`).
    pub ino: u64,
    pub mode: u16,
    pub version: u8,
    pub format: ForkFormat,
    pub size: u64,
    pub nlink: u32,
    pub nextents: u32,
    /// Attribute fork offset in 8-byte units; 0 means no attribute fork.
    pub forkoff: u8,
    /// Raw data fork bytes (inline data or extent records).
    pub data_fork: Vec<u8>,
}

impl Inode {
    pub fn file_type(&self) -> u16 {
        self.mode & 0o170000
    }

    pub fn is_dir(&self) -> bool {
        self.file_type() == 0o040000
    }

    pub fn is_regular(&self) -> bool {
        self.file_type() == 0o100000
    }

    pub fn is_symlink(&self) -> bool {
        self.file_type() == 0o120000
    }

    /// Bytes of the inode available to the data fork (both forks share the 256-byte inode).
    fn data_fork_len(&self, inode_size: usize) -> usize {
        let attr = if self.forkoff > 0 {
            self.forkoff as usize * 8
        } else {
            0
        };
        inode_size.saturating_sub(INODE_CORE_SIZE + attr)
    }

    /// Decode an inode from its raw bytes. Returns `None` for an unused slot.
    pub fn decode(buf: &[u8], ino: u64) -> Result<Option<Self>, XfsError> {
        if buf.len() < INODE_CORE_SIZE + 16 {
            return Err(XfsError::Decoder(format!("inode {ino}: short buffer")));
        }
        let magic = u16::from_be_bytes([buf[0], buf[1]]);
        if magic != XFS_DINODE_MAGIC {
            return Ok(None);
        }
        let mode = u16::from_be_bytes([buf[2], buf[3]]);
        let format = ForkFormat::from_byte(buf[5])
            .ok_or_else(|| XfsError::Decoder(format!("inode {ino}: bad fork format {}", buf[5])))?;
        let size = u64::from_be_bytes(buf[56..64].try_into().unwrap());
        let nlink = u32::from_be_bytes(buf[16..20].try_into().unwrap());
        let nextents = u32::from_be_bytes(buf[76..80].try_into().unwrap());
        let forkoff = buf[82];
        let mut inode = Self {
            ino,
            mode,
            version: buf[4],
            format,
            size,
            nlink,
            nextents,
            forkoff,
            data_fork: Vec::new(),
        };
        let len = inode.data_fork_len(buf.len());
        inode.data_fork = buf[INODE_CORE_SIZE..INODE_CORE_SIZE + len].to_vec();
        Ok(Some(inode))
    }

    /// Extent list of an `Extents`-format inode.
    pub fn extents(&self) -> Result<Vec<Extent>, XfsError> {
        if self.format != ForkFormat::Extents {
            return Err(XfsError::Decoder(format!(
                "inode {}: not an extent-list inode",
                self.ino
            )));
        }
        let avail = self.data_fork.len() / 16;
        let count = (self.nextents as usize).min(avail);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let rec: [u8; 16] = self.data_fork[i * 16..i * 16 + 16].try_into().unwrap();
            out.push(Extent::decode(&rec));
        }
        Ok(out)
    }

    /// Whether this looks like a live inode rather than a stale slot from a
    /// deleted file: a known file type, at least one link, and a sane size.
    pub fn is_plausible(&self) -> bool {
        let known_type = matches!(self.file_type(), 0o040000 | 0o100000 | 0o120000);
        known_type && self.nlink > 0 && self.size < (1u64 << 50)
    }

    /// Whether every extent of an extent-format inode lies inside the filesystem
    /// and is consistent with the file size. Stale inodes routinely fail this.
    pub fn extents_valid(&self, dblocks: u64) -> bool {
        if self.format != ForkFormat::Extents {
            return false;
        }
        let Ok(extents) = self.extents() else {
            return false;
        };
        if extents.is_empty() {
            return self.size == 0;
        }
        let bs = 4096u64; // only used for the size check below; blocks are blockcount units
        let mut expected = 0u64;
        for extent in &extents {
            if extent.blockcount == 0
                || extent.startblock >= dblocks
                || extent.startblock + extent.blockcount > dblocks
            {
                return false;
            }
            if extent.startoff < expected {
                return false; // overlapping or unordered
            }
            expected = extent.startoff + extent.blockcount;
            if bs == 0 {
                return false;
            }
        }
        // the last extent must be able to cover the recorded size
        expected * bs >= self.size
    }

    /// Inline data of a `Local`-format inode, truncated to `size`.
    pub fn local_data(&self) -> &[u8] {
        let len = (self.size as usize).min(self.data_fork.len());
        &self.data_fork[..len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(mode: u16, format: u8, size: u64, nextents: u32) -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[0..2].copy_from_slice(&XFS_DINODE_MAGIC.to_be_bytes());
        b[2..4].copy_from_slice(&mode.to_be_bytes());
        b[4] = 1; // v1 inode
        b[5] = format;
        b[56..64].copy_from_slice(&size.to_be_bytes());
        b[76..80].copy_from_slice(&nextents.to_be_bytes());
        b
    }

    fn put_extent(b: &mut [u8], index: usize, startoff: u64, startblock: u64, count: u64) {
        let l0 = (startoff << 9) | (startblock >> 43);
        let l1 = ((startblock & ((1 << 43) - 1)) << 21) | (count & ((1 << 21) - 1));
        let o = INODE_CORE_SIZE + index * 16;
        b[o..o + 8].copy_from_slice(&l0.to_be_bytes());
        b[o + 8..o + 16].copy_from_slice(&l1.to_be_bytes());
    }

    #[test]
    fn decodes_regular_file_with_one_extent() {
        let mut b = fixture(0o100644, 2, 7425, 1);
        put_extent(&mut b, 0, 0, 386_301_963, 2);
        let inode = Inode::decode(&b, 42).unwrap().unwrap();
        assert!(inode.is_regular());
        assert_eq!(inode.size, 7425);
        let exts = inode.extents().unwrap();
        assert_eq!(exts.len(), 1);
        assert_eq!(exts[0].startblock, 386_301_963);
        assert_eq!(exts[0].blockcount, 2);
        assert!(!exts[0].unwritten);
    }

    #[test]
    fn decodes_multiple_extents_in_order() {
        let mut b = fixture(0o100644, 2, 4096 * 5, 3);
        put_extent(&mut b, 0, 0, 100, 1);
        put_extent(&mut b, 1, 1, 200, 2);
        put_extent(&mut b, 2, 3, 300, 2);
        let inode = Inode::decode(&b, 7).unwrap().unwrap();
        let exts = inode.extents().unwrap();
        assert_eq!(exts.len(), 3);
        assert_eq!(exts[2].startoff, 3);
        assert_eq!(exts[2].startblock, 300);
    }

    #[test]
    fn local_data_is_truncated_to_size() {
        let mut b = fixture(0o100644, 1, 5, 0);
        b[INODE_CORE_SIZE..INODE_CORE_SIZE + 5].copy_from_slice(b"hello");
        b[INODE_CORE_SIZE + 5] = b'X';
        let inode = Inode::decode(&b, 9).unwrap().unwrap();
        assert_eq!(inode.local_data(), b"hello");
    }

    #[test]
    fn unused_slot_is_not_an_inode() {
        assert!(Inode::decode(&[0u8; 256], 1).unwrap().is_none());
    }

    #[test]
    fn stale_inodes_are_implausible() {
        let mut b = fixture(0o100644, 2, 585, 1);
        put_extent(&mut b, 0, 0, 506_802_897, 1); // beyond dblocks
        let inode = Inode::decode(&b, 1).unwrap().unwrap();
        assert_eq!(inode.nlink, 0);
        assert!(!inode.is_plausible());
        assert!(!inode.extents_valid(486_761_216));
    }

    #[test]
    fn live_extents_inside_the_filesystem_are_valid() {
        let mut b = fixture(0o100644, 2, 4096, 1);
        b[16..20].copy_from_slice(&1u32.to_be_bytes()); // nlink = 1
        put_extent(&mut b, 0, 0, 1_000, 1);
        let inode = Inode::decode(&b, 1).unwrap().unwrap();
        assert!(inode.is_plausible());
        assert!(inode.extents_valid(486_761_216));
    }

    #[test]
    fn attribute_fork_shortens_the_data_fork() {
        let mut b = fixture(0o100644, 1, 10, 0);
        b[82] = 8; // forkoff: 64 bytes of attributes
        let inode = Inode::decode(&b, 3).unwrap().unwrap();
        assert_eq!(inode.data_fork.len(), 256 - INODE_CORE_SIZE - 64);
    }

    #[test]
    fn directories_and_symlinks_are_recognised() {
        let dir = Inode::decode(&fixture(0o040755, 1, 0, 0), 1)
            .unwrap()
            .unwrap();
        assert!(dir.is_dir());
        let lnk = Inode::decode(&fixture(0o120777, 1, 4, 0), 2)
            .unwrap()
            .unwrap();
        assert!(lnk.is_symlink());
    }
}
