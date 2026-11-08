use crate::xfs::types::{XfsBe16, XfsBe32, XfsBe64, XfsUuid};

/// 'XFSB'
pub const XFS_SB_MAGIC: u32 = 0x58465342;

/// On-disk size of the v4 superblock prefix decoded here, in bytes.
pub const XFS_SB_SIZE: usize = 204;

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

impl crate::xfs::XfsHeader for XfsSuperBlock {
    const MAGIC: u32 = XFS_SB_MAGIC;
    const KIND: &'static str = "superblock";

    fn magic(&self) -> u32 {
        self.sb_magicnum.0
    }
}

impl XfsSuperBlock {
    /// Filesystem block size in bytes.
    pub fn blocksize(&self) -> u32 {
        self.sb_blocksize.0
    }

    /// Blocks per allocation group.
    pub fn agblocks(&self) -> u32 {
        self.sb_agblocks.0
    }

    /// Number of allocation groups.
    pub fn agcount(&self) -> u32 {
        self.sb_agcount.0
    }

    /// Byte offset of the AGI header of allocation group `ag`.
    ///
    /// Each AG starts with its own headers: block 0 = superblock copy, 1 = AGF,
    /// 2 = AGI, 3 = AGFL. So the AGI of AG `ag` lives at
    /// `(ag * agblocks + 2) * blocksize`.
    pub fn agi_offset(&self, ag: u32) -> Result<u64, crate::xfs::error::XfsError> {
        use crate::xfs::error::XfsError;

        if self.blocksize() == 0 || self.agblocks() == 0 {
            return Err(XfsError::OutOfRange {
                kind: "superblock geometry",
                value: 0,
                max: u64::MAX,
            });
        }
        if ag >= self.agcount() {
            return Err(XfsError::OutOfRange {
                kind: "allocation group",
                value: ag as u64,
                max: self.agcount().saturating_sub(1) as u64,
            });
        }
        Ok((ag as u64 * self.agblocks() as u64 + 2) * self.blocksize() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xfs::{XfsHeader, XfsStruct};

    /// The real on-disk header, padded to `size_of::<XfsSuperBlock>()`.
    fn fixture() -> Vec<u8> {
        let mut buf = vec![0u8; XfsSuperBlock::decoded_size()];
        buf[0..4].copy_from_slice(&0x58465342u32.to_be_bytes()); // 'XFSB'
        buf[4..8].copy_from_slice(&4096u32.to_be_bytes()); // blocksize
        buf[8..16].copy_from_slice(&8192u64.to_be_bytes()); // dblocks
        buf[32..48].copy_from_slice(&[
            0x56, 0xae, 0xc9, 0xf1, 0x11, 0xec, 0x31, 0xda, 0x58, 0xc1, 0x02, 0xe8, 0x60, 0xe1,
            0xf0, 0x5e,
        ]);
        buf[84..88].copy_from_slice(&2048u32.to_be_bytes()); // agblocks
        buf[88..92].copy_from_slice(&4u32.to_be_bytes()); // agcount
        buf[102..104].copy_from_slice(&512u16.to_be_bytes()); // sectsize
        buf[104..106].copy_from_slice(&256u16.to_be_bytes()); // inodesize
        buf
    }

    #[test]
    fn decodes_known_superblock() {
        let (_, sb) = XfsSuperBlock::decode(&mut &fixture()[..]).unwrap();
        assert!(sb.is_valid());
        assert_eq!(sb.blocksize(), 4096);
        assert_eq!(sb.agblocks(), 2048);
        assert_eq!(sb.agcount(), 4);
        assert_eq!(
            sb.sb_uuid.to_string(),
            "56aec9f1-11ec-31da-58c1-02e860e1f05e"
        );
    }

    #[test]
    fn rejects_foreign_header() {
        let mut buf = fixture();
        buf[0..4].copy_from_slice(&0xa92b4efcu32.to_be_bytes()); // md superblock magic
        let (_, sb) = XfsSuperBlock::decode(&mut &buf[..]).unwrap();
        let err = sb.validate().unwrap_err();
        assert!(err.to_string().contains("bad superblock magic"), "{err}");
    }

    #[test]
    fn agi_offset_follows_ag_geometry() {
        let (_, sb) = XfsSuperBlock::decode(&mut &fixture()[..]).unwrap();
        assert_eq!(sb.agi_offset(0).unwrap(), 2 * 4096);
        assert_eq!(sb.agi_offset(3).unwrap(), (3 * 2048 + 2) * 4096);
        assert!(sb.agi_offset(4).is_err(), "ag must be < agcount");
    }

    #[test]
    fn sizes_are_consistent() {
        assert_eq!(XFS_SB_SIZE, 204);
        // repr(C) pads to the 8-byte alignment of the u64 wrappers
        assert_eq!(XfsSuperBlock::decoded_size(), 208);
        assert!(XfsSuperBlock::decoded_size() >= XFS_SB_SIZE);
    }
}
