mod boundaries;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "boundaries" => boundaries::run(),
        [] => bail!("usage: cargo xtask boundaries"),
        _ => bail!("unknown command; usage: cargo xtask boundaries"),
    }
}
