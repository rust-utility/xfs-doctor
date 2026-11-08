//! RAID0 stripe mapping for a single surviving member.
//!
//! A 2-disk RAID0 interleaves fixed-size chunks: logical chunk `k` lives on member
//! `k % disks`, at `data_offset + (k / disks) * chunk + (offset within chunk)`.
//! With one member, roughly `1/disks` of the address space is present — the rest
//! belongs to a disk that is gone.
//!
//! This type answers two questions: "is this logical offset on my member, and
//! where?" and "which logical ranges does my member cover?" — the second is what
//! you need to build a sparse reconstruction.

use crate::xfs::error::XfsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raid0Half {
    /// Byte offset where the data area starts on each member (md `data_offset`).
    data_offset: u64,
    /// Stripe chunk size in bytes.
    chunk: u64,
    /// Array width (number of members).
    disks: u32,
    /// Index of the member we hold, 0-based.
    member: u32,
}

impl Raid0Half {
    pub fn new(data_offset: u64, chunk: u64, disks: u32, member: u32) -> Result<Self, XfsError> {
        if chunk == 0 {
            return Err(XfsError::OutOfRange {
                kind: "chunk size",
                value: 0,
                max: u64::MAX,
            });
        }
        if disks == 0 {
            return Err(XfsError::OutOfRange {
                kind: "disk count",
                value: 0,
                max: u64::MAX,
            });
        }
        if member >= disks {
            return Err(XfsError::OutOfRange {
                kind: "member index",
                value: member as u64,
                max: (disks - 1) as u64,
            });
        }
        Ok(Self {
            data_offset,
            chunk,
            disks,
            member,
        })
    }

    pub fn chunk(&self) -> u64 {
        self.chunk
    }

    pub fn disks(&self) -> u32 {
        self.disks
    }

    pub fn member(&self) -> u32 {
        self.member
    }

    /// Total addressable bytes of the array given the data area of one member.
    pub fn logical_capacity(&self, member_data_area: u64) -> u64 {
        member_data_area / self.chunk * self.disks as u64 * self.chunk
    }

    /// Where `logical` lives on this member, or `None` when it belongs to a
    /// missing disk.
    pub fn disk_offset(&self, logical: u64) -> Option<u64> {
        let index = logical / self.chunk;
        if (index as u32) % self.disks != self.member {
            return None;
        }
        let within = logical % self.chunk;
        let pair = index / self.disks as u64;
        Some(self.data_offset + pair * self.chunk + within)
    }

    /// Logical ranges (start, length) covered by this member, merged when chunks
    /// are adjacent — i.e. what a reconstruction should fill in.
    pub fn present_ranges(&self, logical_len: u64) -> Vec<(u64, u64)> {
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        let mut index = self.member as u64;
        while index * self.chunk < logical_len {
            let start = index * self.chunk;
            let end = ((index + 1) * self.chunk).min(logical_len);
            match ranges.last_mut() {
                Some((last_start, last_len)) if start == *last_start + *last_len => {
                    *last_len += end - start;
                }
                _ => ranges.push((start, end - start)),
            }
            index += self.disks as u64;
        }
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: u64 = 512 * 1024;
    const D: u64 = 8192;

    fn half(member: u32) -> Raid0Half {
        Raid0Half::new(D, C, 2, member).unwrap()
    }

    #[test]
    fn member_zero_holds_even_chunks() {
        let map = half(0);
        assert_eq!(map.disk_offset(0), Some(D));
        assert_eq!(map.disk_offset(C), None);
        assert_eq!(map.disk_offset(2 * C + 100), Some(D + C + 100));
    }

    #[test]
    fn member_one_holds_odd_chunks() {
        let map = half(1);
        assert_eq!(map.disk_offset(0), None);
        assert_eq!(map.disk_offset(C), Some(D));
        assert_eq!(map.disk_offset(C + 100), Some(D + 100));
        assert_eq!(map.disk_offset(3 * C), Some(D + C));
    }

    #[test]
    fn capacity_is_whole_chunks_across_the_array() {
        // four whole chunks on one member cover eight logical chunks on two disks
        let map = half(0);
        assert_eq!(map.logical_capacity(4 * C + 7), 8 * C);
        assert_eq!(map.logical_capacity(C), 2 * C);
        assert_eq!(map.logical_capacity(C - 1), 0);
    }

    #[test]
    fn present_ranges_cover_half_the_space() {
        let map = half(1);
        let ranges = map.present_ranges(8 * C);
        // chunk 1, 3, 5, 7 — none adjacent, so four separate ranges
        assert_eq!(ranges, vec![(C, C), (3 * C, C), (5 * C, C), (7 * C, C)]);
        let covered: u64 = ranges.iter().map(|(_, len)| len).sum();
        assert_eq!(covered, 4 * C);
    }

    #[test]
    fn present_ranges_merge_adjacent_chunks() {
        // width 1 behaves like a plain device: one range
        let map = Raid0Half::new(D, C, 1, 0).unwrap();
        assert_eq!(map.present_ranges(3 * C), vec![(0, 3 * C)]);
    }

    #[test]
    fn rejects_bad_geometry() {
        assert!(Raid0Half::new(D, 0, 2, 0).is_err());
        assert!(Raid0Half::new(D, C, 0, 0).is_err());
        assert!(Raid0Half::new(D, C, 2, 2).is_err());
    }
}
