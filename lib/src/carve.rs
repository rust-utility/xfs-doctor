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
    mut on_hit: impl FnMut(&Hit) -> io::Result<()>,
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
        good.extend_from_slice(&vec![0u8; 96]);
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
