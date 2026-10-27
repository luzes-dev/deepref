//! Explicit finite-delivery diagnostic maintenance. Never claims/replays jobs.
use anyhow::{Context, Result};
use deepref_config::{RuntimeConfig, WorkTransportConfig};
use deepref_integrations::jetstream::JetStream;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportOperatorCommand {
    Status { after: u64, limit: u16 },
    RecoverTerminal { advisory_sequence: u64 },
}

const USAGE: &str = "usage: deepref-server transport terminal-status [--after <advisory-sequence>] [--limit <1..100>] | transport recover-terminal --advisory <positive-advisory-sequence>";

impl TransportOperatorCommand {
    pub fn parse(args: &[String]) -> Result<Self> {
        match args {
            [action, rest @ ..] if action == "terminal-status" => {
                let mut after = None;
                let mut limit = None;
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let value = flags.next().context(USAGE)?;
                    match flag.as_str() {
                        "--after" if after.is_none() => {
                            after = Some(value.parse::<u64>().context(USAGE)?)
                        }
                        "--limit" if limit.is_none() => {
                            limit = Some(value.parse::<u16>().context(USAGE)?)
                        }
                        _ => anyhow::bail!(USAGE),
                    }
                }
                let limit = limit.unwrap_or(25);
                anyhow::ensure!((1..=100).contains(&limit), USAGE);
                Ok(Self::Status {
                    after: after.unwrap_or(0),
                    limit,
                })
            }
            [action, flag, value] if action == "recover-terminal" && flag == "--advisory" => {
                let advisory_sequence = value.parse::<u64>().context(USAGE)?;
                anyhow::ensure!(advisory_sequence > 0, USAGE);
                Ok(Self::RecoverTerminal { advisory_sequence })
            }
            _ => anyhow::bail!(USAGE),
        }
    }
}

/// Retained server advisory is mandatory. Recovery records DB evidence first,
/// then the diagnostic DLQ copy. Its unique identity makes repeat runs safe.
pub async fn recover_terminal_advisory(
    pool: &PgPool,
    broker: &JetStream,
    sequence: u64,
) -> Result<serde_json::Value> {
    let advisory = broker.terminal_advisory(sequence).await?;
    super::jetstream_transport::recover_terminal_sequence(
        pool,
        broker,
        &advisory.consumer,
        advisory.stream_seq,
    )
    .await?;
    Ok(
        serde_json::json!({"advisory_sequence":sequence,"stream_sequence":advisory.stream_seq,
        "consumer":advisory.consumer,"database_evidence":"recorded","diagnostic_dlq":"confirmed",
        "business_replayed":false,"delivery_budget_reset":false}),
    )
}

pub async fn run(config: RuntimeConfig, command: TransportOperatorCommand) -> Result<String> {
    let WorkTransportConfig::JetStream(nats) = config.work_transport else {
        anyhow::bail!("terminal diagnostics require explicitly configured JetStream transport");
    };
    // This absolute budget also bounds connect/management/database requests.
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        let broker = JetStream::connect_existing(nats).await?;
        let output = match command {
            TransportOperatorCommand::Status { after, limit } => {
                serde_json::to_string_pretty(&broker.terminal_advisories(after, limit).await?)?
            }
            TransportOperatorCommand::RecoverTerminal { advisory_sequence } => {
                let pool = PgPoolOptions::new()
                    .min_connections(0)
                    .max_connections(2)
                    .acquire_timeout(Duration::from_secs(2))
                    .connect(config.database.url())
                    .await?;
                if !config.environment.is_development() {
                    deepref_app::database_roles::verify_database_role(
                        &pool,
                        deepref_app::database_roles::DatabaseRolePurpose::Runtime,
                    )
                    .await?;
                }
                let recovered = recover_terminal_advisory(&pool, &broker, advisory_sequence).await;
                super::shutdown::close_control_pool(&pool, Duration::from_secs(2)).await;
                serde_json::to_string_pretty(&recovered?)?
            }
        };
        Ok::<_, anyhow::Error>(output)
    })
    .await
    .context(
        "terminal diagnostic deadline exceeded; inspect persisted evidence before retrying",
    )??;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_bounds_diagnostics_and_has_no_retry_reset_action() -> Result<()> {
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        anyhow::ensure!(
            TransportOperatorCommand::parse(&args(&["terminal-status"]))?
                == TransportOperatorCommand::Status {
                    after: 0,
                    limit: 25
                }
        );
        for bad in [
            vec!["terminal-status", "--limit", "0"],
            vec!["terminal-status", "--limit", "101"],
            vec!["terminal-status", "--limit", "2", "--limit", "3"],
            vec!["recover-terminal", "--advisory", "0"],
            vec!["reset-business-attempts"],
        ] {
            anyhow::ensure!(TransportOperatorCommand::parse(&args(&bad)).is_err());
        }
        anyhow::ensure!(
            TransportOperatorCommand::parse(&args(&["recover-terminal", "--advisory", "12"]))?
                == TransportOperatorCommand::RecoverTerminal {
                    advisory_sequence: 12
                }
        );
        Ok(())
    }
}
