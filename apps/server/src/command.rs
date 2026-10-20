use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    Worker,
    All,
    Migrate,
    BackfillProjectWorkspaces,
    LegacyActor(LegacyActorCommand),
    PrintOpenApi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyActorCommand {
    Map {
        kind: String,
        legacy_id: String,
        user_id: Uuid,
        operator_user_id: Uuid,
        source: String,
    },
    Deny {
        kind: String,
        legacy_id: String,
        user_id: Uuid,
        operator_user_id: Uuid,
        source: String,
    },
    List,
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
            [command] if command == "backfill-project-workspaces" => {
                Ok(Self::BackfillProjectWorkspaces)
            }
            [command] if command == "--print-openapi" => Ok(Self::PrintOpenApi),
            [command, action] if command == "legacy-actor" && action == "list" => {
                Ok(Self::LegacyActor(LegacyActorCommand::List))
            }
            [
                command,
                action,
                kind,
                legacy_id,
                user_id,
                operator_user_id,
                source,
            ] if command == "legacy-actor" && (action == "map" || action == "deny") => {
                let user_id = parse_uuid(user_id, "user UUID")?;
                let operator_user_id = parse_uuid(operator_user_id, "operator UUID")?;
                let fields = || LegacyActorFields {
                    kind: kind.clone(),
                    legacy_id: legacy_id.clone(),
                    user_id,
                    operator_user_id,
                    source: source.clone(),
                };
                Ok(Self::LegacyActor(if action == "map" {
                    fields().map()
                } else {
                    fields().deny()
                }))
            }
            _ => anyhow::bail!(
                "usage: deepref-server [serve|worker|all|migrate|backfill-project-workspaces|legacy-actor map <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor deny <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor list|--print-openapi]"
            ),
        }
    }
}

struct LegacyActorFields {
    kind: String,
    legacy_id: String,
    user_id: Uuid,
    operator_user_id: Uuid,
    source: String,
}

impl LegacyActorFields {
    fn map(self) -> LegacyActorCommand {
        LegacyActorCommand::Map {
            kind: self.kind,
            legacy_id: self.legacy_id,
            user_id: self.user_id,
            operator_user_id: self.operator_user_id,
            source: self.source,
        }
    }

    fn deny(self) -> LegacyActorCommand {
        LegacyActorCommand::Deny {
            kind: self.kind,
            legacy_id: self.legacy_id,
            user_id: self.user_id,
            operator_user_id: self.operator_user_id,
            source: self.source,
        }
    }
}

fn parse_uuid(value: &str, name: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_and_operator_commands() {
        assert_eq!(Command::parse(["serve".into()]).unwrap(), Command::Serve);
        assert_eq!(Command::parse(["worker".into()]).unwrap(), Command::Worker);
        assert_eq!(Command::parse(["all".into()]).unwrap(), Command::All);
        assert_eq!(
            Command::parse(["migrate".into()]).unwrap(),
            Command::Migrate
        );
        assert_eq!(
            Command::parse(["backfill-project-workspaces".into()]).unwrap(),
            Command::BackfillProjectWorkspaces
        );
        assert_eq!(
            Command::parse(["--print-openapi".into()]).unwrap(),
            Command::PrintOpenApi
        );
        assert_eq!(
            Command::parse(["legacy-actor".into(), "list".into()]).unwrap(),
            Command::LegacyActor(LegacyActorCommand::List)
        );
        let user = "00000000-0000-0000-0000-000000000001";
        let operator = "00000000-0000-0000-0000-000000000002";
        assert_eq!(
            Command::parse([
                "legacy-actor".into(),
                "map".into(),
                "user".into(),
                "local-user".into(),
                user.into(),
                operator.into(),
                "incident review".into(),
            ])
            .unwrap(),
            Command::LegacyActor(LegacyActorCommand::Map {
                kind: "user".into(),
                legacy_id: "local-user".into(),
                user_id: Uuid::parse_str(user).unwrap(),
                operator_user_id: Uuid::parse_str(operator).unwrap(),
                source: "incident review".into(),
            })
        );
        assert!(Command::parse(["import-legacy".into()]).is_err());
    }

    #[test]
    fn rejects_ambiguous_or_invalid_operator_commands() {
        assert_eq!(Command::parse(Vec::<String>::new()).unwrap(), Command::All);
        assert!(Command::parse(["serve".into(), "migrate".into()]).is_err());
        assert!(Command::parse(["unknown".into()]).is_err());
        assert!(
            Command::parse([
                "legacy-actor".into(),
                "map".into(),
                "system".into(),
                "x".into(),
                "not-a-uuid".into(),
                "00000000-0000-0000-0000-000000000002".into(),
                "source".into(),
            ])
            .is_err()
        );
    }
}
