mod boundaries;
mod doctor;
mod generate;
mod new_crate;
mod sqlx;
mod workspace;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "boundaries" => boundaries::run(),
        [command] if command == "doctor" => doctor::run(),
        [command] if command == "generate" => generate::run(generate::GenerateMode::Write),
        [command, flag] if command == "generate" && flag == "--check" => {
            generate::run(generate::GenerateMode::Check)
        }
        [command, subcommand] if command == "sqlx" && subcommand == "prepare" => {
            sqlx::run(sqlx::SqlxSubcommand::Prepare)
        }
        [command, subcommand] if command == "sqlx" && subcommand == "check" => {
            sqlx::run(sqlx::SqlxSubcommand::Check)
        }
        [command, flag, layer, name] if command == "new-crate" && flag == "--layer" => {
            new_crate::run(layer, name)
        }
        _ => bail!(
            "unknown command; usage:\n  \
             cargo xtask boundaries\n  \
             cargo xtask doctor\n  \
             cargo xtask generate\n  \
             cargo xtask generate --check\n  \
             cargo xtask sqlx prepare\n  \
             cargo xtask sqlx check\n  \
             cargo xtask new-crate --layer <layer> <name>"
        ),
    }
}
