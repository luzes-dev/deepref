//! Shutdown is bounded even when a request never finishes.
use std::time::{Duration, Instant};

use axum::{Router, routing::get};
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

#[tokio::test]
async fn a_hung_request_cannot_hold_shutdown_past_the_drain_deadline() -> anyhow::Result<()> {
    let (entered_tx, entered_rx) = oneshot::channel::<()>();
    let entered = std::sync::Mutex::new(Some(entered_tx));
    let entered = std::sync::Arc::new(entered);
    let app = Router::new().route(
        "/hang",
        get(move || {
            let entered = entered.clone();
            async move {
                if let Some(tx) = entered.lock().expect("lock").take() {
                    let _ = tx.send(());
                }
                std::future::pending::<()>().await;
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(deepref_http_api::serve_until_drained(
        listener,
        app,
        async move {
            let _ = stop_rx.await;
        },
        Duration::from_millis(300),
    ));
    let mut client = tokio::net::TcpStream::connect(address).await?;
    client
        .write_all(b"GET /hang HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await?;
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;

    let started = Instant::now();
    let _ = stop_tx.send(());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .map_err(|_| anyhow::anyhow!("shutdown waited on the hung request"))???;
    let elapsed = started.elapsed();
    anyhow::ensure!(
        elapsed >= Duration::from_millis(250),
        "in-flight requests must get the drain window: {elapsed:?}"
    );
    anyhow::ensure!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    drop(client);
    Ok(())
}

#[tokio::test]
async fn an_idle_server_stops_immediately_on_shutdown() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let started = Instant::now();
    deepref_http_api::serve_until_drained(
        listener,
        Router::new(),
        async {},
        Duration::from_secs(30),
    )
    .await?;
    anyhow::ensure!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}
