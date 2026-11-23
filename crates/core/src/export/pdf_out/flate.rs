//! A zlib stream (RFC 1950) of one deflate block with the fixed Huffman
//! codes (RFC 1951 §3.2.6), for the PDF's `/FlateDecode` streams.
//!
//! Depends on: nothing.

// DECISION: a small encoder written here instead of a deflate crate (no
// new dependency, CLAUDE.md). Fixed codes and greedy LZ77 over a 32 KiB
// window with a bounded hash chain: PDF path text (digits, spaces and a
// few operators, repeated) shrinks several times, which is all a page
// needs; dynamic Huffman tables would gain a further fraction.

/// The deflate window: matches reach at most this far back.
const WINDOW: usize = 1 << 15;
/// Shortest and longest match deflate can code.
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Candidates tried per position: the speed / size trade.
const MAX_CHAIN: usize = 48;
/// Hash table size, a power of two.
const HASH_BITS: u32 = 15;

/// Base length of length codes 257..=285 and their extra bits.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
/// Base distance of distance codes 0..=29 and their extra bits.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// `data` as a zlib stream.
pub(super) fn zlib(data: &[u8]) -> Vec<u8> {
    let mut bits = Bits {
        out: Vec::with_capacity(data.len() / 3 + 16),
        acc: 0,
        n: 0,
    };
    // CMF 0x78: deflate, 32 KiB window; FLG 0x01: no dictionary, and
    // 0x7801 is a multiple of 31.
    bits.out.extend_from_slice(&[0x78, 0x01]);
    // BFINAL = 1, BTYPE = 01 (fixed codes).
    bits.put(1, 1);
    bits.put(1, 2);
    let mut head = vec![usize::MAX; 1 << HASH_BITS];
    let mut prev = vec![usize::MAX; WINDOW];
    let mut i = 0;
    while i < data.len() {
        let (len, dist) = longest(data, i, &head, &prev);
        if len >= MIN_MATCH {
            bits.length(len);
            bits.distance(dist);
            for j in i..i + len {
                insert(data, j, &mut head, &mut prev);
            }
            i += len;
        } else {
            bits.literal(u16::from(data[i]));
            insert(data, i, &mut head, &mut prev);
            i += 1;
        }
    }
    bits.literal(256);
    bits.flush();
    bits.out.extend_from_slice(&adler32(data).to_be_bytes());
    bits.out
}

/// The hash of the three bytes at `i`; `None` near the end.
fn hash(data: &[u8], i: usize) -> Option<usize> {
    let b = data.get(i..i + MIN_MATCH)?;
    let v = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
    Some((v.wrapping_mul(2_654_435_761) >> (32 - HASH_BITS)) as usize)
}

/// Records position `i` in the hash chains.
fn insert(data: &[u8], i: usize, head: &mut [usize], prev: &mut [usize]) {
    if let Some(h) = hash(data, i) {
        prev[i % WINDOW] = head[h];
        head[h] = i;
    }
}

/// The longest earlier match of the bytes at `i` within the window, as
/// (length, distance); length 0 when there is none.
fn longest(data: &[u8], i: usize, head: &[usize], prev: &[usize]) -> (usize, usize) {
    let Some(h) = hash(data, i) else { return (0, 0) };
    let max = MAX_MATCH.min(data.len() - i);
    let mut best = (0, 0);
    let mut candidate = head[h];
    let mut tries = 0;
    while candidate != usize::MAX && i - candidate <= WINDOW && tries < MAX_CHAIN {
        let len = data[candidate..].iter().zip(&data[i..i + max]).take_while(|(a, b)| a == b).count();
        if len > best.0 {
            best = (len, i - candidate);
            if len == max {
                break;
            }
        }
        let next = prev[candidate % WINDOW];
        // A stale slot (overwritten by a newer position) ends the chain.
        if next == usize::MAX || next >= candidate {
            break;
        }
        candidate = next;
        tries += 1;
    }
    best
}

/// The Adler-32 checksum (RFC 1950 §8).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// Deflate's bit order: values least significant bit first, Huffman
/// codes most significant bit first.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    /// The low `count` bits of `value`, least significant first.
    fn put(&mut self, value: u32, count: u32) {
        self.acc |= value << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// A Huffman code of `count` bits, most significant first.
    fn code(&mut self, code: u32, count: u32) {
        self.put(code.reverse_bits() >> (32 - count), count);
    }

    /// A literal byte or end of block (256), or the symbol of a length
    /// code (257..=287), in the fixed code.
    fn literal(&mut self, symbol: u16) {
        let s = u32::from(symbol);
        match symbol {
            0..=143 => self.code(0x30 + s, 8),
            144..=255 => self.code(0x190 + s - 144, 9),
            256..=279 => self.code(s - 256, 7),
            _ => self.code(0xC0 + s - 280, 8),
        }
    }

    fn length(&mut self, len: usize) {
        let i = LEN_BASE.iter().rposition(|&b| usize::from(b) <= len).unwrap_or(0);
        let code = u16::try_from(257 + i).unwrap_or(285);
        self.literal(code);
        let extra = u32::from(LEN_EXTRA[i]);
        if extra > 0 {
            self.put(u32::try_from(len).unwrap_or(0) - u32::from(LEN_BASE[i]), extra);
        }
    }

    fn distance(&mut self, dist: usize) {
        let i = DIST_BASE.iter().rposition(|&b| usize::from(b) <= dist).unwrap_or(0);
        self.code(u32::try_from(i).unwrap_or(0), 5);
        let extra = u32::from(DIST_EXTRA[i]);
        if extra > 0 {
            self.put(u32::try_from(dist).unwrap_or(0) - u32::from(DIST_BASE[i]), extra);
        }
    }

    /// Pads the last byte with zeros.
    fn flush(&mut self) {
        if self.n > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler32_of_wikipedia() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn empty_input_is_the_known_stream() {
        // zlib.compress(b"", level with fixed codes) == 78 01 03 00 00 00 00 01
        assert_eq!(zlib(b""), [0x78, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn repeated_text_shrinks() {
        let text = "12.5 30.25 l\n".repeat(1000);
        assert!(zlib(text.as_bytes()).len() < text.len() / 20);
    }

    /// The stream inflates back to its input: wrapped as the image data
    /// of a one-row grey PNG and read by resvg's PNG decoder.
    #[cfg(feature = "png")]
    #[test]
    fn zlib_round_trips_through_a_png_decoder() {
        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = !0_u32;
            for &b in bytes {
                crc ^= u32::from(b);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            !crc
        }
        fn chunk(png: &mut Vec<u8>, kind: [u8; 4], body: &[u8]) {
            png.extend_from_slice(&u32::try_from(body.len()).expect("small").to_be_bytes());
            let mut typed = kind.to_vec();
            typed.extend_from_slice(body);
            png.extend_from_slice(&typed);
            png.extend_from_slice(&crc32(&typed).to_be_bytes());
        }
        // Text-like bytes with long repeats and far matches, then noise.
        let mut pixels: Vec<u8> = "0 0 m 10.25 0 l 10.25 4.5 l h f\n".repeat(400).into_bytes();
        let mut x = 7_u32;
        for _ in 0..5000 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            pixels.push((x >> 16).to_le_bytes()[0]);
        }
        pixels.extend_from_within(..3000);
        let width = u32::try_from(pixels.len()).expect("small");
        let mut raw = vec![0_u8]; // filter: none
        raw.extend_from_slice(&pixels);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = width.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grey
        chunk(&mut png, *b"IHDR", &ihdr);
        chunk(&mut png, *b"IDAT", &zlib(&raw));
        chunk(&mut png, *b"IEND", &[]);
        let decoded = resvg::tiny_skia::Pixmap::decode_png(&png).expect("inflates");
        let grey: Vec<u8> = decoded.pixels().iter().map(|p| p.red()).collect();
        assert_eq!(grey, pixels);
    }
}
