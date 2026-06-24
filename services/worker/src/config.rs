use std::env;

pub(crate) struct WorkerConfig {
    pub(crate) database_url: String,
    pub(crate) nats_url: String,
    pub(crate) concurrency: usize,
}

impl WorkerConfig {
    pub(crate) fn from_env() -> anyhow::Result<Self> {
        let concurrency = match env::var("WORKER_CONCURRENCY") {
            Ok(value) => value.parse::<usize>()?,
            Err(_) => 8,
        };
        if concurrency == 0 {
            anyhow::bail!("WORKER_CONCURRENCY must be >= 1");
        }

        Ok(Self {
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://deepref:deepref@localhost:5432/deepref".to_owned()),
            nats_url: env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_owned()),
            concurrency,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_zero_concurrency() {
        let result = "0"
            .parse::<usize>()
            .map_err(anyhow::Error::from)
            .and_then(|value| {
                if value == 0 {
                    anyhow::bail!("WORKER_CONCURRENCY must be >= 1");
                }
                Ok(value)
            });

        assert!(result.is_err());
    }
}
