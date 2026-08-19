use deepref_server::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command = Command::parse(std::env::args().skip(1))?;
    deepref_server::run(command).await
}
