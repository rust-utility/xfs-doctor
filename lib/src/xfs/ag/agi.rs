use crate::xfs::types::XfsBe32;

/// 'XAGI'
pub const XFS_AGI_MAGIC: u32 = 0x58414749;

pub const XFS_AGI_UNLINKED_BUCKETS: usize = 64;

/// On-disk size of the AGI header, in bytes.
pub const XFS_AGI_SIZE: usize = 296;

/// Reference: https://github.dev/torvalds/linux/blob/v2.6.24/fs/xfs/xfs_ag.h
#[derive(bincode::Decode, Debug)]
#[repr(C)]
pub struct XfsAgi {
    /*
     * Common allocation group header information
     */
    /// magic number == XFS_AGI_MAGIC
    pub agi_magicnum: XfsBe32,
    /// header version == XFS_AGI_VERSION
    pub agi_versionnum: XfsBe32,
    /// sequence # starting from 0
    pub agi_seqno: XfsBe32,
    /// size in blocks of a.g.
    pub agi_length: XfsBe32,
    /*
     * Inode information
     * Inodes are mapped by interpreting the inode number, so no
     * mapping data is needed here.
     */
    /// count of allocated inodes
    pub agi_count: XfsBe32,
    /// root of inode btree
    pub agi_root: XfsBe32,
    /// levels in inode btree
    pub agi_level: XfsBe32,
    /// number of free inodes
    pub agi_freecount: XfsBe32,
    /// new inode just allocated
    pub agi_newino: XfsBe32,
    /// last directory inode chunk
    pub agi_dirino: XfsBe32,
    /*
     * Hash table of inodes which have been unlinked but are
     * still being referenced.
     */
    pub agi_unlinked: [XfsBe32; XFS_AGI_UNLINKED_BUCKETS],
}

impl crate::xfs::XfsHeader for XfsAgi {
    const MAGIC: u32 = XFS_AGI_MAGIC;
    const KIND: &'static str = "AGI";

    fn magic(&self) -> u32 {
        self.agi_magicnum.0
    }
}

impl XfsAgi {
    /// Inodes allocated in this allocation group.
    pub fn count(&self) -> u32 {
        self.agi_count.0
    }

    /// Free inodes in this allocation group.
    pub fn freecount(&self) -> u32 {
        self.agi_freecount.0
    }

    /// Unlinked-but-still-referenced inode buckets, in bucket order.
    pub fn unlinked(&self) -> impl Iterator<Item = u32> + '_ {
        self.agi_unlinked.iter().map(|bucket| bucket.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xfs::{XfsHeader, XfsStruct};

    /// 10 big-endian words + 64 unlinked buckets.
    fn fixture() -> Vec<u8> {
        let mut buf = vec![0u8; XfsAgi::decoded_size()];
        buf[0..4].copy_from_slice(&0x58414749u32.to_be_bytes()); // 'XAGI'
        buf[4..8].copy_from_slice(&1u32.to_be_bytes()); // versionnum
        buf[8..12].copy_from_slice(&2u32.to_be_bytes()); // seqno
        buf[12..16].copy_from_slice(&2048u32.to_be_bytes()); // length
        buf[16..20].copy_from_slice(&512u32.to_be_bytes()); // count
        buf[28..32].copy_from_slice(&480u32.to_be_bytes()); // freecount
        buf[40..44].copy_from_slice(&7u32.to_be_bytes()); // unlinked[0]
        buf
    }

    #[test]
    fn decodes_known_agi() {
        let (_, agi) = XfsAgi::decode(&mut &fixture()[..]).unwrap();
        assert!(agi.is_valid());
        assert_eq!(agi.agi_seqno.0, 2);
        assert_eq!(agi.count(), 512);
        assert_eq!(agi.freecount(), 480);
        assert_eq!(agi.unlinked().next(), Some(7));
    }

    #[test]
    fn rejects_superblock_magic() {
        let mut buf = fixture();
        buf[0..4].copy_from_slice(&0x58465342u32.to_be_bytes()); // 'XFSB'
        let (_, agi) = XfsAgi::decode(&mut &buf[..]).unwrap();
        assert!(agi.validate().unwrap_err().to_string().contains("bad AGI magic"));
    }

    #[test]
    fn sizes_are_consistent() {
        assert_eq!(XFS_AGI_SIZE, 296);
        assert_eq!(XfsAgi::decoded_size(), XFS_AGI_SIZE);
    }
}
