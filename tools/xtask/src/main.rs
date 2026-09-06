mod boundaries;
mod sqlx;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "boundaries" => boundaries::run(),
        [command, subcommand] if command == "sqlx" && subcommand == "prepare" => {
            sqlx::run(sqlx::SqlxSubcommand::Prepare)
        }
        [command, subcommand] if command == "sqlx" && subcommand == "check" => {
            sqlx::run(sqlx::SqlxSubcommand::Check)
        }
        _ => bail!(
            "unknown command; usage:\n  \
             cargo xtask boundaries\n  \
             cargo xtask sqlx prepare\n  \
             cargo xtask sqlx check"
        ),
    }
}
