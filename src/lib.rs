pub const DEFAULT_PORT: u16 = 6881;
pub const BLOCK_MAX: usize = 1 << 14;

pub mod download;
pub mod peer;
pub mod piece;
pub mod torrent;
pub mod tracker;
