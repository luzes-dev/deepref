use anyhow::{Context, Result};
use deepref_http_api::DatabaseRolePurpose;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseOperatorCommand {
    Check(DatabaseRolePurpose),
    Provision {
        runtime_login: String,
        migration_login: String,
        maintenance_login: String,
    },
}
const USAGE: &str = "usage: deepref-server database-roles check <runtime|migration|maintenance> | database-roles provision --runtime-login <role> --migration-login <role> --maintenance-login <role>";
impl DatabaseOperatorCommand {
    pub fn parse(args: &[String]) -> Result<Self> {
        match args {
            [action, purpose] if action == "check" => Ok(Self::Check(match purpose.as_str() {
                "runtime" => DatabaseRolePurpose::Runtime,
                "migration" => DatabaseRolePurpose::Migration,
                "maintenance" => DatabaseRolePurpose::Maintenance,
                _ => anyhow::bail!(USAGE),
            })),
            [action, rest @ ..] if action == "provision" => {
                let (mut runtime, mut migration, mut maintenance) = (None, None, None);
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let value = flags.next().context(USAGE)?.clone();
                    match flag.as_str() {
                        "--runtime-login" if runtime.is_none() => runtime = Some(value),
                        "--migration-login" if migration.is_none() => migration = Some(value),
                        "--maintenance-login" if maintenance.is_none() => maintenance = Some(value),
                        _ => anyhow::bail!(USAGE),
                    }
                }
                Ok(Self::Provision {
                    runtime_login: runtime.context(USAGE)?,
                    migration_login: migration.context(USAGE)?,
                    maintenance_login: maintenance.context(USAGE)?,
                })
            }
            _ => anyhow::bail!(USAGE),
        }
    }
}

pub async fn run(
    command: DatabaseOperatorCommand,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<()> {
    let config = deepref_http_api::config::ApiConfig::from_source(get)?;
    let output = match command {
        DatabaseOperatorCommand::Check(purpose) => {
            deepref_http_api::check_database_role(&config, purpose).await?
        }
        DatabaseOperatorCommand::Provision {
            runtime_login,
            migration_login,
            maintenance_login,
        } => {
            deepref_http_api::provision_database_roles(
                &config,
                &runtime_login,
                &migration_login,
                &maintenance_login,
            )
            .await?
        }
    };
    println!("{output}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_duplicate_or_unknown_role_flags() -> Result<()> {
        let args = |a: &[&str]| a.iter().map(|s| (*s).into()).collect::<Vec<String>>();
        anyhow::ensure!(matches!(
            DatabaseOperatorCommand::parse(&args(&["check", "runtime"]))?,
            DatabaseOperatorCommand::Check(DatabaseRolePurpose::Runtime)
        ));
        for bad in [
            vec!["check", "superuser"],
            vec!["provision"],
            vec![
                "provision",
                "--runtime-login",
                "one",
                "--runtime-login",
                "two",
            ],
        ] {
            anyhow::ensure!(DatabaseOperatorCommand::parse(&args(&bad)).is_err());
        }
        Ok(())
    }
}
