use deepref_http_api::AuditQueryParams;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    Worker,
    All,
    Migrate,
    BackfillProjectWorkspaces,
    LegacyActor(LegacyActorCommand),
    Audit(AuditCommand),
    Accounts(AccountsCommand),
    PrintOpenApi,
}

/// Administrative audit tooling (handoff 11 Part B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditCommand {
    /// Deletes audit rows past their retention cutoff.
    Purge { dry_run: bool },
    /// Prints matching audit rows as JSON lines.
    Query(AuditQueryParams),
}

/// Account lifecycle tooling (handoff 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountsCommand {
    /// Completes account deletions whose 30-day window has passed.
    CompleteDeletions,
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
            [command, action] if command == "accounts" && action == "complete-deletions" => {
                Ok(Self::Accounts(AccountsCommand::CompleteDeletions))
            }
            [command, action, rest @ ..] if command == "audit" && action == "purge" => {
                let mut dry_run = false;
                for flag in rest {
                    match flag.as_str() {
                        "--dry-run" => dry_run = true,
                        _ => anyhow::bail!(AUDIT_USAGE),
                    }
                }
                Ok(Self::Audit(AuditCommand::Purge { dry_run }))
            }
            [command, action, rest @ ..] if command == "audit" && action == "query" => {
                let mut params = AuditQueryParams::default();
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let value = flags
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))?;
                    match flag.as_str() {
                        "--workspace" => {
                            params.workspace_id = Some(parse_uuid(value, "--workspace")?);
                        }
                        "--principal" => {
                            params.principal_user_id = Some(parse_uuid(value, "--principal")?);
                        }
                        "--target" => {
                            params.target_user_id = Some(parse_uuid(value, "--target")?);
                        }
                        "--event-type" => params.event_type = Some(value.clone()),
                        "--since" => params.since = Some(value.clone()),
                        "--until" => params.until = Some(value.clone()),
                        "--limit" => {
                            params.limit = value
                                .parse()
                                .map_err(|error| anyhow::anyhow!("invalid --limit: {error}"))?;
                        }
                        _ => anyhow::bail!(AUDIT_USAGE),
                    }
                }
                Ok(Self::Audit(AuditCommand::Query(params)))
            }
            _ => anyhow::bail!(
                "usage: deepref-server [serve|worker|all|migrate|backfill-project-workspaces|legacy-actor map <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor deny <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor list|audit purge [--dry-run]|audit query [--workspace <uuid>] [--principal <uuid>] [--target <uuid>] [--event-type <type>] [--since <rfc3339>] [--until <rfc3339>] [--limit <n>]|accounts complete-deletions|--print-openapi]"
            ),
        }
    }
}

const AUDIT_USAGE: &str = "usage: deepref-server audit purge [--dry-run]|audit query [--workspace <uuid>] [--principal <uuid>] [--target <uuid>] [--event-type <type>] [--since <rfc3339>] [--until <rfc3339>] [--limit <n>]";

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
    fn parses_audit_and_account_commands() {
        assert_eq!(
            Command::parse(["audit".into(), "purge".into()]).unwrap(),
            Command::Audit(AuditCommand::Purge { dry_run: false })
        );
        assert_eq!(
            Command::parse(["audit".into(), "purge".into(), "--dry-run".into()]).unwrap(),
            Command::Audit(AuditCommand::Purge { dry_run: true })
        );
        assert_eq!(
            Command::parse(["accounts".into(), "complete-deletions".into()]).unwrap(),
            Command::Accounts(AccountsCommand::CompleteDeletions)
        );
        let params = AuditQueryParams {
            workspace_id: Some(Uuid::parse_str("00000000-0000-0000-0000-00000000000a").unwrap()),
            principal_user_id: Some(
                Uuid::parse_str("00000000-0000-0000-0000-00000000000b").unwrap(),
            ),
            target_user_id: None,
            event_type: Some("authorization.denied".into()),
            since: Some("2026-01-01T00:00:00Z".into()),
            until: None,
            limit: 50,
        };
        assert_eq!(
            Command::parse([
                "audit".into(),
                "query".into(),
                "--workspace".into(),
                "00000000-0000-0000-0000-00000000000a".into(),
                "--principal".into(),
                "00000000-0000-0000-0000-00000000000b".into(),
                "--event-type".into(),
                "authorization.denied".into(),
                "--since".into(),
                "2026-01-01T00:00:00Z".into(),
                "--limit".into(),
                "50".into(),
            ])
            .unwrap(),
            Command::Audit(AuditCommand::Query(params))
        );
        assert!(Command::parse(["audit".into(), "query".into(), "--limit".into()]).is_err());
        assert!(
            Command::parse(["audit".into(), "purge".into(), "--limit".into(), "5".into()]).is_err()
        );
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
