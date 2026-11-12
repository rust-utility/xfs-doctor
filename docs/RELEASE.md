Read-only tools for **damaged XFS images**: decode headers at any offset, work out a
RAID0 half-image's geometry, salvage inodes and directories without mounting, and
carve files by content signature when the metadata is gone.

Nothing is ever written to the image.

## What's in 0.1.0

```
xfs-doctor read sb|agi     decode a superblock or AGI at a byte offset (--force like xfs_db -F)
xfs-doctor map             RAID0 single-member math: where a logical offset lives, what survives
xfs-doctor salvage         inodes, directories and extents of the AGs on one member -> files
xfs-doctor carve           PNG, JPEG, PDF, ZIP, RIFF (AVI/WAV), MP4, gzip, tar
```

Every carve is validated by the format's own structure, so a bare magic is never
enough:

| Format | End detection | Validation |
|---|---|---|
| PNG | chunk walk to `IEND` | chunk lengths |
| JPEG | `FFD9` | segment structure |
| PDF | `%%EOF` + EOL | |
| ZIP | EOCD + comment length | central directory at the declared offset, first entry's local header at 0 |
| RIFF | length field | form type must be AVI/WAVE/WEBP/CDXA/RMID |
| MP4 | box walk | a `moov` box must exist; never truncates at a size-0 box |
| gzip | DEFLATE walk without decompression | decoded length must equal the trailer ISIZE |
| tar | header walk to the two zero blocks | per-header checksum |

Passes are **incremental**: each writes a `claims.tsv` ledger of recovered byte
ranges, and later passes skip what is already claimed — or re-carve a claim when a
bigger `--max` finds more of the same file.

## Install

```bash
cargo install --git https://github.com/rust-utility/xfs-doctor
```

or take a binary below. Build from source with `cargo build --release`.

## Suggested reading

The README covers each subcommand, the validation table above in more detail, and
the limits (no names or directory structure from carving; v4 XFS structures only).
