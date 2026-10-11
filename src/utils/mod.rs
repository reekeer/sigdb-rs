//! Small helpers shared by the format code.

pub mod hashing;
pub mod varint;

pub use hashing::sha256;
pub use varint::{DecodeResult, decode_varint, encode_varint};
