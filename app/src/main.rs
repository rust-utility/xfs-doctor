//! `xfs-doctor` — inspect XFS headers on a device or disk image.
//!
//! Built to probe a damaged filesystem image without mounting anything: it reads
//! a fixed-size header at a byte offset, hexdumps it and prints the decoded
//! fields.
//!
//! ```
//! xfs-doctor read --device sda4.img sb
//! xfs-doctor read --device sda4.img --offset 0x100000 sb
//! xfs-doctor read --device sda4.img agi --ag 3
//! ```

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};

use anyhow::Context;
use clap::{Parser, Subcommand};
use clap_num::maybe_hex;

use std::collections::HashMap;
use std::fs::create_dir_all;

use xfs_lib::carve;
use xfs_lib::raid::Raid0Half;
use xfs_lib::salvage::{dir_entries, sf_dir_entries, Geometry, Salvage};
use xfs_lib::xfs::inode::{ForkFormat, Inode, XFS_DINODE_MAGIC};
use xfs_lib::xfs::{ag::agi::XfsAgi, sb::XfsSuperBlock, XfsHeader, XfsStruct};

#[derive(Debug, Parser)]
#[command(version, author, about = "Inspect XFS headers on a device or image")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Translate logical offsets of a RAID0 array to this member (half-image math)
    Map {
        /// Data offset of each member in bytes (md "Data Offset")
        #[arg(long, default_value = "8K", value_parser = parse_size)]
        data_offset: u64,
        /// Stripe chunk size in bytes, e.g. 64K, 512K, 1M
        #[arg(long, value_parser = parse_size)]
        chunk: u64,
        /// Array width
        #[arg(long, default_value_t = 2)]
        disks: u32,
        /// Index of the member we hold (0-based)
        #[arg(long, default_value_t = 0)]
        member: u32,
        /// Logical offsets to translate (decimal or 0x hex)
        #[arg(long = "at", value_parser = maybe_hex::<u64>)]
        at: Vec<u64>,
        /// Print the present logical ranges for an array of this many bytes
        #[arg(long, value_parser = maybe_hex::<u64>)]
        ranges: Option<u64>,
    },
    /// Recover files from a single surviving member of a 2-disk RAID0
    Salvage {
        /// Device or image file to read
        #[arg(long, short)]
        device: String,
        /// File holding a surviving superblock copy (512 bytes)
        #[arg(long)]
        sb: Option<String>,
        /// Offset of a surviving superblock copy inside the image
        #[arg(long, value_parser = maybe_hex::<u64>)]
        sb_offset: Option<u64>,
        /// Stripe chunk size
        #[arg(long, default_value = "512K", value_parser = parse_size)]
        chunk: u64,
        /// Data offset of each member (md "Data Offset")
        #[arg(long, default_value = "8K", value_parser = parse_size)]
        data_offset: u64,
        /// Index of the member we hold
        #[arg(long, default_value_t = 1)]
        member: u32,
        /// Array width
        #[arg(long, default_value_t = 2)]
        disks: u32,
        /// Output directory
        #[arg(long, short)]
        out: String,
        /// Scan only these allocation groups (default: all present on this member)
        #[arg(long = "ag", value_parser = maybe_hex::<u32>)]
        ags: Vec<u32>,
        /// Bytes to scan per allocation group for inode chunks
        #[arg(long, default_value = "32M", value_parser = parse_size)]
        scan: u64,
        /// Skip files larger than this
        #[arg(long, default_value = "64M", value_parser = parse_size)]
        max_size: u64,
    },
    /// Carve files by signature (no filesystem mapping needed)
    Carve {
        /// Device or image file to read
        #[arg(long, short)]
        device: String,
        /// Output directory
        #[arg(long, short)]
        out: String,
        /// Start scanning at this GiB offset
        #[arg(long, default_value_t = 0)]
        from_gib: u64,
        /// Stop scanning at this GiB offset (0 = end of image)
        #[arg(long, default_value_t = 0)]
        to_gib: u64,
        /// Ignore hits smaller than this
        #[arg(long, default_value = "4K", value_parser = parse_size)]
        min: u64,
        /// Ignore hits larger than this
        #[arg(long, default_value = "64M", value_parser = parse_size)]
        max: u64,
        /// Read block size
        #[arg(long, default_value = "16M", value_parser = parse_size)]
        block: u64,
        /// Only carve these types (comma separated): png,jpeg,pdf,zip,riff,mp4
        #[arg(long, value_delimiter = ',')]
        types: Vec<String>,
    },
    /// Decode a header at a byte offset
    Read {
        /// Device or image file to read
        #[arg(long, short)]
        device: String,
        /// Byte offset to decode at (hex allowed)
        #[arg(long, short, default_value_t = 0, value_parser = maybe_hex::<u64>)]
        offset: u64,
        /// Decode even when the magic does not match (like xfs_db -F)
        #[arg(short = 'F', long)]
        force: bool,
        #[command(subcommand)]
        what: What,
    },
}

/// `8K`, `512K`, `1M`, `4096` — decimal with an optional K/M/G suffix.
fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (digits, mult) = match text.chars().last() {
        Some('K') | Some('k') => (&text[..text.len() - 1], 1024u64),
        Some('M') | Some('m') => (&text[..text.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        _ => (text, 1),
    };
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("not a size: {text:?} (use 512K, 1M, 8192)"))?;
    value
        .checked_mul(mult)
        .ok_or_else(|| format!("size too large: {text:?}"))
}

#[derive(Debug, Subcommand)]
enum What {
    /// XFS superblock ('XFSB')
    Sb,
    /// AGI header ('XAGI'); --ag derives the offset from the superblock
    Agi {
        /// Allocation group number, 0..agcount
        #[arg(long, value_parser = maybe_hex::<u32>)]
        ag: Option<u32>,
    },
}

fn read_at<H: XfsStruct, R: Read + Seek>(
    device: &mut R,
    offset: u64,
    what: &str,
    dump: bool,
) -> anyhow::Result<H> {
    device
        .seek(SeekFrom::Start(offset))
        .with_context(|| format!("cannot seek to {offset:#x}"))?;
    let (bytes, header) = H::decode(&mut std::io::BufReader::new(device.by_ref()))
        .with_context(|| format!("cannot decode {what} at offset {offset:#x}"))?;
    if dump {
        rhexdump::rhexdump!(bytes);
    }
    Ok(header)
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Map {
            data_offset,
            chunk,
            disks,
            member,
            at,
            ranges,
        } => run_map(data_offset, chunk, disks, member, &at, ranges),
        Command::Read {
            device,
            offset,
            force,
            what,
        } => run_read(device, offset, force, what),
        Command::Carve {
            device,
            out,
            from_gib,
            to_gib,
            min,
            max,
            block,
            types,
        } => run_carve(&device, &out, from_gib, to_gib, min, max, block as usize, &types),
        Command::Salvage {
            device,
            sb,
            sb_offset,
            chunk,
            data_offset,
            member,
            disks,
            out,
            ags,
            scan,
            max_size,
        } => run_salvage(
            &device,
            sb.as_deref(),
            sb_offset,
            chunk,
            data_offset,
            member,
            disks,
            &out,
            &ags,
            scan,
            max_size,
        ),
    }
}

fn run_map(
    data_offset: u64,
    chunk: u64,
    disks: u32,
    member: u32,
    at: &[u64],
    ranges: Option<u64>,
) -> anyhow::Result<()> {
    let map = Raid0Half::new(data_offset, chunk, disks, member)?;
    println!(
        "# RAID0: {} disks, {} byte chunks, data offset {}, holding member {}",
        map.disks(),
        map.chunk(),
        data_offset,
        map.member()
    );
    for logical in at {
        match map.disk_offset(*logical) {
            Some(disk) => println!("logical {logical:#x} -> disk {disk:#x}"),
            None => println!("logical {logical:#x} -> MISSING (on another member)"),
        }
    }
    if let Some(len) = ranges {
        let mut covered = 0u64;
        for (start, length) in map.present_ranges(len) {
            println!("present {start:#x}..{:#x} ({length} bytes)", start + length);
            covered += length;
        }
        println!(
            "# covered {covered} of {len} bytes ({:.1}%)",
            covered as f64 * 100.0 / len as f64
        );
    }
    Ok(())
}

fn run_read(device: String, offset: u64, force: bool, what: What) -> anyhow::Result<()> {
    let mut device =
        BufReader::new(File::open(&device).with_context(|| format!("cannot open {device}"))?);

    match what {
        What::Sb => {
            let sb: XfsSuperBlock = read_at(&mut device, offset, "superblock", true)?;
            if force {
                if !sb.is_valid() {
                    eprintln!("# warning: magic does not match 'XFSB' (forced)");
                }
            } else {
                sb.validate()?;
            }
            println!("{sb:#?}");
        }
        What::Agi { ag } => {
            let agi_offset = match ag {
                None => offset,
                Some(ag) => {
                    let sb: XfsSuperBlock = read_at(&mut device, offset, "superblock", false)?;
                    sb.validate().context("scanning AGIs needs a valid superblock")?;
                    let agi_offset = sb.agi_offset(ag)?;
                    println!(
                        "# AGI offset for AG {ag}: {agi_offset:#x} (block {:#x})",
                        agi_offset / sb.blocksize() as u64
                    );
                    agi_offset
                }
            };
            let agi: XfsAgi = read_at(&mut device, agi_offset, "AGI", true)?;
            if force {
                if !agi.is_valid() {
                    eprintln!("# warning: magic does not match 'XAGI' (forced)");
                }
            } else {
                agi.validate()?;
            }
            println!("{agi:#?}");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_salvage(
    device: &str,
    sb_file: Option<&str>,
    sb_offset: Option<u64>,
    chunk: u64,
    data_offset: u64,
    member: u32,
    disks: u32,
    out: &str,
    ags: &[u32],
    scan: u64,
    max_size: u64,
) -> anyhow::Result<()> {
    let sb_bytes = match (sb_file, sb_offset) {
        (Some(path), _) => std::fs::read(path).context("cannot read superblock file")?,
        (None, Some(offset)) => {
            let mut f = File::open(device)?;
            f.seek(SeekFrom::Start(offset))?;
            let mut buf = [0u8; 512];
            f.read_exact(&mut buf)?;
            buf.to_vec()
        }
        (None, None) => anyhow::bail!("need --sb <file> or --sb-offset <offset>"),
    };

    let geometry = Geometry::from_sb(&sb_bytes)?;
    let stripe = Raid0Half::new(data_offset, chunk, disks, member)?;
    let salvage = Salvage::new(geometry, stripe);
    let bs = geometry.blocksize as u64;
    let max_inode = (geometry.agcount as u64)
        .saturating_mul(geometry.agblocks as u64)
        << geometry.inopblog;

    println!(
        "# fs: {} byte blocks, {} AGs of {} blocks, {} byte inodes",
        geometry.blocksize, geometry.agcount, geometry.agblocks, geometry.inodesize
    );
    println!(
        "# stripes: {} byte chunks, member {} of {}, data offset {}",
        chunk, member, disks, data_offset
    );

    let present = if ags.is_empty() { salvage.present_ags() } else { ags.to_vec() };
    println!("# scanning AGs: {present:?}");

    create_dir_all(out)?;
    let files_dir = std::path::Path::new(out).join("files");
    create_dir_all(&files_dir)?;
    let mut manifest = String::from("# ino\tkind\tsize\tname\tparent\n");
    let mut names: HashMap<u64, String> = HashMap::new();

    let mut image = File::open(device)?;
    let mut inodes_seen = 0usize;
    let mut files_written = 0usize;
    let mut bytes_written = 0u64;
    let mut files_skipped = 0usize;
    let mut dirs_parsed = 0usize;
    let mut dir_entries_total = 0usize;
    let inode_size = geometry.inodesize as usize;

    let chunk = chunk.max(1);
    for ag in 0..geometry.agcount {
        if !ags.is_empty() && !ags.contains(&ag) {
            continue;
        }
        let ag_start = ag as u64 * geometry.ag_size();
        let ag_end = ag_start + geometry.ag_size();
        let mut logical = ag_start;
        let mut scanned = 0u64;

        while logical < ag_end && scanned < scan {
            let len = chunk.min(scan - scanned).min(ag_end - logical);
            // The image is interleaved: a chunk is readable only when it lives on
            // this member, and it maps to its own logical position.
            if let Some(disk) = salvage.disk_offset(logical) {
                let mut buf = vec![0u8; len as usize];
                image.seek(SeekFrom::Start(disk))?;
                let got = image.read(&mut buf)?;
                buf.truncate(got);
                scan_chunk(
                    &buf,
                    logical - ag_start,
                    ag,
                    &geometry,
                    &salvage,
                    &mut image,
                    &files_dir,
                    &mut manifest,
                    &mut names,
                    max_inode,
                    max_size,
                    &mut inodes_seen,
                    &mut files_written,
                    &mut bytes_written,
                    &mut files_skipped,
                    &mut dirs_parsed,
                    &mut dir_entries_total,
                )?;
            }
            logical += chunk;
            scanned += chunk;
        }
        println!("# AG{ag}: done ({inodes_seen} inodes so far, {files_written} files)");
    }

    // second pass: files were written before their names were known
    let mut renamed = 0usize;
    for (ino, name) in &names {
        let from = files_dir.join(format!("{ino}"));
        if from.exists() {
            let to = files_dir.join(format!("{ino}_{}", sanitize(name)));
            if std::fs::rename(&from, &to).is_ok() {
                renamed += 1;
            }
        }
    }
    if renamed > 0 {
        println!("# attached harvested names to {renamed} files");
    }

    std::fs::write(std::path::Path::new(out).join("manifest.tsv"), manifest)?;
    println!(
        "# inodes={inodes_seen} dirs={dirs_parsed} names={dir_entries_total} files={files_written} skipped={files_skipped} bytes={bytes_written}"
    );
    println!("# output: {} (files/, manifest.tsv)", out);
    Ok(())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c == '/' || (c as u32) < 32 { '_' } else { c })
        .collect::<String>()
        .chars()
        .take(80)
        .collect()
}

fn clean(name: Option<&str>) -> String {
    name.unwrap_or("").replace(['\t', '\n', '\r'], " ")
}

#[allow(clippy::too_many_arguments)]
fn scan_chunk(
    buf: &[u8],
    ag_offset: u64,
    ag: u32,
    geometry: &Geometry,
    salvage: &Salvage,
    image: &mut File,
    files_dir: &std::path::Path,
    manifest: &mut String,
    names: &mut HashMap<u64, String>,
    max_inode: u64,
    max_size: u64,
    inodes_seen: &mut usize,
    files_written: &mut usize,
    bytes_written: &mut u64,
    files_skipped: &mut usize,
    dirs_parsed: &mut usize,
    dir_entries_total: &mut usize,
) -> anyhow::Result<()> {
    let bs = geometry.blocksize as u64;
    let inode_size = geometry.inodesize as usize;
    let mut pos = 0usize;

    while pos + inode_size <= buf.len() {
        if buf[pos..pos + 2] != XFS_DINODE_MAGIC.to_be_bytes() {
            pos += inode_size;
            continue;
        }
        let mut run = 0usize;
        while pos + run * inode_size + 2 <= buf.len()
            && buf[pos + run * inode_size..pos + run * inode_size + 2] == XFS_DINODE_MAGIC.to_be_bytes()
        {
            run += 1;
        }
        if run < 4 {
            pos += inode_size;
            continue;
        }

        for i in 0..run {
            let slot = pos + i * inode_size;
            let slot_in_ag = ag_offset + slot as u64;
            let agino = (slot_in_ag / bs) * geometry.inopblock as u64 + (slot_in_ag % bs) / inode_size as u64;
            let ino = geometry.inode_number(ag, agino);
            let Some(inode) = Inode::decode(&buf[slot..slot + inode_size], ino)? else { continue };
            if inode.mode == 0 || !inode.is_plausible() {
                continue;
            }
            *inodes_seen += 1;

            match inode.format {
                ForkFormat::Local if inode.is_regular() => {
                    let data = inode.local_data();
                    if (data.len() as u64) <= max_size {
                        let name = names.get(&ino).cloned();
                        let file_name = match &name {
                            Some(n) => format!("{ino}_{n}"),
                            None => format!("{ino}"),
                        };
                        if std::fs::write(files_dir.join(&file_name), data).is_ok() {
                            *files_written += 1;
                            *bytes_written += data.len() as u64;
                            manifest.push_str(&format!("{ino}\tfile\t{}\t{}\t\n", data.len(), clean(name.as_deref())));
                        }
                    }
                }
                ForkFormat::Local if inode.is_dir() => {
                    *dirs_parsed += 1;
                    for entry in sf_dir_entries(inode.local_data(), max_inode) {
                        if entry.name == "." || entry.name == ".." {
                            continue;
                        }
                        *dir_entries_total += 1;
                        names.entry(entry.ino).or_insert_with(|| entry.name.clone());
                        manifest.push_str(&format!("\tname\t\t{}\t{ino}\n", clean(Some(&entry.name))));
                    }
                }
                ForkFormat::Extents if inode.is_regular() => {
                    if inode.size > max_size || !inode.extents_valid(geometry.dblocks) {
                        *files_skipped += 1;
                        continue;
                    }
                    let extents = inode.extents()?;
                    match salvage.readable_runs(&extents) {
                        None => *files_skipped += 1,
                        Some(runs) => {
                            let name = names.get(&ino).cloned();
                            let file_name = match &name {
                                Some(n) => format!("{ino}_{n}"),
                                None => format!("{ino}"),
                            };
                            let target = files_dir.join(&file_name);
                            let mut out_file = File::create(&target)?;
                            let mut chunk_buf = vec![0u8; bs as usize];
                            for (logical, disk, len) in &runs {
                                image.seek(SeekFrom::Start(*disk))?;
                                let n = image.read(&mut chunk_buf)?;
                                out_file.seek(SeekFrom::Start(*logical))?;
                                out_file.write_all(&chunk_buf[..n.min(*len as usize)])?;
                            }
                            out_file.set_len(inode.size)?;
                            *files_written += 1;
                            *bytes_written += inode.size;
                            manifest.push_str(&format!("{ino}\tfile\t{}\t{}\t\n", inode.size, clean(name.as_deref())));
                        }
                    }
                }
                ForkFormat::Extents if inode.is_dir() => {
                    if !inode.extents_valid(geometry.dblocks) {
                        continue;
                    }
                    *dirs_parsed += 1;
                    let extents = inode.extents()?;
                    let Some(runs) = salvage.readable_runs(&extents) else { continue };
                    let mut block = vec![0u8; bs as usize];
                    for (_, disk, len) in runs {
                        image.seek(SeekFrom::Start(disk))?;
                        let n = image.read(&mut block)?;
                        if n < 32 {
                            continue;
                        }
                        for entry in dir_entries(&block[..n.min(len as usize)], max_inode) {
                            if entry.name == "." || entry.name == ".." {
                                continue;
                            }
                            *dir_entries_total += 1;
                            names.entry(entry.ino).or_insert_with(|| entry.name.clone());
                            manifest.push_str(&format!("\tname\t\t{}\t{ino}\n", clean(Some(&entry.name))));
                        }
                    }
                }
                _ => {}
            }
        }
        pos += run * inode_size;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_carve(
    device: &str,
    out: &str,
    from_gib: u64,
    to_gib: u64,
    min: u64,
    max: u64,
    block: usize,
    types: &[String],
) -> anyhow::Result<()> {
    use std::io::Write as _;

    let mut image = File::open(device).with_context(|| format!("cannot open {device}"))?;
    let mut scanner = File::open(device).with_context(|| format!("cannot open {device}"))?;
    let size = image.metadata()?.len();
    let from = from_gib * 1024 * 1024 * 1024;
    let to = if to_gib == 0 { size } else { (to_gib * 1024 * 1024 * 1024).min(size) };

    create_dir_all(std::path::Path::new(out).join("carved"))?;
    let carved_dir = std::path::Path::new(out).join("carved");
    let mut manifest = String::from("# offset\ttype\tbytes\tlength\n");
    let mut buf = vec![0u8; 1024 * 1024];
    let mut count = 0usize;
    let mut total = 0u64;
    let started = std::time::Instant::now();

    let type_names: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
    let filter = if type_names.is_empty() { None } else { Some(type_names.as_slice()) };
    carve::scan_types(&mut scanner, from, to, block, min, max, filter, |hit| {
        let name = format!("{}_{}.{}", hit.start, hit.sig.name, hit.sig.ext);
        let path = carved_dir.join(&name);
        let mut target = File::create(&path)?;
        let mut left = hit.len;
        let mut pos = hit.start;
        while left > 0 {
            let want = (left as usize).min(buf.len());
            image.seek(SeekFrom::Start(pos))?;
            let n = image.read(&mut buf[..want])?;
            if n == 0 {
                break;
            }
            target.write_all(&buf[..n])?;
            pos += n as u64;
            left -= n as u64;
        }
        count += 1;
        total += hit.len;
        manifest.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            hit.start, hit.sig.name, path.display(), hit.len
        ));
        if count % 200 == 0 {
            println!(
                "# {count} carved, {:.1} MB, elapsed {}s",
                total as f64 / 1048576.0,
                started.elapsed().as_secs()
            );
        }
        Ok(())
    })?;

    std::fs::write(std::path::Path::new(out).join("carved.tsv"), manifest)?;
    println!("# carved={count} bytes={total} out={out}");
    Ok(())
}
