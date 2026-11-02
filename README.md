# strpack

A tiny, fast, dependency-free binary encoding for strings and counts.

* A string is a `u8` length followed by its bytes, or `0xFF` followed by a little-endian `u32` length
  for strings of 255 bytes or more.
* A count is a little-endian `u32`.

`Writer` copies short strings with a few inline loads and stores instead of calling `memcpy`,
and `Reader` skips full UTF-8 validation for ASCII strings.

`Reader` never panics: it returns `None` on truncated or corrupted input,
and a count can't claim more items than the rest of the input could hold,
so a corrupted count can't make you allocate a huge collection.

## Example

```rust
let mut writer = strpack::Writer::default();
writer.write_count(2);
writer.write_str("hello");
writer.write_str("world");
let bytes = writer.into_bytes();

let mut reader = strpack::Reader::new(&bytes);
let count = reader.read_count(1).unwrap();
let strings: Vec<&str> = (0..count).map(|_| reader.read_str().unwrap()).collect();
assert_eq!(strings, ["hello", "world"]);
```
