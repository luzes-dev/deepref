use async_nats::jetstream;
use deepref_events::STREAM;

pub(crate) async fn connect_jetstream(
    nats_url: &str,
) -> anyhow::Result<Option<jetstream::Context>> {
    match async_nats::connect(nats_url).await {
        Ok(client) => {
            let jetstream = jetstream::new(client);
            ensure_stream(&jetstream).await?;
            Ok(Some(jetstream))
        }
        Err(error) => {
            tracing::warn!(%error, "NATS JetStream unavailable; ingestion publishing is disabled");
            Ok(None)
        }
    }
}

async fn ensure_stream(jetstream: &jetstream::Context) -> anyhow::Result<()> {
    jetstream
        .create_or_update_stream(jetstream::stream::Config {
            name: STREAM.to_owned(),
            subjects: vec![
                "work.>".to_owned(),
                "metrics.>".to_owned(),
                "ingestion.>".to_owned(),
            ],
            ..Default::default()
        })
        .await?;
    Ok(())
}
