#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Serve,
    Worker,
    All,
    Migrate,
    ImportLegacy,
}

impl Command {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        match arguments.as_slice() {
            [] => Ok(Self::All),
            [command] if command == "serve" => Ok(Self::Serve),
            [command] if command == "worker" => Ok(Self::Worker),
            [command] if command == "all" => Ok(Self::All),
            [command] if command == "migrate" => Ok(Self::Migrate),
            [command] if command == "import-legacy" => Ok(Self::ImportLegacy),
            _ => anyhow::bail!("usage: deepref-server [serve|worker|all|migrate|import-legacy]"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_runtime_role() {
        assert_eq!(Command::parse(["serve".into()]).unwrap(), Command::Serve);
        assert_eq!(Command::parse(["worker".into()]).unwrap(), Command::Worker);
        assert_eq!(Command::parse(["all".into()]).unwrap(), Command::All);
        assert_eq!(
            Command::parse(["migrate".into()]).unwrap(),
            Command::Migrate
        );
        assert_eq!(
            Command::parse(["import-legacy".into()]).unwrap(),
            Command::ImportLegacy
        );
    }

    #[test]
    fn defaults_to_all_and_rejects_ambiguous_commands() {
        assert_eq!(Command::parse(Vec::<String>::new()).unwrap(), Command::All);
        assert!(Command::parse(["serve".into(), "migrate".into()]).is_err());
        assert!(Command::parse(["unknown".into()]).is_err());
    }
}
