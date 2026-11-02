//! These tests also run under Miri, which checks all of strpack's unsafe code: `cargo +nightly miri test`.

use strpack::{Reader, Writer};

/// A tiny xorshift generator, so the tests don't need any dependencies.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Returns a string of exactly `len` bytes, which has some multi-byte characters unless it's `ascii`.
fn make_string(rng: &mut Rng, len: usize, ascii: bool) -> String {
    let mut s = String::with_capacity(len);
    while s.len() < len {
        let c = if ascii || rng.below(4) != 0 {
            char::from(b'!' + rng.below(94) as u8)
        } else {
            ['é', '€', '😀', 'ß'][rng.below(4)]
        };
        s.push(if c.len_utf8() <= len - s.len() { c } else { 'a' });
    }
    s
}

/// The number of bytes `Writer::write_str` writes for the given string.
fn encoded_len(s: &str) -> usize {
    s.len() + if s.len() < 255 { 1 } else { 5 }
}

/// Writes a count and then the given strings, and returns the bytes.
fn encode(strings: &[String]) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.write_u8(7);
    writer.write_count(strings.len());
    strings.iter().for_each(|s| writer.write_str(s));
    writer.into_bytes()
}

/// Reads what `encode` wrote, requiring that the input has nothing else in it.
fn decode(bytes: &[u8]) -> Option<Vec<&str>> {
    let mut reader = Reader::new(bytes);
    if reader.read_u8()? != 7 { return None }
    let count = reader.read_count(1)?;
    let strings = (0..count).map(|_| reader.read_str()).collect::<Option<Vec<_>>>()?;
    reader.is_empty().then_some(strings)
}

#[test]
fn round_trips_every_length() {
    let mut rng = Rng(0x5eed);
    let max_len = if cfg!(miri) { 300 } else { 1_000 };
    let offsets = if cfg!(miri) { 4 } else { 16 };
    for len in 0..=max_len {
        for ascii in [true, false] {
            let s = make_string(&mut rng, len, ascii);
            // Different amounts of bytes before the string make the copies start at different alignments.
            for offset in 0..offsets {
                let mut writer = Writer::default();
                (0..offset).for_each(|i| writer.write_u8(i as u8));
                writer.write_str(&s);
                writer.write_count(len);
                let bytes = writer.into_bytes();
                assert_eq!(bytes.len(), offset + encoded_len(&s) + 4);

                let mut reader = Reader::new(&bytes);
                (0..offset).for_each(|i| assert_eq!(reader.read_u8(), Some(i as u8)));
                assert_eq!(reader.read_str(), Some(s.as_str()));
                // Nothing comes after that count, so it can only be valid if it's 0.
                assert_eq!(reader.read_count(1), (len == 0).then_some(0));
                assert!(reader.is_empty());
            }
        }
    }
}

#[test]
fn long_strings_use_a_u32_length() {
    let mut rng = Rng(0x10_0000);
    let lens: &[usize] = if cfg!(miri) { &[254, 255, 256, 4_000] } else { &[254, 255, 256, 65_535, 65_536, 1_000_000] };
    for &len in lens {
        let strings = vec![make_string(&mut rng, len, false), make_string(&mut rng, len, true)];
        let bytes = encode(&strings);
        let header = &bytes[5..10];
        if len < 255 {
            assert_eq!(header[0] as usize, len);
        } else {
            assert_eq!(header[0], 255);
            assert_eq!(u32::from_le_bytes(header[1..5].try_into().unwrap()) as usize, len);
        }
        assert_eq!(decode(&bytes), Some(strings.iter().map(String::as_str).collect()));
    }
}

#[test]
fn exact_capacity_never_grows() {
    let mut rng = Rng(0xcafe);
    let strings: Vec<String> = (0..300).map(|i| make_string(&mut rng, i, i % 3 == 0)).collect();
    let exact_len = 1 + 4 + strings.iter().map(|s| encoded_len(s)).sum::<usize>();
    let mut writer = Writer::with_capacity(exact_len);
    writer.write_u8(7);
    writer.write_count(strings.len());
    strings.iter().for_each(|s| writer.write_str(s));
    let bytes = writer.into_bytes();
    assert_eq!(bytes.len(), exact_len);
    assert_eq!(bytes.capacity(), exact_len);
}

#[test]
fn counts_are_bounded_by_the_rest_of_the_input() {
    let mut writer = Writer::default();
    writer.write_count(3);
    ["ab", "cd", "ef"].iter().for_each(|s| writer.write_str(s));
    writer.write_count(u32::MAX as usize);
    let bytes = writer.into_bytes();

    // Each of those 3 strings takes 3 bytes, so 3 items of up to 4 bytes each can fit, but not 5 bytes each.
    assert_eq!(Reader::new(&bytes).read_count(4), Some(3));
    assert_eq!(Reader::new(&bytes).read_count(5), None);
    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.read_count(3), Some(3));
    assert_eq!((0..3).map(|_| reader.read_str()).collect::<Vec<_>>(), [Some("ab"), Some("cd"), Some("ef")]);
    assert_eq!(reader.read_count(0), None);
    assert!(reader.is_empty());
}

#[test]
fn truncated_input_is_never_misread() {
    let mut rng = Rng(0xdead_beef);
    let strings: Vec<String> = [0, 3, 4, 7, 8, 15, 16, 31, 32, 33, 64, 65, 254, 255, 300]
        .iter()
        .map(|&len| make_string(&mut rng, len, len % 2 == 0))
        .collect();
    let bytes = encode(&strings);
    for end in 0..bytes.len() {
        assert_eq!(decode(&bytes[..end]), None, "a truncation to {end} bytes was accepted");
    }
    assert_eq!(decode(&bytes), Some(strings.iter().map(String::as_str).collect()));
}

#[test]
fn corrupted_input_never_panics() {
    let mut rng = Rng(0xbad_c0de);
    let strings: Vec<String> = (0..40).map(|i| make_string(&mut rng, i * 9, i % 2 == 0)).collect();
    let bytes = encode(&strings);
    let rounds = if cfg!(miri) { 300 } else { 20_000 };
    for round in 0..rounds {
        let mut corrupted = bytes.clone();
        match round % 4 {
            0 => {
                let i = rng.below(corrupted.len());
                corrupted[i] ^= 1 << rng.below(8);
            }
            1 => {
                let start = rng.below(corrupted.len());
                let end = (start + rng.below(64)).min(corrupted.len());
                corrupted[start..end].fill(0);
            }
            2 => corrupted.extend((0..rng.below(16) + 1).map(|_| rng.next() as u8)),
            _ => corrupted = (0..rng.below(300)).map(|_| rng.next() as u8).collect(),
        }
        // Any strings it does read must still be valid UTF-8, which `&str` guarantees.
        if let Some(read) = decode(&corrupted) {
            assert!(read.iter().all(|s| s.chars().count() <= s.len()));
        }
    }
}

#[test]
fn invalid_utf8_is_rejected() {
    for invalid in [&[0xC3, 0x28][..], &[b'a', 0x80], &[0xE2, 0x82], &[0xF0, 0x9F, 0x98], &[0xFF; 9]] {
        let mut bytes = vec![invalid.len() as u8];
        bytes.extend_from_slice(invalid);
        assert_eq!(Reader::new(&bytes).read_str(), None);
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
#[should_panic(expected = "strpack counts must fit in a u32")]
fn too_big_counts_panic() {
    Writer::default().write_count(u32::MAX as usize + 1);
}
