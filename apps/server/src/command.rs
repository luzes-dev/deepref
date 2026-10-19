use deepref_domain::WorkloadClass;

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
    PrintOpenApi,
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
            _ => anyhow::bail!(
                "usage: deepref-server [serve|worker [--pool <class>[,<class>]]|all [--pool <class>[,<class>]]|migrate|--print-openapi]"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_runtime_role() {
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
        assert!(Command::parse(["import-legacy".into()]).is_err());
        assert_eq!(
            Command::parse(["--print-openapi".into()]).unwrap(),
            Command::PrintOpenApi
        );
    }

    #[test]
    fn defaults_to_all_and_rejects_ambiguous_commands() {
        assert_eq!(
            Command::parse(Vec::<String>::new()).unwrap(),
            Command::All {
                pools: WorkloadClass::ALL.to_vec()
            }
        );
        assert!(Command::parse(["serve".into(), "migrate".into()]).is_err());
        assert!(Command::parse(["unknown".into()]).is_err());
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
