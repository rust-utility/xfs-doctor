use crate::xfs::types::XfsBe32;

/// 'XAGI'
pub const XFS_AGI_MAGIC: u32 = 0x58414749;

pub const XFS_AGI_UNLINKED_BUCKETS: usize = 64;

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
