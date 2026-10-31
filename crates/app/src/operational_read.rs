//! Bounded operational reads use one connection and one consistent snapshot.
use sqlx::{PgPool, Postgres, Transaction};

pub(crate) async fn begin(pool: &PgPool) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await?;
    // Install backend deadlines before any catalog or application read. The
    // caller also has a total client deadline. Dropping its future cannot leave
    // a lock wait unbounded; an idle aborted connection is bounded as well.
    sqlx::query("SET LOCAL statement_timeout='1s'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='250ms'")
        .execute(&mut *transaction)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='2s'")
        .execute(&mut *transaction)
        .await?;
    Ok(transaction)
}
