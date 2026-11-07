use crate::xfs::types::{XfsBe16, XfsBe32, XfsBe64, XfsUuid};

/// 'XFSB'
pub const XFS_SB_MAGIC: u32 = 0x58465342;

/// Superblock - on disk version.  Must match the in core version below.
///
/// Reference: https://github.dev/torvalds/linux/blob/v2.6.24/fs/xfs/xfs_sb.h
#[derive(bincode::Decode, Debug)]
#[repr(C)]
pub struct XfsSuperBlock {
    /// magic number == XFS_SB_MAGIC
    pub sb_magicnum: XfsBe32,
    /// logical block size, bytes
    pub sb_blocksize: XfsBe32,
    /// number of data blocks
    pub sb_dblocks: XfsBe64,
    /// number of realtime blocks
    pub sb_rblocks: XfsBe64,
    /// number of realtime extents
    pub sb_rextents: XfsBe64,
    /// file system unique id
    pub sb_uuid: XfsUuid,
    /// starting block of log if internal
    pub sb_logstart: XfsBe64,
    /// root inode number
    pub sb_rootino: XfsBe64,
    /// bitmap inode for realtime extents
    pub sb_rbmino: XfsBe64,
    /// summary inode for rt bitmap
    pub sb_rsumino: XfsBe64,
    /// realtime extent size, blocks
    pub sb_rextsize: XfsBe32,
    /// size of an allocation group
    pub sb_agblocks: XfsBe32,
    /// number of allocation groups
    pub sb_agcount: XfsBe32,
    /// number of rt bitmap blocks
    pub sb_rbmblocks: XfsBe32,
    /// number of log blocks
    pub sb_logblocks: XfsBe32,
    /// header version == XFS_SB_VERSION
    pub sb_versionnum: XfsBe16,
    /// volume sector size, bytes
    pub sb_sectsize: XfsBe16,
    /// inode size, bytes
    pub sb_inodesize: XfsBe16,
    /// inodes per block
    pub sb_inopblock: XfsBe16,
    /// file system name
    pub sb_fname: [u8; 12],
    /// log2 of sb_blocksize
    pub sb_blocklog: u8,
    /// log2 of sb_sectsize
    pub sb_sectlog: u8,
    /// log2 of sb_inodesize
    pub sb_inodelog: u8,
    /// log2 of sb_inopblock
    pub sb_inopblog: u8,
    /// log2 of sb_agblocks (rounded up)
    pub sb_agblklog: u8,
    /// log2 of sb_rextents
    pub sb_rextslog: u8,
    /// mkfs is in progress, don't mount
    pub sb_inprogress: u8,
    /// max % of fs for inode space
    pub sb_imax_pct: u8,
    /* statistics */
    /*
     * These fields must remain contiguous.
     */
    /// allocated inodes
    pub sb_icount: XfsBe64,
    /// free inodes
    pub sb_ifree: XfsBe64,
    /// free data blocks
    pub sb_fdblocks: XfsBe64,
    /// free realtime extents
    pub sb_frextents: XfsBe64,
    /*
     * End contiguous fields.
     */
    /// user quota inode
    pub sb_uquotino: XfsBe64,
    /// group quota inode
    pub sb_gquotino: XfsBe64,
    /// quota flags
    pub sb_qflags: XfsBe16,
    /// misc. flags
    pub sb_flags: u8,
    /// shared version number
    pub sb_shared_vn: u8,
    /// inode chunk alignment, fsblocks
    pub sb_inoalignmt: XfsBe32,
    /// stripe or raid unit
    pub sb_unit: XfsBe32,
    /// stripe or raid width
    pub sb_width: XfsBe32,
    /// log2 of dir block size (fsbs)
    pub sb_dirblklog: u8,
    /// log2 of the log sector size
    pub sb_logsectlog: u8,
    /// sector size for the log, bytes
    pub sb_logsectsize: XfsBe16,
    /// stripe unit size for the log
    pub sb_logsunit: XfsBe32,
    /// additional feature bits
    pub sb_features2: XfsBe32,
}
