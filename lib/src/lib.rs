/*!
# xfs-lib

On-disk XFS structure decoding for `xfs-doctor`: big-endian scalars, the v4
superblock and the AGI header, decoded straight from a device or image with
`bincode`, plus RAID0 stripe mapping for single-member recovery.

```rust,no_run
# fn main() -> Result<(), Box<dyn std::error::Error>> {
use xfs_lib::xfs::{sb::XfsSuperBlock, XfsHeader, XfsStruct};

let mut file = std::io::BufReader::new(std::fs::File::open("sda4.img")?);
let (_bytes, sb) = XfsSuperBlock::decode(&mut file)?;
sb.validate()?;                       // checks the 'XFSB' magic
println!("{} blocks of {} bytes", sb.agblocks(), sb.blocksize());
# Ok(())
# }
```

*/

pub mod carve;
pub mod claims;
pub mod raid;
pub mod salvage;
pub mod xfs;
