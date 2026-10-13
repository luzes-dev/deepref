use deepref_server::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Dotenv files are a local-development convenience: they load only when the
    // process environment already says APP_ENV=local.
    deepref_config::load_local_dotenv_files();
    let command = Command::parse(std::env::args().skip(1))?;
    deepref_server::run(command).await
}
