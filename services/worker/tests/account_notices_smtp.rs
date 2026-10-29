//! Real bounded loopback SMTP evidence. No mailbox, public network, provider
//! account, credentials or merchant approval is supplied by this fixture.
use anyhow::{Context, ensure};
use deepref_application::account_lifecycle::{
    AccountNoticeDelivery, InactivityNotice, NoticeDeliveryError,
};
use deepref_config::{PublicOrigin, SmtpConfig};
use deepref_worker::account_notices::SmtpAccountNotices;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

async fn peer(
    accept: bool,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<anyhow::Result<String>>)> {
    peer_with_reply(Some(accept)).await
}

async fn peer_with_reply(
    accept: Option<bool>,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<anyhow::Result<String>>)> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    eprintln!("ACCOUNT_NOTICE_SMTP_FIXTURE_PORT={port}");
    let task = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), async move {
            let (socket, _) = listener.accept().await?;
            let (input, mut output) = socket.into_split();
            // Bound the complete command+DATA stream, including one long line.
            let mut input = BufReader::new(input.take(128 * 1024));
            output.write_all(b"220 fixture.local ESMTP\r\n").await?;
            let mut body = String::new();
            loop {
                let mut line = String::new();
                if input.read_line(&mut line).await? == 0 {
                    break;
                }
                let command = line.to_ascii_uppercase();
                if command.starts_with("DATA") {
                    output.write_all(b"354 End with dot\r\n").await?;
                    loop {
                        line.clear();
                        ensure!(
                            input.read_line(&mut line).await? > 0,
                            "SMTP DATA ended without terminator or exceeded bounded stream"
                        );
                        if line == ".\r\n" {
                            break;
                        }
                        body.push_str(&line);
                    }
                    let Some(accept) = accept else {
                        // The peer received all DATA, but its acceptance is
                        // unknown to the sender. Close without a forged250.
                        break;
                    };
                    output
                        .write_all(if accept {
                            b"250 2.0.0 accepted\r\n"
                        } else {
                            b"451 4.3.0 refused\r\n"
                        })
                        .await?;
                } else if command.starts_with("QUIT") {
                    output.write_all(b"221 bye\r\n").await?;
                    break;
                } else {
                    ensure!(
                        command.starts_with("EHLO")
                            || command.starts_with("HELO")
                            || command.starts_with("MAIL FROM:")
                            || command.starts_with("RCPT TO:")
                            || command.starts_with("RSET")
                            || command.starts_with("NOOP"),
                        "unexpected SMTP fixture command"
                    );
                    output.write_all(b"250 fixture.local\r\n").await?;
                }
            }
            Ok(body)
        })
        .await
        .context("bounded SMTP fixture deadline")?
    });
    Ok((port, task))
}

fn adapter(port: u16) -> anyhow::Result<SmtpAccountNotices> {
    Ok(SmtpAccountNotices {
        smtp: SmtpConfig::from_source(&|name| match name {
            "SMTP_HOST" => Some("127.0.0.1".into()),
            "SMTP_PORT" => Some(port.to_string()),
            "SMTP_FROM" => Some("notices@fixture.example".into()),
            "SMTP_TLS" => Some("none".into()),
            _ => None,
        }),
        origin: PublicOrigin::parse("https://deepref.example")?,
    })
}
fn notice() -> anyhow::Result<InactivityNotice> {
    Ok(InactivityNotice {
        id: Uuid::new_v4(),
        recipient: "researcher@fixture.example".into(),
        last_human_activity: "2024-10-29T00:00:00Z".parse()?,
        delete_not_before: "2026-10-29T00:00:00Z".parse()?,
    })
}

#[tokio::test]
async fn actual_smtp_acceptance_preserves_the_logical_warning_identity() -> anyhow::Result<()> {
    let notice = notice()?;
    for _ in 0..2 {
        let (port, task) = peer(true).await?;
        let sender = adapter(port)?;
        let result = sender.deliver(&notice).await;
        // Await peer closure before any assertion, including delivery failure.
        let body = task.await??;
        result?;
        let expected = format!("Message-ID: <{}@deepref.invalid>", notice.id);
        ensure!(
            body.lines().any(|line| line == expected),
            "SMTP accepted the warning but its logical Message-ID was not preserved"
        );
        ensure!(
            body.contains("https://deepref.example/settings"),
            "warning has no account-settings link"
        );
    }
    Ok(())
}

#[tokio::test]
async fn smtp_refusal_or_missing_configuration_is_not_delivery() -> anyhow::Result<()> {
    let notice = notice()?;
    let (port, task) = peer(false).await?;
    let sender = adapter(port)?;
    let result = sender.deliver(&notice).await;
    let _body = task.await??;
    ensure!(
        matches!(result, Err(NoticeDeliveryError::Unavailable)),
        "SMTP451 must never acknowledge a delivered warning"
    );
    let disabled = SmtpAccountNotices {
        smtp: SmtpConfig::default(),
        origin: PublicOrigin::parse("https://deepref.example")?,
    };
    ensure!(
        matches!(
            disabled.deliver(&notice).await,
            Err(NoticeDeliveryError::NotConfigured)
        ),
        "unconfigured sender must never acknowledge a delivered warning"
    );
    Ok(())
}

#[tokio::test]
async fn lost_smtp_acknowledgement_is_not_delivery() -> anyhow::Result<()> {
    let notice = notice()?;
    let (port, task) = peer_with_reply(None).await?;
    let sender = adapter(port)?;
    let result = sender.deliver(&notice).await;
    let body = task.await??;
    ensure!(
        !body.is_empty(),
        "SMTP peer must actually receive DATA before losing its acknowledgement"
    );
    ensure!(
        matches!(result, Err(NoticeDeliveryError::Unavailable)),
        "unknown SMTP acceptance must never acknowledge a delivered warning"
    );
    Ok(())
}
