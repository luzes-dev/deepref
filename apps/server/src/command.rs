use deepref_domain::WorkloadClass;
use deepref_http_api::AuditQueryParams;
use uuid::Uuid;

/// Quiet window `ai-usage enable-export` checks without `--quiet-window-seconds`.
const DEFAULT_EXPORT_QUIET_WINDOW_SECONDS: u64 = 300;

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
    ActivateTenantObjectKeys {
        drain_attestation: String,
    },
    ReconcileObjectOrphans {
        drain_attestation: String,
    },
    WorkflowStorage {
        contract: bool,
        drain_attestation: String,
    },
    BackfillProjectWorkspaces,
    LegacyActor(LegacyActorCommand),
    Audit(AuditCommand),
    Accounts(AccountsCommand),
    AiUsage(AiUsageCommand),
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
pub enum AiUsageCommand {
    Backfill {
        batch_size: usize,
    },
    EnableHardCaps {
        operator_user_id: Uuid,
        quiet_window_seconds: u64,
    },
    DisableHardCaps {
        operator_user_id: Uuid,
    },
    EnableExport {
        operator_user_id: Uuid,
        quiet_window_seconds: u64,
    },
    DisableExport {
        operator_user_id: Uuid,
    },
    Resolve {
        reservation_id: Uuid,
        operator_user_id: Uuid,
        resolution: AiUsageResolutionCommand,
    },
    Reconcile {
        workspace_id: Option<Uuid>,
    },
    /// Emit usage fact export events from retained usage facts.
    ExportReplay {
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: Option<i64>,
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
    ListUnresolvedExports {
        limit: Option<i64>,
    },
    ResolveExportHold {
        fact_id: Uuid,
        operator_user_id: Uuid,
        resolution_id: Uuid,
        action: String,
        reason: String,
        notes: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiUsageResolutionCommand {
    Settle { actual_cost_micros: i64 },
    Void { reason: String },
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
            [command, phase, flag, attestation]
                if command == "workflow-storage"
                    && flag == "--confirm-drained"
                    && (phase == "object-only" || phase == "contract") =>
            {
                Ok(Self::WorkflowStorage {
                    contract: phase == "contract",
                    drain_attestation: attestation.clone(),
                })
            }
            [command, phase, flag, attestation]
                if command == "object-storage"
                    && phase == "reconcile-orphans"
                    && flag == "--confirm-drained" =>
            {
                Ok(Self::ReconcileObjectOrphans {
                    drain_attestation: attestation.clone(),
                })
            }
            [command, phase, flag, attestation]
                if command == "object-storage"
                    && phase == "activate-tenant-keys"
                    && flag == "--confirm-drained" =>
            {
                Ok(Self::ActivateTenantObjectKeys {
                    drain_attestation: attestation.clone(),
                })
            }
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
            [command, action] if command == "ai-usage" && action == "list-unresolved-exports" => {
                Ok(Self::AiUsage(AiUsageCommand::ListUnresolvedExports {
                    limit: None,
                }))
            }
            [command, action, flag, limit]
                if command == "ai-usage"
                    && action == "list-unresolved-exports"
                    && flag == "--limit" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::ListUnresolvedExports {
                    limit: Some(parse_limit(limit)?),
                }))
            }
            [command, action, fact_id, rest @ ..]
                if command == "ai-usage" && action == "resolve-export-hold" =>
            {
                let fact_uuid = parse_uuid(fact_id, "fact UUID")?;
                let mut operator = None;
                let mut evidence = None;
                let mut act = None;
                let mut reason = None;
                let mut notes = None;

                let mut chunks = rest.chunks_exact(2);
                for chunk in &mut chunks {
                    match chunk[0].as_str() {
                        "--operator" => {
                            operator = Some(parse_uuid(&chunk[1], "operator UUID")?);
                        }
                        "--evidence" => {
                            evidence = Some(parse_uuid(&chunk[1], "evidence resolution UUID")?);
                        }
                        "--action" => {
                            act = Some(chunk[1].clone());
                        }
                        "--reason" => {
                            reason = Some(chunk[1].clone());
                        }
                        "--notes" => {
                            let note = chunk[1].trim();
                            anyhow::ensure!(!note.is_empty(), "--notes cannot be empty");
                            anyhow::ensure!(
                                note.len() <= 500,
                                "--notes must be at most 500 characters"
                            );
                            notes = Some(note.to_string());
                        }
                        flag => anyhow::bail!("unknown flag for resolve-export-hold: {flag}"),
                    }
                }
                anyhow::ensure!(
                    chunks.remainder().is_empty(),
                    "unmatched trailing argument for resolve-export-hold"
                );
                let operator_user_id =
                    operator.ok_or_else(|| anyhow::anyhow!("missing required --operator flag"))?;
                let resolution_id =
                    evidence.ok_or_else(|| anyhow::anyhow!("missing required --evidence flag"))?;
                let action =
                    act.ok_or_else(|| anyhow::anyhow!("missing required --action flag"))?;
                let reason =
                    reason.ok_or_else(|| anyhow::anyhow!("missing required --reason flag"))?;

                Ok(Self::AiUsage(AiUsageCommand::ResolveExportHold {
                    fact_id: fact_uuid,
                    operator_user_id,
                    resolution_id,
                    action,
                    reason,
                    notes,
                }))
            }
            [command, action, workspace_id] if command == "ai-usage" && action == "reconcile" => {
                Ok(Self::AiUsage(AiUsageCommand::Reconcile {
                    workspace_id: Some(parse_uuid(workspace_id, "workspace UUID")?),
                }))
            }
            [command, action] if command == "ai-usage" && action == "export-replay" => {
                Ok(Self::AiUsage(AiUsageCommand::ExportReplay {
                    since: None,
                    limit: None,
                }))
            }
            [command, action, flag, since]
                if command == "ai-usage" && action == "export-replay" && flag == "--since" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::ExportReplay {
                    since: Some(parse_since(since)?),
                    limit: None,
                }))
            }
            [command, action, flag, limit]
                if command == "ai-usage" && action == "export-replay" && flag == "--limit" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::ExportReplay {
                    since: None,
                    limit: Some(parse_limit(limit)?),
                }))
            }
            [command, action, since_flag, since, limit_flag, limit]
                if command == "ai-usage"
                    && action == "export-replay"
                    && since_flag == "--since"
                    && limit_flag == "--limit" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::ExportReplay {
                    since: Some(parse_since(since)?),
                    limit: Some(parse_limit(limit)?),
                }))
            }
            [command, action] if command == "ai-usage" && action == "backfill" => {
                Ok(Self::AiUsage(AiUsageCommand::Backfill { batch_size: 500 }))
            }
            [command, action, flag, batch_size]
                if command == "ai-usage" && action == "backfill" && flag == "--batch-size" =>
            {
                let batch_size = batch_size
                    .parse::<usize>()
                    .map_err(|error| anyhow::anyhow!("invalid batch size: {error}"))?;
                anyhow::ensure!(
                    (1..=1000).contains(&batch_size),
                    "batch size must be 1..=1000"
                );
                Ok(Self::AiUsage(AiUsageCommand::Backfill { batch_size }))
            }
            [command, action, flag, seconds, operator_user_id]
                if command == "ai-usage"
                    && action == "enable-hard-caps"
                    && flag == "--quiet-window-seconds" =>
            {
                let quiet_window_seconds = seconds
                    .parse::<u64>()
                    .map_err(|error| anyhow::anyhow!("invalid quiet window: {error}"))?;
                anyhow::ensure!(quiet_window_seconds > 0, "quiet window must be positive");
                Ok(Self::AiUsage(AiUsageCommand::EnableHardCaps {
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                    quiet_window_seconds,
                }))
            }
            [command, action, operator_user_id]
                if command == "ai-usage" && action == "disable-hard-caps" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::DisableHardCaps {
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                }))
            }
            [command, action, operator_user_id]
                if command == "ai-usage" && action == "enable-export" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::EnableExport {
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                    quiet_window_seconds: DEFAULT_EXPORT_QUIET_WINDOW_SECONDS,
                }))
            }
            [command, action, flag, seconds, operator_user_id]
                if command == "ai-usage"
                    && action == "enable-export"
                    && flag == "--quiet-window-seconds" =>
            {
                let quiet_window_seconds = seconds
                    .parse::<u64>()
                    .map_err(|error| anyhow::anyhow!("invalid quiet window: {error}"))?;
                anyhow::ensure!(quiet_window_seconds > 0, "quiet window must be positive");
                Ok(Self::AiUsage(AiUsageCommand::EnableExport {
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                    quiet_window_seconds,
                }))
            }
            [command, action, operator_user_id]
                if command == "ai-usage" && action == "disable-export" =>
            {
                Ok(Self::AiUsage(AiUsageCommand::DisableExport {
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                }))
            }
            [
                command,
                action,
                reservation_id,
                flag,
                micros,
                operator_flag,
                operator_user_id,
            ] if command == "ai-usage"
                && action == "resolve"
                && flag == "--settle"
                && operator_flag == "--operator" =>
            {
                let actual_cost_micros = micros
                    .parse::<i64>()
                    .map_err(|error| anyhow::anyhow!("invalid cost micros: {error}"))?;
                anyhow::ensure!(actual_cost_micros >= 0, "cost micros must be nonnegative");
                Ok(Self::AiUsage(AiUsageCommand::Resolve {
                    reservation_id: parse_uuid(reservation_id, "reservation UUID")?,
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                    resolution: AiUsageResolutionCommand::Settle { actual_cost_micros },
                }))
            }
            [
                command,
                action,
                reservation_id,
                flag,
                reason_flag,
                reason,
                operator_flag,
                operator_user_id,
            ] if command == "ai-usage"
                && action == "resolve"
                && flag == "--void"
                && reason_flag == "--reason"
                && operator_flag == "--operator" =>
            {
                anyhow::ensure!(
                    !reason.trim().is_empty() && reason.len() <= 500,
                    "reason must be 1..=500 characters"
                );
                Ok(Self::AiUsage(AiUsageCommand::Resolve {
                    reservation_id: parse_uuid(reservation_id, "reservation UUID")?,
                    operator_user_id: parse_uuid(operator_user_id, "operator UUID")?,
                    resolution: AiUsageResolutionCommand::Void {
                        reason: reason.clone(),
                    },
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
                "usage: deepref-server [serve|worker [--pool <class>[,<class>]]|all [--pool <class>[,<class>]]|migrate|backfill-project-workspaces|legacy-actor map <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor deny <user> <legacy-id> <user-uuid> <operator-uuid> <source>|legacy-actor list|audit purge [--dry-run]|audit query [--workspace <uuid>] [--principal <uuid>] [--target <uuid>] [--event-type <type>] [--since <rfc3339>] [--until <rfc3339>] [--limit <n>]|accounts complete-deletions|ai-usage backfill [--batch-size <1..1000>]|ai-usage enable-hard-caps --quiet-window-seconds <seconds> <operator-user-uuid>|ai-usage disable-hard-caps <operator-user-uuid>|ai-usage enable-export [--quiet-window-seconds <seconds>] <operator-user-uuid>|ai-usage disable-export <operator-user-uuid>|ai-usage resolve <reservation-uuid> --settle <micros> --operator <user-uuid>|ai-usage resolve <reservation-uuid> --void --reason <text> --operator <user-uuid>|ai-usage reconcile [workspace-uuid]|ai-usage export-replay [--since <rfc3339>] [--limit <rows>]|ai-usage waive-overage <workspace-uuid> <operator-user-uuid>|ai-usage set-cap <platform|workspace|project> <-|scope-uuid> <limit-micros> <operator-user-uuid>|ai-usage list-unresolved-exports [--limit <rows>]|ai-usage resolve-export-hold <fact-uuid> --operator <user-uuid> --evidence <resolution-uuid> --action <action> --reason <reason> [--notes <text>]|--print-openapi]"
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

fn parse_since(value: &str) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&chrono::Utc))
        .map_err(|error| anyhow::anyhow!("invalid --since timestamp: {error}"))
}

fn parse_limit(value: &str) -> anyhow::Result<i64> {
    let limit = value
        .parse::<i64>()
        .map_err(|error| anyhow::anyhow!("invalid replay limit: {error}"))?;
    anyhow::ensure!(limit > 0, "replay limit must be positive");
    Ok(limit)
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
        assert_eq!(
            Command::parse(["ai-usage".into(), "backfill".into()]).unwrap(),
            Command::AiUsage(AiUsageCommand::Backfill { batch_size: 500 })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "backfill".into(),
                "--batch-size".into(),
                "25".into(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::Backfill { batch_size: 25 })
        );
        let workspace_id = Uuid::from_u128(55);
        let operator_user_id = Uuid::from_u128(56);
        let reservation_id = Uuid::from_u128(57);
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "enable-hard-caps".into(),
                "--quiet-window-seconds".into(),
                "3600".into(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::EnableHardCaps {
                operator_user_id,
                quiet_window_seconds: 3600,
            })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "disable-hard-caps".into(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::DisableHardCaps { operator_user_id })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "resolve".into(),
                reservation_id.to_string(),
                "--settle".into(),
                "150".into(),
                "--operator".into(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::Resolve {
                reservation_id,
                operator_user_id,
                resolution: AiUsageResolutionCommand::Settle {
                    actual_cost_micros: 150
                },
            })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "resolve".into(),
                reservation_id.to_string(),
                "--void".into(),
                "--reason".into(),
                "provider confirmed no charge".into(),
                "--operator".into(),
                operator_user_id.to_string(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::Resolve {
                reservation_id,
                operator_user_id,
                resolution: AiUsageResolutionCommand::Void {
                    reason: "provider confirmed no charge".into(),
                },
            })
        );
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
        assert!(
            Command::parse([
                "ai-usage".into(),
                "backfill".into(),
                "--batch-size".into(),
                "1001".into(),
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

    #[test]
    fn parses_usage_export_replay_flags() {
        assert_eq!(
            Command::parse(["ai-usage".into(), "export-replay".into()]).unwrap(),
            Command::AiUsage(AiUsageCommand::ExportReplay {
                since: None,
                limit: None,
            })
        );
        let since = "2026-10-01T00:00:00Z";
        let expected = chrono::DateTime::parse_from_rfc3339(since)
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "export-replay".into(),
                "--since".into(),
                since.into(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::ExportReplay {
                since: Some(expected),
                limit: None,
            })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "export-replay".into(),
                "--limit".into(),
                "25".into(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::ExportReplay {
                since: None,
                limit: Some(25),
            })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "export-replay".into(),
                "--since".into(),
                since.into(),
                "--limit".into(),
                "25".into(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::ExportReplay {
                since: Some(expected),
                limit: Some(25),
            })
        );
        assert!(
            Command::parse([
                "ai-usage".into(),
                "export-replay".into(),
                "--since".into(),
                "yesterday".into(),
            ])
            .is_err()
        );
        assert!(
            Command::parse([
                "ai-usage".into(),
                "export-replay".into(),
                "--limit".into(),
                "0".into(),
            ])
            .is_err()
        );
    }
    #[test]
    fn ai_usage_list_unresolved_exports_command_parses() {
        assert_eq!(
            Command::parse(["ai-usage".into(), "list-unresolved-exports".into(),]).unwrap(),
            Command::AiUsage(AiUsageCommand::ListUnresolvedExports { limit: None })
        );
        assert_eq!(
            Command::parse([
                "ai-usage".into(),
                "list-unresolved-exports".into(),
                "--limit".into(),
                "25".into(),
            ])
            .unwrap(),
            Command::AiUsage(AiUsageCommand::ListUnresolvedExports { limit: Some(25) })
        );
        assert!(
            Command::parse([
                "ai-usage".into(),
                "list-unresolved-exports".into(),
                "--limit".into(),
                "0".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn ai_usage_resolve_export_hold_command_parses() {
        let fact_id = Uuid::new_v4();
        let operator_id = Uuid::new_v4();
        let evidence_id = Uuid::new_v4();

        let cmd = Command::parse([
            "ai-usage".into(),
            "resolve-export-hold".into(),
            fact_id.to_string(),
            "--operator".into(),
            operator_id.to_string(),
            "--evidence".into(),
            evidence_id.to_string(),
            "--action".into(),
            "reopened_retry".into(),
            "--reason".into(),
            "external_adjustment_confirmed".into(),
        ])
        .unwrap();
        assert_eq!(
            cmd,
            Command::AiUsage(AiUsageCommand::ResolveExportHold {
                fact_id,
                operator_user_id: operator_id,
                resolution_id: evidence_id,
                action: "reopened_retry".into(),
                reason: "external_adjustment_confirmed".into(),
                notes: None,
            })
        );

        let cmd2 = Command::parse([
            "ai-usage".into(),
            "resolve-export-hold".into(),
            fact_id.to_string(),
            "--reason".into(),
            "manual_credit_issued".into(),
            "--notes".into(),
            "verified credit memo #12345".into(),
            "--action".into(),
            "manual_reconciliation".into(),
            "--evidence".into(),
            evidence_id.to_string(),
            "--operator".into(),
            operator_id.to_string(),
        ])
        .unwrap();
        assert_eq!(
            cmd2,
            Command::AiUsage(AiUsageCommand::ResolveExportHold {
                fact_id,
                operator_user_id: operator_id,
                resolution_id: evidence_id,
                action: "manual_reconciliation".into(),
                reason: "manual_credit_issued".into(),
                notes: Some("verified credit memo #12345".into()),
            })
        );

        assert!(
            Command::parse([
                "ai-usage".into(),
                "resolve-export-hold".into(),
                fact_id.to_string(),
                "--operator".into(),
                operator_id.to_string(),
                "--evidence".into(),
                evidence_id.to_string(),
                "--action".into(),
                "manual_reconciliation".into(),
            ])
            .is_err()
        );

        let long_notes = "a".repeat(501);
        assert!(
            Command::parse([
                "ai-usage".into(),
                "resolve-export-hold".into(),
                fact_id.to_string(),
                "--operator".into(),
                operator_id.to_string(),
                "--evidence".into(),
                evidence_id.to_string(),
                "--action".into(),
                "manual_reconciliation".into(),
                "--reason".into(),
                "manual_credit_issued".into(),
                "--notes".into(),
                long_notes,
            ])
            .is_err()
        );
    }
}
