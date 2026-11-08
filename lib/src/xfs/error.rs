#[derive(thiserror::Error, Debug)]
pub enum XfsError {
    #[error("decoder failed: {0}")]
    Decoder(String),
    #[error("io failed: {0}")]
    IO(String),
    #[error("bad {kind} magic: found {found:#010x}, expected {expected:#010x}")]
    BadMagic {
        kind: &'static str,
        found: u32,
        expected: u32,
    },
    #[error("{kind} field out of range: {value} (max {max})")]
    OutOfRange {
        kind: &'static str,
        value: u64,
        max: u64,
    },
}
