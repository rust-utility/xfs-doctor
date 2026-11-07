use std::{fs::File, io::BufReader};

use clap::Parser;
use clap_num::maybe_hex;

use xfs_lib::xfs::{ag::agi::XfsAgi, sb::XfsSuperBlock, XfsStruct as _};

#[derive(Debug, Parser)]
#[command(version, author)]
struct Cli {
    #[arg(long, short)]
    device: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Parser, Debug)]
struct ReadParams {
    #[arg(long, short, default_value_t, value_parser=maybe_hex::<u64>)]
    offset: u64,
    #[arg(long, short, default_value_t)]
    magic: bool,
}

#[derive(Parser, Debug)]
enum Command {
    Read {
        #[command(flatten)]
        params: ReadParams,
        #[command(subcommand)]
        read_command: ReadCommand,
    },
}

#[derive(Parser, Debug)]
enum ReadCommand {
    Sb,
    Agi,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut device = BufReader::new(File::open(cli.device)?);

    match cli.command {
        Command::Read {
            params: ReadParams { offset, magic: _ },
            read_command,
        } => {
            device.seek_relative(offset as i64)?;
            match read_command {
                ReadCommand::Sb => {
                    let (sb_bytes, sb) = XfsSuperBlock::decode(&mut device)?;
                    rhexdump::rhexdump!(sb_bytes);
                    dbg!(sb);
                }
                ReadCommand::Agi => {
                    let (agi_bytes, agi) = XfsAgi::decode(&mut device)?;
                    rhexdump::rhexdump!(agi_bytes);
                    dbg!(agi);
                }
            }
        }
    }
    Ok(())
}
