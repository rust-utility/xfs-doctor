pub mod ag;
pub mod error;
pub mod sb;
pub mod types;

use std::{io::BufRead, mem};

use crate::xfs::error::XfsError;

pub trait XfsStruct: Sized {
    fn decode<B>(buf_reader: &mut B) -> Result<(Vec<u8>, Self), XfsError>
    where
        B: BufRead;
}

impl<XFS> XfsStruct for XFS
where
    XFS: bincode::Decode,
{
    fn decode<B>(buf_reader: &mut B) -> Result<(Vec<u8>, Self), XfsError>
    where
        B: BufRead,
    {
        let mut buf = vec![0u8; mem::size_of::<Self>()];

        buf_reader
            .read_exact(&mut buf)
            .map_err(|err| XfsError::IO(err.to_string()))?;

        bincode::decode_from_slice(buf.as_mut(), bincode::config::standard())
            .map(|(value, _remaining)| (buf, value))
            .map_err(|err| XfsError::Decoder(err.to_string()))
    }
}
