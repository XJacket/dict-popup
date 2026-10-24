//! `dict-core` - offline StarDict reading for dict-popup.
//!
//! Layering:
//! - [`mmap`]: read-only file mappings (the dictionary is 190 MB; it must not
//!   be read into process memory).
//! - [`stardict`]: `.ifo` parsing, offset table, binary search, fallback chain.
//! - [`normalize`]: turning selected text into an ordered list of candidates.
//! - [`direction`]: inferring a dictionary's language pair from its data.

pub mod direction;
pub mod mmap;
pub mod normalize;
pub mod stardict;

pub use direction::{fingerprint, Direction, Fingerprint};
pub use stardict::{Dictionary, Entry, IfoInfo, Lookup};
