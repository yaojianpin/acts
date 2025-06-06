use super::{DbConnection, into_query, map_db_err};
use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Index, Order as SeaOrder,
    PostgresQueryBuilder, Query as SeaQuery, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Error as DbError, Row, postgres::PgRow};

#[derive(Debug)]
pub struct ProcCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "procs"]
enum CollectionIden {
    Table,
    Id,
    State,
    Mid,
    Name,
    StartTime,
    EndTime,
    Timestamp,
    Model,
    Env,
    Err,
}

impl DbCollection for ProcCollection {
    type Item = data::Proc;

    fn exists(&self, id: &str) -> Result<bool> {
        let (sql, values) = SeaQuery::select()
            .from(CollectionIden::Table)
            .expr(SeaFunc::count(SeaExpr::col(CollectionIden::Id)))
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_sqlx(PostgresQueryBuilder);

        let count = self
            .conn
            .query_one(sql.as_str(), values)
            .map(|row| row.get::<i64, usize>(0))
            .map_err(map_db_err)?;

        Ok(count > 0)
    }

    fn find(&self, id: &str) -> Result<Self::Item> {
        let (sql, values) = SeaQuery::select()
            .from(CollectionIden::Table)
            .columns([
                CollectionIden::Id,
                CollectionIden::State,
                CollectionIden::Mid,
                CollectionIden::Name,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Timestamp,
                CollectionIden::Model,
                CollectionIden::Env,
                CollectionIden::Err,
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_sqlx(PostgresQueryBuilder);

        self.conn
            .query_one(&sql, values)
            .map(|row| Self::Item::from_row(&row).map_err(map_db_err))
            .map_err(map_db_err)?
    }

    fn query(&self, q: &acts::query::Query) -> Result<acts::PageData<Self::Item>> {
        let filter = into_query(q);

        let mut count_query = SeaQuery::select();
        count_query
            .from(CollectionIden::Table)
            .expr(SeaFunc::count(SeaExpr::col(SeaAlias::new("id"))));

        let mut query = SeaQuery::select();
        query
            .columns([
                CollectionIden::Id,
                CollectionIden::State,
                CollectionIden::Mid,
                CollectionIden::Name,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Timestamp,
                CollectionIden::Model,
                CollectionIden::Env,
                CollectionIden::Err,
            ])
            .from(CollectionIden::Table);

        if !filter.is_empty() {
            count_query.cond_where(filter.clone());
            query.cond_where(filter);
        }

        if !q.order_by().is_empty() {
            for (order, rev) in q.order_by().iter() {
                query.order_by(
                    SeaAlias::new(order),
                    if *rev { SeaOrder::Desc } else { SeaOrder::Asc },
                );
            }
        }
        let (sql, values) = query
            .limit(q.limit() as u64)
            .offset(q.offset() as u64)
            .build_sqlx(PostgresQueryBuilder);

        let (count_sql, count_values) = count_query.build_sqlx(PostgresQueryBuilder);
        let count = self
            .conn
            .query_one(count_sql.as_str(), count_values)
            .map_err(map_db_err)?
            .get::<i64, usize>(0) as usize;
        let page_count = count.div_ceil(q.limit());
        let page_num = q.offset() / q.limit() + 1;
        let data = PageData {
            count,
            page_size: q.limit(),
            page_num,
            page_count,
            rows: self
                .conn
                .query(&sql, values)
                .map_err(map_db_err)?
                .iter()
                .map(|row| Self::Item::from_row(row).unwrap())
                .collect::<Vec<_>>(),
        };
        Ok(data)
    }

    fn create(&self, data: &Self::Item) -> Result<bool> {
        let data = data.clone();
        let (sql, sql_values) = SeaQuery::insert()
            .into_table(CollectionIden::Table)
            .columns([
                CollectionIden::Id,
                CollectionIden::State,
                CollectionIden::Mid,
                CollectionIden::Name,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Timestamp,
                CollectionIden::Model,
                CollectionIden::Env,
                CollectionIden::Err,
            ])
            .values([
                data.id.into(),
                data.state.into(),
                data.mid.into(),
                data.name.into(),
                data.start_time.into(),
                data.end_time.into(),
                data.timestamp.into(),
                data.model.into(),
                data.env.into(),
                data.err.into(),
            ])
            .map_err(map_db_err)?
            .build_sqlx(PostgresQueryBuilder);

        let result = self
            .conn
            .execute(sql.as_str(), sql_values)
            .map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }

    fn update(&self, data: &Self::Item) -> Result<bool> {
        let model = data.clone();
        let (sql, sql_values) = SeaQuery::update()
            .table(CollectionIden::Table)
            .values([
                (CollectionIden::State, model.state.into()),
                (CollectionIden::Mid, model.mid.into()),
                (CollectionIden::Name, model.name.into()),
                (CollectionIden::StartTime, model.start_time.into()),
                (CollectionIden::EndTime, model.end_time.into()),
                (CollectionIden::Timestamp, model.timestamp.into()),
                (CollectionIden::Model, model.model.into()),
                (CollectionIden::Env, model.env.into()),
                (CollectionIden::Err, model.err.into()),
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(data.id()))
            .build_sqlx(PostgresQueryBuilder);

        let result = self
            .conn
            .execute(sql.as_str(), sql_values)
            .map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        let (sql, values) = SeaQuery::delete()
            .from_table(CollectionIden::Table)
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_sqlx(PostgresQueryBuilder);

        let result = self
            .conn
            .execute(sql.as_str(), values)
            .map_err(map_db_err)?;
        Ok(result.rows_affected() > 0)
    }
}

impl DbRow for data::Proc {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get("id"),
            state: row.get("state"),
            mid: row.get("mid"),
            name: row.get("name"),
            model: row.get("model"),
            env: row.get("name"),
            err: row.get("err"),
            start_time: row.get("start_time"),
            end_time: row.get("end_time"),
            timestamp: row.get("timestamp"),
        })
    }
}

impl DbInit for ProcCollection {
    fn init(&self) {
        let sql = [
            Table::create()
                .table(CollectionIden::Table)
                .if_not_exists()
                .col(
                    ColumnDef::new(CollectionIden::Id)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(CollectionIden::State).string().not_null())
                .col(ColumnDef::new(CollectionIden::Mid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Name).string())
                .col(
                    ColumnDef::new(CollectionIden::StartTime)
                        .big_integer()
                        .default(0),
                )
                .col(
                    ColumnDef::new(CollectionIden::EndTime)
                        .big_integer()
                        .default(0),
                )
                .col(ColumnDef::new(CollectionIden::Model).string().not_null())
                .col(ColumnDef::new(CollectionIden::Env).string().not_null())
                .col(ColumnDef::new(CollectionIden::Err).string())
                .col(
                    ColumnDef::new(CollectionIden::Timestamp)
                        .big_integer()
                        .default(0),
                )
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_procs_state")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::State)
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_procs_mid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Mid)
                .build(PostgresQueryBuilder),
        ];
        self.conn.batch_execute(&sql).unwrap();
    }
}

impl ProcCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
