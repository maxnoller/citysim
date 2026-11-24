//! Save / load. Format: `b"CSIM"`, format version (u32 LE), postcard body.

use std::fmt;

use crate::{MAP_CELLS, World, balance};

pub const MAGIC: &[u8; 4] = b"CSIM";
/// Bump when the World layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 3;

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
    let world: World = postcard::from_bytes(&bytes[8..]).map_err(|e| LoadError::Corrupt(e.to_string()))?;
    check_loaded(&world).map_err(LoadError::Corrupt)?;
    Ok(world)
}

/// Checks on a decoded world for damage the simulation cannot survive
/// (index panics, a road graph pointing at missing nodes, negative or
/// non-finite road numbers that break path search), so a damaged file is
/// reported as corrupt instead of crashing or hanging the game later.
/// Deliberately not the full invariants: worlds put together by writing
/// public fields (tests, dev tooling) must still load.
fn check_loaded(w: &World) -> Result<(), String> {
    let cells = (MAP_CELLS * MAP_CELLS) as usize;
    if w.grid.size != MAP_CELLS || w.grid.cells.len() != cells {
        return Err("grid has the wrong size".into());
    }
    for (n, node) in w.roads.nodes.iter() {
        if !node.pos.iter().all(|x| x.is_finite()) {
            return Err(format!("{n:?} has a non-finite position"));
        }
    }
    for (s, seg) in w.roads.segments.iter() {
        let positive = |x: f32| x.is_finite() && x > 0.0;
        if !positive(seg.length) || !positive(seg.speed_limit) || !seg.capacity.into_iter().all(positive) {
            return Err(format!("{s:?} has a bad length, speed limit or capacity"));
        }
        if !seg.load.iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err(format!("{s:?} has a bad load"));
        }
    }
    w.roads.validate()?;
    let tax = w.economy.tax_rate;
    if !(tax.is_finite() && (0.0..=balance::MAX_TAX_RATE).contains(&tax)) {
        return Err(format!("tax rate {tax}"));
    }
    Ok(())
}
