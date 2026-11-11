# xfs-doctor

Read-only tools for **damaged XFS filesystems and disk images**: decode on-disk
headers at an arbitrary offset, work out a RAID0 half-image's geometry, salvage
inodes and directories without mounting, and carve files by content signature when
the filesystem metadata is beyond reach.

No mount, no `xfs_db` dependency, nothing written to the image.

## Why it exists

A NAS left one surviving member of a 2-disk RAID0 to deal with. The image begins
with an md superblock, the filesystem does not start at offset 0, `xfs_db` refuses
to look past the foreign magic — and the array's other half was gone. This tool
grew out of that: first to answer "what is actually at this offset", then to recover
what could be recovered (~100 GB, described below).

## Commands

| Command | Purpose |
|---|---|
| `read sb` | decode an XFS superblock at a byte offset |
| `read agi` | decode an AGI header, by offset or by allocation-group number |
| `map` | translate logical offsets to a member of a 2-disk RAID0 (half-image math) |
| `salvage` | walk inodes and directories of the AGs present on one member, write files |
| `carve` | recover files by signature: PNG, JPEG, PDF, ZIP, RIFF (AVI/WAV), MP4, gzip, tar |

## Build

```bash
cargo build --release      # target/release/xfs-doctor
cargo test                 # 61 unit tests, no fixture images needed
```

## Usage

### Inspect a header

```bash
# superblock at offset 0, or anywhere (hex accepted)
xfs-doctor read --device sda4.img sb
xfs-doctor read --device sda4.img --offset 0x100000 sb

# decode a foreign/garbage header anyway (like xfs_db -F)
xfs-doctor read --device sda4.img sb --force

# AGI by offset, or by AG number (offset derived from the on-disk superblock)
xfs-doctor read --device sda4.img --offset 0x2000 agi
xfs-doctor read --device sda4.img agi --ag 3
```

A wrong magic is an error, not a silent garbage dump:

```
$ xfs-doctor read -d sda4.img sb
Error: cannot decode superblock at offset 0x0: bad superblock magic: found 0xfc4e2ba9, expected 0x58465342
```

### Half-image math

Two disks, one survivor: every other chunk is missing. `map` answers where a
logical offset lives and which ranges are still readable.

```bash
xfs-doctor map --chunk 512K --data-offset 8K --member 1 \
  --at 0x80000 --ranges 200G
```

```
logical 0x80000 -> disk 0x2000
present 0x80000..0x100000 (524288 bytes)
present 0x180000..0x200000 (524288 bytes)
...
```

### Structured salvage

For the allocation groups that lie entirely on the surviving member:

```bash
xfs-doctor salvage --device sda4.img --sb sb-ag1.bin \
  --chunk 512K --data-offset 8K --member 1 --out salvage/ --scan 256M
```

It decodes inodes (v1/v2 cores, extent lists, inline data), walks directory blocks
(`dir2` and shortform), writes recoverable files as `files/<ino>_<name>`, and emits
`manifest.tsv`.

### Carving

When the mapping is unknown or the metadata is gone, carve by content:

```bash
xfs-doctor carve --device sda4.img --out carve/ --types jpeg,png --max 16M
xfs-doctor carve --device sda4.img --out carve-big/ --types mp4,riff --min 64K --max 64G
```

Every candidate is validated by the format's own structure, so a bare magic is
never enough:

| Format | End detection | Validation |
|---|---|---|
| PNG | chunk walk to `IEND` | chunk lengths |
| JPEG | `FFD9` | segment structure |
| PDF | `%%EOF` + EOL | — |
| ZIP | EOCD + comment length | central directory at the declared offset, first entry's local header at 0 (rejects members *inside* an archive) |
| RIFF | length field | form type must be AVI/WAVE/WEBP/CDXA/RMID |
| MP4 | box walk | a `moov` box must exist; never truncates at a size-0 box |
| gzip | DEFLATE walk (symbols only, no decompression) | decoded length must equal the trailer ISIZE |
| tar | 512-byte header walk to the two zero blocks | per-header checksum |

### Incremental passes: the claims ledger

Each pass writes `claims.tsv` (claimed byte ranges). A later pass consults it, so
it neither re-carves what an earlier pass found nor loses it:

- hit already covered → skipped
- hit *longer* than its claim (a bigger `--max` finding the rest) → carved again,
  claim extended (`--upgrade`)
- `--skip-inside-claims true` rejects candidates inside a claim before validating
  them, which is much faster for formats whose candidates are dense

Repeated passes with different `--types`/`--max` are therefore cheap and never
duplicate work.

## What it actually recovered

From the single RAID0 member described above:

| Pass | Result |
|---|---|
| photos / docs / jars | 461,868 files, 36.7 GB |
| large media (`--max 4G`) | 97 AVI/WAV/MP4, 60.3 GB |
| PDFs (`--max 256M`) | 50 files, 3.1 GB |
| gzip | 2,580 members, 102 MB — all verified by `gzip -t` |
| zip | 1,650 archives, 337 MB — 97 % verified by `unzip -t` |
| tar | 35 archives, 80 MB — 100 % verified by `tar -tf` |

Structured salvage stayed blocked: the image turned out to mix two RAID layouts, so
no single block mapping explains both the AG superblocks and the data. Carving does
not care.

## Limits

- Carving recovers content, not names or directory structure.
- A file whose bytes straddle a missing chunk is either rejected (formats with a
  completeness check) or comes out truncated (formats without one).
- `salvage` needs the correct geometry; it is only as good as the mapping.
- XFS v5 (CRC) structures are not decoded — the superblock/AGI/inode layouts here
  are v4.

## Layout

```
lib/src/xfs/      superblock, AGI, inode decoding; big-endian scalars (bincode)
lib/src/carve.rs  signature carving + per-format validation (gzip DEFLATE walker inside)
lib/src/claims.rs byte-range ledger
lib/src/raid.rs   RAID0 single-member mapping
lib/src/salvage.rs geometry, AG presence, dir parsing
app/src/main.rs   clap CLI: read, map, salvage, carve
```

## Licence

Unlicense OR MIT — see `UNLICENSE` and `MIT-LICENSE`.
