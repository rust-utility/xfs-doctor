# xfs-doctor

Read-only inspection of XFS on-disk headers — superblock and AGI — straight from a
device or a disk image. No mount, no `xfs_db` dependency, safe on damaged media.

It exists because a NAS left a 2-disk RAID0 member image to deal with: the image
starts with an md superblock, the filesystem does not start at offset 0, and
`xfs_db` refuses to look past the foreign magic. `xfs-doctor` decodes at an
arbitrary byte offset and tells you what is actually there.

## Build

```bash
cargo build --release          # target/release/xfs-doctor
cargo test                     # unit tests, no fixture files needed
```

## Usage

```bash
# superblock at offset 0
xfs-doctor read --device sda4.img sb

# superblock at an arbitrary offset (hex accepted)
xfs-doctor read --device sda4.img --offset 0x100000 sb

# decode a foreign/garbage header anyway (like xfs_db -F)
xfs-doctor read --device sda4.img sb --force

# AGI by offset, or by allocation group number (offset derived from the superblock)
xfs-doctor read --device sda4.img --offset 0x2000 agi
xfs-doctor read --device sda4.img agi --ag 3
```

Output is a hexdump of the decoded bytes followed by the parsed fields. A wrong
magic is an error, not a silent garbage dump:

```
$ xfs-doctor read -d sda4.img sb
Error: cannot decode superblock at offset 0x0: bad superblock magic: found 0xfc4e2ba9, expected 0x58465342
```

## What it understands

| Header | Magic | Size | Fields |
|---|---|---|---|
| superblock | `XFSB` (0x58465342) | 204 B (v4 prefix) | geometry, UUID, allocation groups, inode sizing, feature bits |
| AGI | `XAGI` (0x58414749) | 296 B | AG sequence/length, inode counts, inode btree root and level, 64 unlinked buckets |

The AGI of allocation group *n* lives at `(n * agblocks + 2) * blocksize`
(block 0 = superblock copy, 1 = AGF, 2 = AGI, 3 = AGFL) — `agi --ag n` computes
that from the on-disk superblock, so you don't have to.

Not implemented yet: AGF/AGFL headers, inode decoding, directory and extent
walking, btree traversal.

## Layout

```
lib/   xfs-lib: big-endian scalars, superblock and AGI decoding (bincode)
app/   xfs-doctor: clap CLI (read sb / agi, --offset, --force)
```

`XfsStruct::decode` reads `decoded_size()` bytes — `repr(C)` padding can make this
a few bytes larger than the on-disk size (`XFS_SB_SIZE`, `XFS_AGI_SIZE`), which is
harmless. Header structs implement `XfsHeader`, which carries the magic and the
validation.

## Licence

Unlicense OR MIT (see `UNLICENSE`, `MIT-LICENSE`).
