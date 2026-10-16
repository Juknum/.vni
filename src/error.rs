use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("not a .vni file (bad magic bytes)")]
    BadMagic,
    #[error("unsupported .vni format version {0}")]
    UnsupportedVersion(u16),
    #[error("corrupt .vni file: {0}")]
    Corrupt(String),
    #[error("revision not found: {0}")]
    RevisionNotFound(String),
    #[error("ambiguous revision: {0}")]
    AmbiguousRevision(String),
    #[error("invalid author {0:?}, expected \"Name <email>\"")]
    InvalidAuthor(String),
    #[error("nothing to commit: image is identical to {0}")]
    NothingToCommit(String),
    #[error("images have different dimensions ({0}x{1} vs {2}x{3})")]
    DimensionMismatch(u32, u32, u32, u32),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn corrupt<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Corrupt(msg.into()))
}
