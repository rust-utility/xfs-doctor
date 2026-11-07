#[derive(thiserror::Error, Debug)]
pub enum XfsError {
    #[error("Decoder failed: {0}")]
    Decoder(String),
    #[error("IO failed: {0}")]
    IO(String),
}
