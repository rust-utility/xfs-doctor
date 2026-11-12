//! On-disk scalar types.
//!
//! XFS stores every integer big-endian, while the host may be little-endian, so
//! each scalar is wrapped. The wrappers implement [`bincode::Decode`] (so structs
//! can derive the decoding) and print as bare numbers, which keeps the CLI output
//! readable.

use std::fmt;

use bincode::de::read::Reader;

macro_rules! big_endian_scalar {
    ($name:ident, $ty:ty, $size:expr) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(pub $ty);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // bare value: `sb_blocksize: 4096`, not `XfsBe32(4096)`
                write!(f, "{}", self.0)
            }
        }

        impl From<$name> for $ty {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl<Context> bincode::Decode<Context> for $name {
            fn decode<D: bincode::de::Decoder<Context = Context>>(
                decoder: &mut D,
            ) -> Result<Self, bincode::error::DecodeError> {
                decoder.claim_bytes_read($size)?;
                let mut bytes = [0u8; $size];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(<$ty>::from_be_bytes(bytes)))
            }
        }
        bincode::impl_borrow_decode!($name);
    };
}

big_endian_scalar!(XfsBe16, u16, 2);
big_endian_scalar!(XfsBe32, u32, 4);
big_endian_scalar!(XfsBe64, u64, 8);

/// A 16-byte UUID, stored as-is by XFS (no byte swapping).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct XfsUuid(pub uuid::Uuid);

impl fmt::Display for XfsUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for XfsUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<XfsUuid> for uuid::Uuid {
    fn from(value: XfsUuid) -> Self {
        value.0
    }
}

impl<Context> bincode::Decode<Context> for XfsUuid {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        <[u8; 16] as bincode::Decode<Context>>::decode(decoder)
            .map(|v| Self(uuid::Uuid::from_bytes(v)))
    }
}

bincode::impl_borrow_decode!(XfsUuid);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_are_big_endian() {
        let (be32, _) = bincode::decode_from_slice::<XfsBe32, _>(
            &[0x00, 0x00, 0x10, 0x00],
            bincode::config::standard(),
        )
        .unwrap();
        assert_eq!(be32.0, 4096);
        assert_eq!(be32.to_string(), "4096");

        let (be64, _) = bincode::decode_from_slice::<XfsBe64, _>(
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00],
            bincode::config::standard(),
        )
        .unwrap();
        assert_eq!(be64.0, 256);
    }

    #[test]
    fn uuid_roundtrips_without_byte_swapping() {
        let bytes: [u8; 16] = [
            0x56, 0xae, 0xc9, 0xf1, 0x11, 0xec, 0x31, 0xda, 0x58, 0xc1, 0x02, 0xe8, 0x60, 0xe1,
            0xf0, 0x5e,
        ];
        let (uuid, _) =
            bincode::decode_from_slice::<XfsUuid, _>(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(uuid.to_string(), "56aec9f1-11ec-31da-58c1-02e860e1f05e");
    }
}
