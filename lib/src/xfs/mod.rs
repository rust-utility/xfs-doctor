//! Decoding of XFS on-disk headers.

pub mod ag;
pub mod error;
pub mod inode;
pub mod sb;
pub mod types;

use std::{io::BufRead, mem};

use crate::xfs::error::XfsError;

/// A fixed-size, self-describing on-disk structure.
pub trait XfsStruct: Sized {
    /// Read exactly `size_of::<Self>()` bytes and decode them.
    fn decode<B>(buf_reader: &mut B) -> Result<(Vec<u8>, Self), XfsError>
    where
        B: BufRead;

    /// Bytes read for one decode.
    ///
    /// `repr(C)` rounds the Rust struct size up to its alignment, so this can be a
    /// few bytes larger than the on-disk size (see `XFS_SB_SIZE` / `XFS_AGI_SIZE`).
    /// Reading the padding bytes is harmless.
    fn decoded_size() -> usize {
        mem::size_of::<Self>()
    }
}

impl<XFS> XfsStruct for XFS
where
    XFS: bincode::Decode,
{
    fn decode<B>(buf_reader: &mut B) -> Result<(Vec<u8>, Self), XfsError>
    where
        B: BufRead,
    {
        let mut buf = vec![0u8; Self::decoded_size()];

        buf_reader
            .read_exact(&mut buf)
            .map_err(|err| XfsError::IO(err.to_string()))?;

        bincode::decode_from_slice(&buf[..], bincode::config::standard())
            .map(|(value, _remaining)| (buf, value))
            .map_err(|err| XfsError::Decoder(err.to_string()))
    }
}

/// An XFS header identified by a magic number.
pub trait XfsHeader: XfsStruct {
    /// Expected magic, as it appears in the on-disk big-endian field once decoded.
    const MAGIC: u32;
    /// Human name used in error messages, e.g. `superblock`.
    const KIND: &'static str;

    /// The magic number actually decoded from the buffer.
    fn magic(&self) -> u32;

    /// Whether the decoded magic matches [`Self::MAGIC`].
    fn is_valid(&self) -> bool {
        self.magic() == Self::MAGIC
    }

    /// Fail unless the magic matches. Callers that deliberately inspect a damaged
    /// header can skip this (`xfs-doctor read --force`).
    fn validate(&self) -> Result<(), XfsError> {
        if self.is_valid() {
            Ok(())
        } else {
            Err(XfsError::BadMagic {
                kind: Self::KIND,
                found: self.magic(),
                expected: Self::MAGIC,
            })
        }
    }
}
