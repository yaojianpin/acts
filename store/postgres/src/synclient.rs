use futures::executor::block_on;
use sqlx::{
    Database, Error, IntoArguments, PgPool, Postgres,
    postgres::{PgPoolOptions, PgRow},
};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SynClient {
    pool: PgPool,
}

impl SynClient {
    pub fn connect(db_url: &str) -> Self {
        let pool = block_on(async move {
            PgPoolOptions::new()
                .acquire_timeout(Duration::from_secs(5))
                .max_connections(200)
                .connect(db_url)
                .await
        })
        .expect(&format!("failed to connect to DB {}", db_url));

        Self { pool }
    }

    pub fn query_one<'q, A>(&self, sql: &'q str, params: A) -> Result<PgRow, Error>
    where
        A: IntoArguments<'q, Postgres> + 'q,
    {
        block_on(async move {
            let mut conn = self
                .pool
                .acquire()
                .await
                .expect("failed to get connection from pool");

            sqlx::query_with(sql, params).fetch_one(&mut *conn).await
        })
    }

    pub fn query<'q, A>(&self, sql: &'q str, params: A) -> Result<Vec<PgRow>, Error>
    where
        A: IntoArguments<'q, Postgres> + 'q,
    {
        block_on(async move {
            let mut conn = self
                .pool
                .acquire()
                .await
                .expect("failed to get connection from pool");

            sqlx::query_with(sql, params).fetch_all(&mut *conn).await
        })
    }

    pub fn execute<'q, A>(
        &self,
        sql: &'q str,
        params: A,
    ) -> Result<<Postgres as Database>::QueryResult, Error>
    where
        A: IntoArguments<'q, Postgres> + 'q,
    {
        block_on(async move {
            let mut conn = self
                .pool
                .acquire()
                .await
                .expect("failed to get connection from pool");

            sqlx::query_with(sql, params).execute(&mut *conn).await
        })
    }

    pub fn batch_execute(&self, sqls: &[String]) -> Result<(), Error> {
        block_on(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .expect("failed to get connection from pool");

            for sql in sqls {
                sqlx::query(sql).execute(&mut *tx).await?;
            }
            tx.commit().await
        })
    }
}
