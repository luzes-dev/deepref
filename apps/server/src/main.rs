use deepref_server::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Dotenv files are a development convenience: they load only when the
    // process environment already says APP_ENV=development.
    deepref_config::load_development_dotenv_files();
    // Runtime-role dispatch only; Command::parse accepts a fixed set of
    // positional commands and no security decision depends on argv[0].
    // nosemgrep: rust.lang.security.args.args
    let command = Command::parse(std::env::args().skip(1))?;
    deepref_server::run(command).await
}
