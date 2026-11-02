//! A tiny, fast binary encoding for strings and counts.
//!
//! A string is a `u8` length followed by its bytes, or `0xFF` followed by a little-endian `u32`
//! length for strings of 255 bytes or more. A count is a little-endian `u32`.
//!
//! [`Writer`] copies strings of 4 to 64 bytes without calling `memcpy`, and [`Reader`] skips full
//! UTF-8 validation for ASCII strings. [`Reader`] never panics, even on truncated or corrupted input.

use std::{ptr, str};

/// The length byte that means a string's real length is in the next 4 bytes.
const LONG_LEN: u8 = u8::MAX;

/// Writes strings and counts into a byte buffer.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// Creates a writer that can hold `capacity` bytes before it has to grow.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { bytes: Vec::with_capacity(capacity) }
    }

    /// Writes a single byte, e.g., a format version.
    #[inline(always)]
    pub fn write_u8(&mut self, byte: u8) {
        self.bytes.push(byte);
    }

    /// Writes a count as a little-endian `u32`.
    ///
    /// # Panics
    /// If `count` doesn't fit in a `u32`.
    #[inline(always)]
    pub fn write_count(&mut self, count: usize) {
        let count = u32::try_from(count).expect("strpack counts must fit in a u32");
        self.bytes.extend_from_slice(&count.to_le_bytes());
    }

    /// Writes a string's length and then its bytes.
    ///
    /// # Panics
    /// If the string is 4 GiB or longer.
    #[inline(always)]
    pub fn write_str(&mut self, s: &str) {
        let len = s.len();
        let len_len = if len < LONG_LEN as usize {
            1
        } else {
            assert!(u32::try_from(len).is_ok(), "strpack strings must be shorter than 4 GiB");
            5
        };
        self.bytes.reserve(len_len + len);
        let start = self.bytes.len();
        // SAFETY: we reserved `len_len + len` bytes after `start`, and each copy stays within `s` and those bytes.
        unsafe {
            let dst = self.bytes.as_mut_ptr().add(start);
            if len_len == 1 {
                *dst = len as u8;
            } else {
                *dst = LONG_LEN;
                dst.add(1).cast::<[u8; 4]>().write_unaligned((len as u32).to_le_bytes());
            }
            let (src, dst) = (s.as_ptr(), dst.add(len_len));
            match len {
                8..16 => copy_overlapping::<8>(src, dst, len),
                16..32 => copy_overlapping::<16>(src, dst, len),
                // 32-byte copies aren't inlined on targets without SIMD (like 32-bit ARM Android),
                // so we copy the first and last 32 bytes with 16-byte copies instead.
                32..=64 => {
                    copy_overlapping::<16>(src, dst, 32);
                    copy_overlapping::<16>(src.add(len - 32), dst.add(len - 32), 32);
                }
                4..8 => copy_overlapping::<4>(src, dst, len),
                _ => ptr::copy_nonoverlapping(src, dst, len),
            }
            self.bytes.set_len(start + len_len + len);
        }
    }

    /// Returns all of the bytes written so far.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Copies `len` bytes (`N <= len <= 2 * N`) with two overlapping `N`-byte copies, which is much faster than `memcpy`.
///
/// SAFETY: `src` must be readable and `dst` writable for `len` bytes.
#[inline(always)]
unsafe fn copy_overlapping<const N: usize>(src: *const u8, dst: *mut u8, len: usize) {
    unsafe {
        dst.cast::<[u8; N]>().write_unaligned(src.cast::<[u8; N]>().read_unaligned());
        dst.add(len - N).cast::<[u8; N]>().write_unaligned(src.add(len - N).cast::<[u8; N]>().read_unaligned());
    }
}

/// Reads strings and counts from bytes written by a [`Writer`].
///
/// Every read returns `None` instead of panicking if the input is truncated or invalid.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Creates a reader that starts at the beginning of the given bytes.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    /// Returns whether there's nothing left to read.
    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    /// Reads a single byte.
    #[inline(always)]
    pub fn read_u8(&mut self) -> Option<u8> {
        let (&byte, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(byte)
    }

    /// Reads a count of items that each take at least `min_item_len` (1 or more) bytes,
    /// so a corrupted count can't claim more items than the rest of the input could hold.
    #[inline(always)]
    pub fn read_count(&mut self, min_item_len: usize) -> Option<usize> {
        let (count, rest) = self.rest.split_first_chunk::<4>()?;
        self.rest = rest;
        let count = u32::from_le_bytes(*count) as usize;
        (count <= rest.len() / min_item_len.max(1)).then_some(count)
    }

    /// Reads a string written by [`Writer::write_str`].
    #[inline(always)]
    pub fn read_str(&mut self) -> Option<&'a str> {
        let len = match self.read_u8()? {
            LONG_LEN => self.read_count(1)?,
            len => len as usize,
        };
        let (bytes, rest) = self.rest.split_at_checked(len)?;
        self.rest = rest;
        if bytes.is_ascii() {
            // SAFETY: ASCII is valid UTF-8, and it's much faster to check for than all of UTF-8.
            Some(unsafe { str::from_utf8_unchecked(bytes) })
        } else {
            str::from_utf8(bytes).ok()
        }
    }
}
