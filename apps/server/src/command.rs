use deepref_domain::WorkloadClass;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Serve,
    /// Worker for the selected workload pools (`--pool`), or every pool when
    /// the process was started without a selection.
    Worker {
        pools: Vec<WorkloadClass>,
    },
    All {
        pools: Vec<WorkloadClass>,
    },
    Migrate,
    BackfillProjectWorkspaces,
    LegacyActor(LegacyActorCommand),
    AiUsage(AiUsageCommand),
    PrintOpenApi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiUsageCommand {
    Reconcile {
        workspace_id: Option<Uuid>,
    },
    WaiveOverage {
        workspace_id: Uuid,
        operator_user_id: Uuid,
    },
    SetCap {
        scope_kind: String,
        scope_id: Option<Uuid>,
        limit_micros: i64,
        operator_user_id: Uuid,
    },
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
            [] => Ok(Self::All {
                pools: Self::all_pools(),
            }),
            [command] if command == "serve" => Ok(Self::Serve),
            [command] if command == "worker" => Ok(Self::Worker {
                pools: Self::all_pools(),
            }),
            [command] if command == "all" => Ok(Self::All {
                pools: Self::all_pools(),
            }),
            [command] if command == "migrate" => Ok(Self::Migrate),
            [command] if command == "backfill-project-workspaces" => {
                Ok(Self::BackfillProjectWorkspaces)
            }
            [command] if command == "--print-openapi" => Ok(Self::PrintOpenApi),
            // `deepref-server worker --pool ai,documents` runs one worker
            // process per selection; `all` selects the pools of its worker
            // half the same way.
            [command, flag, pools]
                if flag == "--pool" && (command == "worker" || command == "all") =>
            {
                let pools = Self::parse_pools(pools)?;
                if command == "worker" {
                    Ok(Self::Worker { pools })
                } else {
                    Ok(Self::All { pools })
                }
            }
            [command, action] if command == "legacy-actor" && action == "list" => {
                Ok(Self::LegacyActor(LegacyActorCommand::List))
            }
            [command, action] if command == "ai-usage" && action == "reconcile" => {
                Ok(Self::AiUsage(AiUsageCommand::Reconcile {
                    workspace_id: None,
                }))
            }
            [command, action, workspace_id] if command == "ai-usage" && action == "reconcile" => {
                Ok(Self::AiUsage(AiUsageCommand::Reconcile {
                    workspace_id: Some(parse_uuid(workspace_id, "workspace UUID")?),
                }))
            }
            [command, action, workspace_id, operator_user_id]
                if command == "ai-usage" && action == "waive-overage" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::WaiveOverage {
                    workspace_id: parse_uuid(workspace_id, "workspace UUID")?,
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                }))
            }
            [
                command,
                action,
                scope_kind,
                scope_id,
                limit_micros,
                operator_user_id,
            ] if command == "ai-usage" && action == "set-cap" => {
                let scope_id = match (scope_kind.as_str(), scope_id.as_str()) {
                    ("platform", "-") => None,
                    ("workspace" | "project", value) => Some(parse_uuid(value, "scope UUID")?),
                    ("platform", _) => anyhow::bail!("platform scope id must be '-'"),
                    _ => anyhow::bail!("scope must be platform, workspace or project"),
                };
                let limit_micros = limit_micros
                    .parse::<i64>()
                    .map_err(|error| anyhow::anyhow!("invalid limit micros: {error}"))?;
                anyhow::ensure!(limit_micros >= 0, "limit micros must be nonnegative");
                Ok(Self::AiUsage(AiUsageCommand::SetCap {
                    scope_kind: scope_kind.clone(),
                    scope_id,
                    limit_micros,
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                }))
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
                "usage: deepref-server [serve|worker [--pool <class>[,<class>]]|all [--pool <class>[,<class>]]|migrate|backfill-project-workspaces|legacy-actor map <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor deny <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor list|ai-usage reconcile [workspace-uuid]|ai-usage waive-overage <workspace-uuid> <operator-user-uuid>|ai-usage set-cap <platform|workspace|project> <-|scope-uuid> <limit-micros> <operator-user-uuid>|--print-openapi]"
            ),
        }
    }

    /// Every workload class: the behavior of `deepref-server worker` without
    /// `--pool` is unchanged.
    fn all_pools() -> Vec<WorkloadClass> {
        WorkloadClass::ALL.to_vec()
    }

    fn parse_pools(spec: &str) -> anyhow::Result<Vec<WorkloadClass>> {
        let mut pools: Vec<WorkloadClass> = Vec::new();
        for name in spec.split(',') {
            let class: WorkloadClass = name.parse()?;
            if !pools.contains(&class) {
                pools.push(class);
            }
        }
        Ok(pools)
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
        assert_eq!(
            Command::parse(["worker".into()]).unwrap(),
            Command::Worker {
                pools: WorkloadClass::ALL.to_vec()
            }
        );
        assert_eq!(
            Command::parse(["all".into()]).unwrap(),
            Command::All {
                pools: WorkloadClass::ALL.to_vec()
            }
        );
        assert_eq!(
            Command::parse(["migrate".into()]).unwrap(),
            Command::Migrate
        );
        assert_eq!(
            Command::parse(["ai-usage".into(), "reconcile".into()]).unwrap(),
            Command::AiUsage(AiUsageCommand::Reconcile { workspace_id: None })
        );
        let workspace_id = Uuid::from_u128(55);
        let operator_user_id = Uuid::from_u128(56);
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "waive-overage".into(),
                workspace_id.to_string(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::WaiveOverage {
                workspace_id,
                operator_user_id,
            })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "set-cap".into(),
                "platform".into(),
                "-".into(),
                "1000000".into(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::SetCap {
                scope_kind: "platform".to_owned(),
                scope_id: None,
                limit_micros: 1_000_000,
                operator_user_id,
            })
        );
        assert!(
            Command::parse([
                "ai-usage".into(),
                "set-cap".into(),
                "platform".into(),
                workspace_id.to_string(),
                "1".into(),
                operator_user_id.to_string(),
            ])
            .is_err()
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
        assert_eq!(
            Command::parse(Vec::<String>::new()).unwrap(),
            Command::All {
                pools: WorkloadClass::ALL.to_vec()
            }
        );
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

    /// Pool selection is process-level: `worker --pool <class>[,<class>]`.
    #[test]
    fn parses_pool_selection() {
        assert_eq!(
            Command::parse(["worker".into(), "--pool".into(), "ai".into()]).unwrap(),
            Command::Worker {
                pools: vec![WorkloadClass::Ai]
            }
        );
        assert_eq!(
            Command::parse(["worker".into(), "--pool".into(), "documents,ai".into()]).unwrap(),
            Command::Worker {
                pools: vec![WorkloadClass::Documents, WorkloadClass::Ai]
            }
        );
        // Repeats collapse without reordering; `all` selects its worker half.
        assert_eq!(
            Command::parse(["worker".into(), "--pool".into(), "ai,documents,ai".into()]).unwrap(),
            Command::Worker {
                pools: vec![WorkloadClass::Ai, WorkloadClass::Documents]
            }
        );
        assert_eq!(
            Command::parse(["all".into(), "--pool".into(), "integration".into()]).unwrap(),
            Command::All {
                pools: vec![WorkloadClass::Integration]
            }
        );
    }

    #[test]
    fn rejects_unknown_pools_and_flags() {
        assert!(Command::parse(["worker".into(), "--pool".into(), "review".into()]).is_err());
        assert!(
            Command::parse(["worker".into(), "--pool".into(), "ai,,documents".into()]).is_err()
        );
        assert!(Command::parse(["worker".into(), "--pools".into(), "ai".into()]).is_err());
        assert!(Command::parse(["serve".into(), "--pool".into(), "ai".into()]).is_err());
        assert!(
            Command::parse([
                "worker".into(),
                "--pool".into(),
                "ai".into(),
                "extra".into()
            ])
            .is_err()
        );
    }
}
