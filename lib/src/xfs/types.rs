use bincode::de::read::Reader;

#[derive(Debug)]
pub struct XfsBe16(pub u16);
impl bincode::Decode for XfsBe16 {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        decoder.claim_bytes_read(2)?;
        let mut bytes = [0u8; 2];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(u16::from_be_bytes(bytes)))
    }
}
bincode::impl_borrow_decode!(XfsBe16);

#[derive(Debug)]
pub struct XfsBe32(pub u32);
impl bincode::Decode for XfsBe32 {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        decoder.claim_bytes_read(4)?;
        let mut bytes = [0u8; 4];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(u32::from_be_bytes(bytes)))
    }
}
bincode::impl_borrow_decode!(XfsBe32);

#[derive(Debug)]
pub struct XfsBe64(pub u64);
impl bincode::Decode for XfsBe64 {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        decoder.claim_bytes_read(8)?;
        let mut bytes = [0u8; 8];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(u64::from_be_bytes(bytes)))
    }
}
bincode::impl_borrow_decode!(XfsBe64);

#[derive(Debug)]
pub struct XfsUuid(uuid::Uuid);

impl bincode::Decode for XfsUuid {
    fn decode<D: bincode::de::Decoder>(
        decoder: &mut D,
    ) -> Result<Self, bincode::error::DecodeError> {
        <[u8; 16]>::decode(decoder).map(|v| Self(uuid::Uuid::from_bytes(v)))
    }
}
bincode::impl_borrow_decode!(XfsUuid);
