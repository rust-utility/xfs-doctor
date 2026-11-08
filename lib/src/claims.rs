//! Claims ledger: byte ranges already carved by earlier passes.
//!
//! Later passes consult it so they neither re-carve known content nor lose it:
//! a hit that lies inside a claim is skipped, unless this pass can prove it is
//! *longer* than the claimed range (a bigger `--max` finding the rest of the
//! file) — then the claim is extended and the file is carved again.
//!
//! Format: `start \t end \t type \t length` per line, one row per accepted hit.

use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// Sorted, non-overlapping `(start, end)` byte ranges.
    ranges: Vec<(u64, u64)>,
}

impl Claims {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.ranges.iter().map(|(s, e)| e - s).sum()
    }

    /// Insert `[start, end)`, merging with neighbours.
    ///
    /// The common case — ranges arriving in ascending order, as carving produces
    /// them — is O(1) amortised; only an out-of-order insert rebuilds the vector.
    pub fn add(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        match self.ranges.last_mut() {
            None => {
                self.ranges.push((start, end));
                return;
            }
            Some((last_start, last_end)) => {
                if start > *last_end {
                    self.ranges.push((start, end));
                    return;
                }
                if start >= *last_start {
                    *last_end = (*last_end).max(end);
                    return;
                }
            }
        }

        // out of order: merge in place
        let mut merged = Vec::with_capacity(self.ranges.len() + 1);
        let mut inserted = false;
        for (s, e) in self.ranges.drain(..) {
            if e < start {
                merged.push((s, e));
            } else if s > end {
                if !inserted {
                    merged.push((start, end));
                    inserted = true;
                }
                merged.push((s, e));
            } else {
                let ns = s.min(start);
                let ne = e.max(end);
                merged.push((ns, ne));
                inserted = true;
            }
        }
        if !inserted {
            merged.push((start, end));
        }
        merged.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(merged.len());
        for (s, e) in merged {
            match out.last_mut() {
                Some((_, le)) if s <= *le => *le = (*le).max(e),
                _ => out.push((s, e)),
            }
        }
        self.ranges = out;
    }

    /// The claimed range containing `offset`, if any.
    pub fn find(&self, offset: u64) -> Option<(u64, u64)> {
        let mut lo = 0usize;
        let mut hi = self.ranges.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (s, e) = self.ranges[mid];
            if offset < s {
                hi = mid;
            } else if offset >= e {
                lo = mid + 1;
            } else {
                return Some((s, e));
            }
        }
        None
    }

    /// Whether `offset` is already covered by a range at least `end` long.
    pub fn covers(&self, offset: u64, end: u64) -> bool {
        matches!(self.find(offset), Some((_, claimed_end)) if claimed_end >= end)
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let mut claims = Self::new();
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(claims),
            Err(e) => return Err(e),
        };
        let mut rows: Vec<(u64, u64)> = Vec::new();
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let start: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
            let end: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
            if end > start {
                rows.push((start, end));
            }
        }
        rows.sort_unstable();
        for (start, end) in rows {
            claims.add(start, end);
        }
        Ok(claims)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut text = String::from("# start\tend\n");
        for (s, e) in &self.ranges {
            text.push_str(&format!("{s}\t{e}\n"));
        }
        fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_merges_overlapping_and_adjacent_ranges() {
        let mut c = Claims::new();
        c.add(100, 200);
        c.add(150, 250); // overlaps -> 100..250
        c.add(250, 260); // adjacent -> 100..260
        c.add(500, 600);
        assert_eq!(c.len(), 2);
        assert_eq!(c.total_bytes(), 260);
        assert_eq!(c.find(100), Some((100, 260)));
        assert_eq!(c.find(559), Some((500, 600)));
        assert_eq!(c.find(300), None);
    }

    #[test]
    fn add_merges_when_new_range_bridges_two() {
        let mut c = Claims::new();
        c.add(100, 200);
        c.add(400, 500);
        c.add(150, 450); // bridges both
        assert_eq!(c.len(), 1);
        assert_eq!(c.find(0), None);
        assert_eq!(c.find(499), Some((100, 500)));
    }

    #[test]
    fn covers_accepts_equal_or_longer_only() {
        let mut c = Claims::new();
        c.add(1000, 2000);
        assert!(c.covers(1000, 2000));
        assert!(c.covers(1000, 1500));
        assert!(!c.covers(1000, 2001), "longer hit must be re-carved");
        assert!(!c.covers(999, 1500));
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("claims-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claims.tsv");
        let mut c = Claims::new();
        c.add(10, 20);
        c.add(30, 40);
        c.save(&path).unwrap();
        let loaded = Claims::load(&path).unwrap();
        assert_eq!(loaded, c);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_file_is_an_empty_ledger() {
        let loaded = Claims::load(Path::new("/nonexistent/claims.tsv")).unwrap();
        assert!(loaded.is_empty());
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    #[test]
    fn ascending_inserts_stay_linear() {
        let mut c = Claims::new();
        let t = std::time::Instant::now();
        for i in 0..400_000u64 {
            c.add(i * 1000, i * 1000 + 500);
        }
        assert_eq!(c.len(), 400_000);
        assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
    }
}
