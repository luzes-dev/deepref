use deepref_config::RuntimeConfig;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectorCommand {
    Run,
    Migrate,
    Status,
    Rebuild { run_id: Uuid },
}

impl ProjectorCommand {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let arguments = arguments.into_iter().collect::<Vec<_>>();
        match arguments.as_slice() {
            [] => Ok(Self::Run),
            [command] if command == "run" => Ok(Self::Run),
            [command] if command == "migrate" => Ok(Self::Migrate),
            [command] if command == "status" => Ok(Self::Status),
            [command, flag, value] if command == "rebuild" && flag == "--run-id" => {
                Ok(Self::Rebuild {
                    run_id: Uuid::parse_str(value)?,
                })
            }
            _ => {
                anyhow::bail!("usage: deepref-projector [run|migrate|status|rebuild --run-id UUID]")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectorConfig {
    pub runtime: RuntimeConfig,
    pub command: ProjectorCommand,
    pub batch_size: i64,
    pub advisory_lock_key: i64,
}

impl ProjectorConfig {
    pub fn from_env(arguments: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let batch_size = std::env::var("PROJECTOR_BATCH_SIZE")
            .ok()
            .map(|value| value.parse())
            .transpose()?
            .unwrap_or(1_000_i64);
        if !(1..=10_000).contains(&batch_size) {
            anyhow::bail!("PROJECTOR_BATCH_SIZE must be between 1 and 10000");
        }
        Ok(Self {
            runtime: RuntimeConfig::from_env("deepref-projector")?,
            command: ProjectorCommand::parse(arguments)?,
            batch_size,
            advisory_lock_key: 4_445_570_701_i64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_requires_explicit_run_id() {
        assert!(ProjectorCommand::parse(["rebuild".into()]).is_err());
        assert!(matches!(
            ProjectorCommand::parse(["rebuild".into(), "--run-id".into(), Uuid::nil().to_string()])
                .unwrap(),
            ProjectorCommand::Rebuild { .. }
        ));
    }
}
