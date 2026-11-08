//! Signature carving: recover files by content, independent of any filesystem
//! mapping.
//!
//! The image this was written for mixes two RAID layouts, so block→logical
//! mapping cannot be trusted. Carving only relies on the bytes themselves: find a
//! signature, follow the format's own structure to the end, copy the range.
//!
//! Every carve is validated structurally (chunk walk for PNG, box walk for MP4,
//! terminator search for JPEG/PDF, EOCD for ZIP, length field for RIFF), which
//! keeps false positives low.

use std::io::{self, Read, Seek, SeekFrom};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Png,
    Gzip,
    Jpeg,
    Pdf,
    Zip,
    Riff,
    Mp4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub name: &'static str,
    pub ext: &'static str,
    pub magic: &'static [u8],
    pub kind: Kind,
    /// Bytes to skip back from the magic to the real start (e.g. MP4 box size).
    pub lookbehind: u64,
}

pub const SIGNATURES: &[Signature] = &[
    Signature { name: "png", ext: "png", magic: b"\x89PNG\r\n\x1a\n", kind: Kind::Png, lookbehind: 0 },
    Signature { name: "jpeg", ext: "jpg", magic: b"\xff\xd8\xff", kind: Kind::Jpeg, lookbehind: 0 },
    Signature { name: "pdf", ext: "pdf", magic: b"%PDF-", kind: Kind::Pdf, lookbehind: 0 },
    Signature { name: "zip", ext: "zip", magic: b"PK\x03\x04", kind: Kind::Zip, lookbehind: 0 },
    Signature { name: "riff", ext: "riff", magic: b"RIFF", kind: Kind::Riff, lookbehind: 0 },
    Signature { name: "mp4", ext: "mp4", magic: b"ftyp", kind: Kind::Mp4, lookbehind: 4 },
    Signature { name: "gzip", ext: "gz", magic: b"\x1f\x8b\x08", kind: Kind::Gzip, lookbehind: 0 },
];

/// A carved range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub start: u64,
    pub len: u64,
    pub sig: &'static Signature,
}

const MAX_BOX: u64 = 1 << 40;

/// RIFF form types worth carving; anything else after `RIFF<size>` is noise.
/// (`RIFF` is common in random data, the four-byte form type is not.)
pub const RIFF_FORMS: [&[u8]; 5] = [b"AVI ", b"WAVE", b"WEBP", b"CDXA", b"RMID"];

fn riff_ok(body: &[u8]) -> bool {
    body.len() >= 12 && RIFF_FORMS.iter().any(|form| *form == &body[8..12])
}

fn read_exact_at<R: Read + Seek>(r: &mut R, offset: u64, buf: &mut [u8]) -> io::Result<bool> {
    r.seek(SeekFrom::Start(offset))?;
    match r.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

/// Scan `[from, from + window)` for `needle`, returning absolute offsets.
fn find_all(buf: &[u8], needle: &[u8], from: u64, out: &mut Vec<(u64, usize)>) {
    if needle.is_empty() || buf.len() < needle.len() {
        return;
    }
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= buf.len() {
        match buf[i..].iter().position(|b| *b == first) {
            Some(p) => {
                let at = i + p;
                if at + needle.len() > buf.len() {
                    break;
                }
                if &buf[at..at + needle.len()] == needle {
                    out.push((from + at as u64, at));
                }
                i = at + 1;
            }
            None => break,
        }
    }
}

/// Cheap path: resolve the end using only the already-read window.
///
/// Returns `Some(len)` when the end is inside `window`, `None` when the caller has
/// to fall back to `find_end` (which seeks).
pub fn find_end_in_window(window: &[u8], start: usize, sig: &Signature, max_size: u64) -> Option<u64> {
    let limit = window.len().min(start + max_size as usize);
    let body = &window[start..limit];
    match sig.kind {
        Kind::Riff => {
            if body.len() < 12 || !riff_ok(body) {
                return None;
            }
            let size = u32::from_le_bytes([body[4], body[5], body[6], body[7]]) as u64;
            let total = 8 + size;
            (total <= max_size && total as usize <= body.len()).then_some(total)
        }
        Kind::Png => {
            let mut pos = 8usize;
            while pos + 8 <= body.len() {
                let len = u32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as u64;
                let next = pos + 12 + len as usize;
                if next > body.len() || next <= pos {
                    return None;
                }
                if &body[pos + 4..pos + 8] == b"IEND" {
                    return Some(next as u64);
                }
                pos = next;
            }
            None
        }
        Kind::Mp4 => {
            let mut pos = 0usize;
            loop {
                if pos + 8 > body.len() {
                    return None;
                }
                let mut size = u32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as u64;
                if size == 1 {
                    if pos + 16 > body.len() {
                        return None;
                    }
                    size = u64::from_be_bytes(body[pos + 8..pos + 16].try_into().ok()?);
                }
                if size == 0 || size > body.len() as u64 {
                    return None;
                }
                pos += size as usize;
                if pos > body.len() {
                    return None;
                }
                if pos == body.len() {
                    return Some(pos as u64);
                }
            }
        }
        Kind::Gzip => None,
        Kind::Jpeg | Kind::Pdf | Kind::Zip => {
            let terminator: &[u8] = match sig.kind {
                Kind::Jpeg => b"\xff\xd9",
                Kind::Pdf => b"%%EOF",
                _ => b"PK\x05\x06",
            };
            let i = find(&body[2.min(body.len())..], terminator)? + 2.min(body.len());
            let mut end = i + terminator.len();
            if sig.kind == Kind::Pdf {
                if body.get(end) == Some(&b'\r') && body.get(end + 1) == Some(&b'\n') {
                    end += 2;
                } else if matches!(body.get(end), Some(b'\n') | Some(b'\r')) {
                    end += 1;
                }
            }
            if sig.kind == Kind::Zip && end + 2 <= body.len() {
                end += u16::from_le_bytes([body[end], body[end + 1]]) as usize;
            }
            (end <= body.len()).then_some(end as u64)
        }
    }
}

/// Determine where a file starting at `start` ends, using the format's structure.
pub fn find_end<R: Read + Seek>(
    r: &mut R,
    start: u64,
    sig: &Signature,
    max_size: u64,
) -> io::Result<Option<u64>> {
    match sig.kind {
        Kind::Riff => {
            let mut hdr = [0u8; 12];
            if !read_exact_at(r, start, &mut hdr)? {
                return Ok(None);
            }
            if !riff_ok(&hdr) {
                return Ok(None);
            }
            let size = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as u64;
            let total = 8 + size;
            Ok((total <= max_size).then_some(total))
        }
        Kind::Png => {
            let mut pos = start + 8;
            let mut header = [0u8; 8];
            while pos < start + max_size {
                if !read_exact_at(r, pos, &mut header)? {
                    return Ok(None);
                }
                let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
                let name = &header[4..8];
                let next = pos + 12 + len;
                if next > start + max_size || next <= pos {
                    return Ok(None);
                }
                if name == b"IEND" {
                    return Ok(Some(next - start));
                }
                pos = next;
            }
            Ok(None)
        }
        Kind::Mp4 => {
            let mut pos = start;
            let mut header = [0u8; 8];
            loop {
                if !read_exact_at(r, pos, &mut header)? {
                    return Ok(None);
                }
                let mut size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
                if size == 1 {
                    let mut ext = [0u8; 8];
                    if !read_exact_at(r, pos + 8, &mut ext)? {
                        return Ok(None);
                    }
                    size = u64::from_be_bytes(ext);
                }
                if size == 0 {
                    // box extends to end of file: stop at the cap
                    return Ok((pos - start <= max_size).then_some(pos - start));
                }
                if size > MAX_BOX {
                    return Ok(None);
                }
                pos += size;
                if pos <= start || pos - start > max_size {
                    return Ok(None);
                }
                if pos - start >= max_size {
                    return Ok(None);
                }
            }
        }
        Kind::Gzip => gzip_len(r, start, max_size),
        Kind::Jpeg | Kind::Pdf | Kind::Zip => {
            let terminator: &[u8] = match sig.kind {
                Kind::Jpeg => b"\xff\xd9",
                Kind::Pdf => b"%%EOF",
                _ => b"PK\x05\x06",
            };
            let extra: u64 = match sig.kind {
                Kind::Pdf => 5,
                Kind::Zip => 22,
                _ => 2,
            };
            let mut search_from = start + 2;
            let mut buf = vec![0u8; 1024 * 1024];
            while search_from < start + max_size {
                let want = buf.len().min((start + max_size - search_from) as usize);
                r.seek(SeekFrom::Start(search_from))?;
                let n = r.read(&mut buf[..want])?;
                if n == 0 {
                    return Ok(None);
                }
                if let Some(i) = find(&buf[..n], terminator) {
                    let mut end = search_from + i as u64 + extra;
                    if sig.kind == Kind::Pdf {
                        // PDFs end with "%%EOF" followed by EOL; keep it
                        let mut eol = [0u8; 2];
                        if read_exact_at(r, end, &mut eol)? {
                            if eol[0] == b'\r' && eol[1] == b'\n' {
                                end += 2;
                            } else if eol[0] == b'\n' || eol[0] == b'\r' {
                                end += 1;
                            }
                        }
                    }
                    if sig.kind == Kind::Zip {
                        // zip comment length follows the EOCD signature
                        let mut c = [0u8; 2];
                        if read_exact_at(r, search_from + i as u64 + 20, &mut c)? {
                            end += u16::from_le_bytes(c) as u64;
                        }
                    }
                    let len = end.saturating_sub(start);
                    return Ok((len > 0 && len <= max_size).then_some(len));
                }
                search_from += n as u64 - (terminator.len().min(n) - 1) as u64;
            }
            Ok(None)
        }
    }
}

/// Find the first occurrence of `needle`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        let p = haystack[i..].iter().position(|b| *b == needle[0])?;
        let at = i + p;
        if at + needle.len() <= haystack.len() && &haystack[at..at + needle.len()] == needle {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

/// Scan `[from, to)` of `image` and return validated hits, skipping any hit that
/// starts inside a previously accepted file.
pub fn scan<R: Read + Seek>(
    r: &mut R,
    from: u64,
    to: u64,
    block: usize,
    min_size: u64,
    max_size: u64,
    on_hit: impl FnMut(&Hit) -> io::Result<()>,
) -> io::Result<()> {
    scan_types(r, from, to, block, min_size, max_size, None, on_hit)
}

/// Like [`scan`], but only considers signatures whose `name` is in `types`
/// (`None` = all).
#[allow(clippy::too_many_arguments)]
pub fn scan_types<R: Read + Seek>(
    r: &mut R,
    from: u64,
    to: u64,
    block: usize,
    min_size: u64,
    max_size: u64,
    types: Option<&[&str]>,
    mut on_hit: impl FnMut(&Hit) -> io::Result<()>,
) -> io::Result<()> {
    let overlap = 16usize;
    let sigs: Vec<&Signature> = SIGNATURES
        .iter()
        .filter(|s| types.map(|t| t.contains(&s.name)).unwrap_or(true))
        .collect();
    let mut buf = vec![0u8; block];
    let mut pos = from;
    let mut skip_before = from;

    while pos < to {
        r.seek(SeekFrom::Start(pos))?;
        let want = buf.len().min((to - pos) as usize);
        let n = r.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        let window = &buf[..n];

        let mut candidates: Vec<(u64, &Signature)> = Vec::new();
        for sig in &sigs {
            let mut hits = Vec::new();
            find_all(window, sig.magic, pos, &mut hits);
            for (offset, _) in hits {
                let start = offset.saturating_sub(sig.lookbehind);
                if start >= skip_before {
                    candidates.push((start, sig));
                }
            }
        }
        candidates.sort_by_key(|(s, _)| *s);

        for (start, sig) in candidates {
            if start < skip_before {
                continue;
            }
            let rel = (start - pos) as usize;
            let len = match find_end_in_window(window, rel, sig, max_size) {
                Some(len) => Some(len),
                None => find_end(r, start, sig, max_size)?,
            };
            if let Some(len) = len {
                if len >= min_size {
                    on_hit(&Hit { start, len, sig })?;
                    skip_before = start + len;
                }
            }
        }
        // step forward, keeping an overlap so signatures spanning blocks are seen
        if n < overlap + 1 {
            break;
        }
        pos += (n - overlap) as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn blob(parts: &[(u64, Vec<u8>)], size: usize) -> Vec<u8> {
        let mut data = vec![0x41u8; size];
        for (offset, part) in parts {
            let start = *offset as usize;
            data[start..start + part.len()].copy_from_slice(part);
        }
        data
    }

    fn png() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&[0u8; 13]);
        v.extend_from_slice(&[0u8; 4]); // crc
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(b"IEND");
        v.extend_from_slice(&[0u8; 4]);
        v
    }

    fn jpeg() -> Vec<u8> {
        let mut v = b"\xff\xd8\xff\xe0".to_vec();
        v.extend_from_slice(&[0u8; 32]);
        v.extend_from_slice(b"\xff\xd9");
        v
    }

    fn pdf() -> Vec<u8> {
        let mut v = b"%PDF-1.4\n".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v.extend_from_slice(b"%%EOF\n");
        v
    }

    fn riff() -> Vec<u8> {
        let payload = 100u32;
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&payload.to_le_bytes());
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&vec![0u8; payload as usize - 4]);
        v
    }

    #[test]
    fn carves_png_jpeg_pdf_riff_from_noise() {
        let data = blob(
            &[
                (1000, png()),
                (5000, jpeg()),
                (9000, pdf()),
                (14000, riff()),
            ],
            20000,
        );
        let mut r = Cursor::new(data);
        let mut hits = Vec::new();
        scan(&mut r, 0, 20000, 4096, 16, 1 << 20, |h| {
            hits.push((h.start, h.len, h.sig.name));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            hits,
            vec![
                (1000, png().len() as u64, "png"),
                (5000, jpeg().len() as u64, "jpeg"),
                (9000, pdf().len() as u64, "pdf"),
                (14000, riff().len() as u64, "riff"),
            ]
        );
    }

    #[test]
    fn in_window_path_matches_streaming_path() {
        let data = blob(&[(64, png()), (2048, jpeg()), (4096, pdf())], 8192);
        for sig in SIGNATURES.iter() {
            let start = match sig.name {
                "png" => 64usize,
                "jpeg" => 2048,
                "pdf" => 4096,
                _ => continue,
            };
            let window = &data[start..];
            let fast = find_end_in_window(window, 0, sig, 1 << 20);
            let mut r = Cursor::new(data[start..].to_vec());
            let slow = find_end(&mut r, 0, sig, 1 << 20).unwrap();
            assert_eq!(fast, slow, "{}", sig.name);
        }
    }

    #[test]
    fn type_filter_limits_signatures() {
        let data = blob(&[(64, png()), (2048, riff())], 8192);
        let mut r = Cursor::new(data);
        let mut names = Vec::new();
        scan_types(&mut r, 0, 8192, 4096, 16, 1 << 20, Some(&["riff"]), |h| {
            names.push(h.sig.name);
            Ok(())
        })
        .unwrap();
        assert_eq!(names, vec!["riff"]);
    }

    #[test]
    fn riff_requires_a_known_form_type() {
        let mut good = b"RIFF".to_vec();
        good.extend_from_slice(&100u32.to_le_bytes());
        good.extend_from_slice(b"WAVE");
        good.extend_from_slice(&[0u8; 96]);
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

        let mut r = Cursor::new(good.clone());
        assert!(find_end(&mut r, 0, &SIGNATURES[4], 1 << 20).unwrap().is_some());
        let mut r = Cursor::new(bad);
        assert!(find_end(&mut r, 0, &SIGNATURES[4], 1 << 20).unwrap().is_none());
        assert!(find_end_in_window(&good, 0, &SIGNATURES[4], 1 << 20).is_some());
    }

    #[test]
    fn ignores_signature_without_valid_structure() {
        let mut data = vec![0x42u8; 8192];
        data[100..104].copy_from_slice(b"%PDF"); // no %%EOF anywhere
        data[3000..3008].copy_from_slice(b"\x89PNG\r\n\x1a\n"); // truncated
        let mut r = Cursor::new(data);
        let mut hits = 0;
        scan(&mut r, 0, 8192, 4096, 16, 1 << 20, |_| {
            hits += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(hits, 0);
    }

    #[test]
    fn signature_spanning_a_block_boundary_is_found() {
        let mut data = vec![0u8; 8192];
        let p = png();
        let start = 4090usize; // signature straddles the 4096 boundary
        data[start..start + p.len()].copy_from_slice(&p);
        let mut r = Cursor::new(data);
        let mut hits = Vec::new();
        scan(&mut r, 0, 8192, 4096, 16, 1 << 20, |h| {
            hits.push(h.start);
            Ok(())
        })
        .unwrap();
        assert_eq!(hits, vec![start as u64]);
    }
}

// --------------------------------------------------------------------------- //
// gzip: a structure-only DEFLATE walker
// --------------------------------------------------------------------------- //
//
// gzip has no length field: the only way to know where the member ends is to walk
// the DEFLATE stream. We do not need the output, only its length, so symbols are
// decoded and skipped — and the result is checked against the member trailer's
// ISIZE, which makes false positives essentially impossible.

mod inflate {
    use std::collections::HashMap;

    pub struct Skipped {
        /// Bytes of the DEFLATE stream that were consumed.
        pub consumed: usize,
        /// Uncompressed size, used to validate the gzip trailer.
        pub out_len: u64,
    }

    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u8,
    }

    impl<'a> BitReader<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self { data, pos: 0, bit: 0 }
        }
        fn bit(&mut self) -> Option<u32> {
            if self.pos >= self.data.len() {
                return None;
            }
            let b = ((self.data[self.pos] >> self.bit) & 1) as u32;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
            Some(b)
        }
        fn bits(&mut self, n: u32) -> Option<u32> {
            let mut v = 0u32;
            for i in 0..n {
                v |= self.bit()? << i;
            }
            Some(v)
        }
        fn align(&mut self) {
            if self.bit != 0 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        fn skip(&mut self, n: usize) -> Option<()> {
            if self.pos + n > self.data.len() {
                return None;
            }
            self.pos += n;
            Some(())
        }
        fn consumed(&self) -> usize {
            self.pos
        }
    }

    struct Huffman {
        maps: [HashMap<u16, u16>; 16],
        max: u8,
    }

    fn reverse(mut code: u32, len: u32) -> u16 {
        let mut out = 0u32;
        for _ in 0..len {
            out = (out << 1) | (code & 1);
            code >>= 1;
        }
        out as u16
    }

    impl Huffman {
        fn new(lengths: &[u8]) -> Option<Self> {
            let mut maps: [HashMap<u16, u16>; 16] = Default::default();
            let mut bl_count = [0u32; 16];
            for &l in lengths {
                if l as usize > 15 {
                    return None;
                }
                if l > 0 {
                    bl_count[l as usize] += 1;
                }
            }
            let mut next_code = [0u32; 16];
            let mut code = 0u32;
            for bits in 1..16 {
                code = (code + bl_count[bits - 1]) << 1;
                next_code[bits] = code;
            }
            let mut max = 0u8;
            for (sym, &l) in lengths.iter().enumerate() {
                if l > 0 {
                    let c = next_code[l as usize];
                    next_code[l as usize] += 1;
                    maps[l as usize].insert(reverse(c, l as u32), sym as u16);
                    max = max.max(l);
                }
            }
            if max == 0 {
                return None;
            }
            Some(Self { maps, max })
        }

        fn decode(&mut self, br: &mut BitReader) -> Option<u16> {
            let mut acc = 0u16;
            for len in 1..=self.max as u32 {
                acc |= (br.bit()? as u16) << (len - 1);
                if let Some(&sym) = self.maps[len as usize].get(&acc) {
                    return Some(sym);
                }
            }
            None
        }
    }

    const LEN_BASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEN_EXTRA: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DIST_BASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DIST_EXTRA: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];

    fn fixed_tables() -> (Huffman, Huffman) {
        let mut lit = [0u8; 288];
        for (i, slot) in lit.iter_mut().enumerate() {
            *slot = match i {
                0..=143 => 8,
                144..=255 => 9,
                256..=279 => 7,
                _ => 8,
            };
        }
        let dist = [5u8; 32];
        (Huffman::new(&lit).unwrap(), Huffman::new(&dist).unwrap())
    }

    fn block(
        br: &mut BitReader,
        lit: &mut Huffman,
        dist: &mut Huffman,
        out: &mut u64,
    ) -> Option<()> {
        loop {
            let sym = lit.decode(br)?;
            match sym {
                0..=255 => *out += 1,
                256 => return Some(()),
                257..=285 => {
                    let i = (sym - 257) as usize;
                    let len = LEN_BASE[i] as u64 + br.bits(LEN_EXTRA[i] as u32)? as u64;
                    let dsym = dist.decode(br)?;
                    if dsym > 29 {
                        return None;
                    }
                    let _d = DIST_BASE[dsym as usize] as u64
                        + br.bits(DIST_EXTRA[dsym as usize] as u32)? as u64;
                    *out = out.checked_add(len)?;
                }
                _ => return None,
            }
        }
    }

    fn dynamic_tables(br: &mut BitReader) -> Option<(Huffman, Huffman)> {
        let hlit = br.bits(5)? as usize + 257;
        let hdist = br.bits(5)? as usize + 1;
        let hclen = br.bits(4)? as usize + 4;
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let mut cl = [0u8; 19];
        for i in 0..hclen {
            cl[ORDER[i]] = br.bits(3)? as u8;
        }
        let mut clh = Huffman::new(&cl)?;

        let total = hlit + hdist;
        let mut lengths = vec![0u8; total];
        let mut i = 0usize;
        while i < total {
            let sym = clh.decode(br)?;
            match sym {
                0..=15 => {
                    lengths[i] = sym as u8;
                    i += 1;
                }
                16 => {
                    if i == 0 {
                        return None;
                    }
                    let prev = lengths[i - 1];
                    let rep = 3 + br.bits(2)? as usize;
                    for _ in 0..rep {
                        if i >= total {
                            return None;
                        }
                        lengths[i] = prev;
                        i += 1;
                    }
                }
                17 | 18 => {
                    let rep = if sym == 17 {
                        3 + br.bits(3)? as usize
                    } else {
                        11 + br.bits(7)? as usize
                    };
                    for _ in 0..rep {
                        if i >= total {
                            return None;
                        }
                        lengths[i] = 0;
                        i += 1;
                    }
                }
                _ => return None,
            }
        }
        Some((Huffman::new(&lengths[..hlit])?, Huffman::new(&lengths[hlit..])?))
    }

    /// Walk a raw DEFLATE stream, counting output bytes.
    pub fn skip(data: &[u8]) -> Option<Skipped> {
        let mut br = BitReader::new(data);
        let mut out = 0u64;
        loop {
            let final_block = br.bits(1)?;
            let btype = br.bits(2)?;
            match btype {
                0 => {
                    br.align();
                    let len = br.bits(16)?;
                    let nlen = br.bits(16)?;
                    if (len ^ 0xffff) != nlen {
                        return None;
                    }
                    br.skip(len as usize)?;
                    out += len as u64;
                }
                1 => {
                    let (mut lit, mut dist) = fixed_tables();
                    block(&mut br, &mut lit, &mut dist, &mut out)?;
                }
                2 => {
                    let (mut lit, mut dist) = dynamic_tables(&mut br)?;
                    block(&mut br, &mut lit, &mut dist, &mut out)?;
                }
                _ => return None,
            }
            if final_block == 1 {
                break;
            }
        }
        // the stream ends mid-byte; the trailer starts at the next byte
        br.align();
        Some(Skipped { consumed: br.consumed(), out_len: out })
    }
}

/// Total length of the gzip member starting at `start`, or `None` when the stream
/// does not validate against its trailer.
fn gzip_len<R: Read + Seek>(r: &mut R, start: u64, max_size: u64) -> io::Result<Option<u64>> {
    // header: magic(2) cm(1) flg(1) mtime(4) xfl(1) os(1)
    let mut hdr = vec![0u8; 4096];
    r.seek(SeekFrom::Start(start))?;
    let read = r.read(&mut hdr)?;
    hdr.truncate(read);
    if hdr.len() < 18 {
        return Ok(None);
    }
    if hdr[0] != 0x1f || hdr[1] != 0x8b || hdr[2] != 8 {
        return Ok(None);
    }
    let flg = hdr[3];
    if flg & 0xe0 != 0 {
        return Ok(None); // reserved bits
    }
    let mut pos = 10usize;
    if flg & 0x04 != 0 {
        if pos + 2 > hdr.len() {
            return Ok(None);
        }
        let xlen = u16::from_le_bytes([hdr[pos], hdr[pos + 1]]) as usize;
        pos += 2 + xlen;
    }
    for bit in [0x08u8, 0x10] {
        if flg & bit != 0 {
            while pos < hdr.len() && hdr[pos] != 0 {
                pos += 1;
            }
            pos += 1; // NUL
        }
    }
    if flg & 0x02 != 0 {
        pos += 2;
    }
    if pos >= hdr.len() {
        return Ok(None);
    }

    let data_start = start + pos as u64;
    let cap = max_size.min(1 << 28) as usize;
    let mut buf = vec![0u8; cap];
    r.seek(SeekFrom::Start(data_start))?;
    let n = r.read(&mut buf)?;
    buf.truncate(n);
    let Some(skipped) = inflate::skip(&buf) else {
        return Ok(None);
    };

    let trailer_at = data_start + skipped.consumed as u64;
    let mut trailer = [0u8; 8];
    if !read_exact_at(r, trailer_at, &mut trailer)? {
        return Ok(None);
    }
    let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]) as u64;
    if isize != skipped.out_len & 0xffff_ffff {
        return Ok(None);
    }
    let total = trailer_at + 8 - start;
    Ok((total <= max_size).then_some(total))
}

#[cfg(test)]
mod gzip_tests {
    use super::*;
    use std::io::Cursor;

    /// 52 bytes: 720 B of repeated text, stored/fixed blocks.
    pub(super) const SMALL: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x08, 0xed, 0x04, 0xf1, 0x6a, 0x02, 0x03, 0x74, 0x31, 0x2e, 0x74, 0x78, 0x74,
        0x00, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0x48, 0xaf, 0xca, 0x2c, 0x50, 0x48, 0x4e, 0x2c, 0x2a,
        0x4b, 0x2d, 0xe2, 0xca, 0x18, 0x15, 0x19, 0x15, 0x19, 0xb2, 0x22, 0x00, 0x50, 0xcb, 0x18, 0xe7,
        0xd0, 0x02, 0x00, 0x00,
    ];

    #[test]
    fn finds_the_end_of_a_small_member() {
        let mut r = Cursor::new(SMALL.to_vec());
        let len = gzip_len(&mut r, 0, 1 << 20).unwrap();
        assert_eq!(len, Some(SMALL.len() as u64));
    }

    #[test]
    fn rejects_a_member_whose_trailer_disagrees() {
        let mut bad = SMALL.to_vec();
        let n = bad.len();
        bad[n - 2] ^= 0xff; // corrupt ISIZE (last four bytes of the trailer)
        let mut r = Cursor::new(bad);
        assert_eq!(gzip_len(&mut r, 0, 1 << 20).unwrap(), None);
    }

    #[test]
    fn rejects_a_corrupt_stream() {
        let mut bad = SMALL.to_vec();
        bad[20] ^= 0x55; // damage the deflate data
        let mut r = Cursor::new(bad);
        assert!(gzip_len(&mut r, 0, 1 << 20).unwrap().is_none());
    }

    #[test]
    fn carve_finds_a_gzip_member_in_noise() {
        let mut data = vec![0x41u8; 4096];
        data[1000..1000 + SMALL.len()].copy_from_slice(SMALL);
        let mut r = Cursor::new(data);
        let mut hits = Vec::new();
        scan_types(&mut r, 0, 4096, 4096, 16, 1 << 20, Some(&["gzip"]), |h| {
            hits.push((h.start, h.len));
            Ok(())
        })
        .unwrap();
        assert_eq!(hits, vec![(1000, SMALL.len() as u64)]);
    }

    #[test]
    fn ignores_a_bare_magic_without_a_stream() {
        let mut data = vec![0u8; 2048];
        data[64..67].copy_from_slice(b"\x1f\x8b\x08");
        let mut r = Cursor::new(data);
        let mut hits = 0;
        scan_types(&mut r, 0, 2048, 1024, 16, 1 << 20, Some(&["gzip"]), |_| {
            hits += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(hits, 0);
    }
}
