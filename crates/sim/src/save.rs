//! Save / load. Format: `b"CSIM"`, format version (u32 LE), postcard body.

use std::fmt;

use crate::World;

pub const MAGIC: &[u8; 4] = b"CSIM";
/// Bump when the World layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Not a CitySim save.
    BadMagic,
    UnsupportedVersion(u32),
    Corrupt(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::BadMagic => write!(f, "not a CitySim save file"),
            LoadError::UnsupportedVersion(v) => write!(f, "unsupported save version {v}"),
            LoadError::Corrupt(e) => write!(f, "corrupt save: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

pub(crate) fn save(world: &World) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * 1024);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    let body = postcard::to_stdvec(world).expect("World is always serialisable");
    out.extend_from_slice(&body);
    out
}

pub(crate) fn load(bytes: &[u8]) -> Result<World, LoadError> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(LoadError::BadMagic);
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(LoadError::UnsupportedVersion(version));
    }
    postcard::from_bytes(&bytes[8..]).map_err(|e| LoadError::Corrupt(e.to_string()))
}
