use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use rusqlite::{Error as DbError, Result as DbResult, Row};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Index, Order as SeaOrder,
    Query as SeaQuery, SqliteQueryBuilder, Table,
};
use sea_query_rusqlite::RusqliteBinder;

use super::{DbConnection, into_query, map_db_err};

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
    EnvLocal,
    Err,
}

impl DbCollection for ProcCollection {
    type Item = data::Proc;

    fn exists(&self, id: &str) -> Result<bool> {
        let conn = self.conn.get().unwrap();
        let (sql, values) = SeaQuery::select()
            .from(CollectionIden::Table)
            .expr(SeaFunc::count(SeaExpr::col(CollectionIden::Id)))
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_rusqlite(SqliteQueryBuilder);

        let mut stmt = conn.prepare(sql.as_str()).map_err(map_db_err)?;
        let result = stmt
            .query_row(&*values.as_params(), |row| row.get::<usize, i64>(0))
            .map_err(map_db_err)?;

        Ok(result > 0)
    }

    fn find(&self, id: &str) -> Result<Self::Item> {
        let conn = self.conn.get().unwrap();
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
                CollectionIden::EnvLocal,
                CollectionIden::Err,
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_rusqlite(SqliteQueryBuilder);

        let mut stmt = conn.prepare(sql.as_str()).map_err(map_db_err)?;
        let row = stmt
            .query_row(&*values.as_params(), |row| data::Proc::from_row(row))
            .map_err(map_db_err)?;

        Ok(row)
    }

    fn query(&self, q: &acts::query::Query) -> Result<acts::PageData<Self::Item>> {
        let conn = self.conn.get().unwrap();
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
                CollectionIden::EnvLocal,
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
            .build_rusqlite(SqliteQueryBuilder);

        let (count_sql, count_values) = count_query.build_rusqlite(SqliteQueryBuilder);
        let count = conn
            .prepare(count_sql.as_str())
            .map_err(map_db_err)?
            .query_row::<usize, _, _>(&*count_values.as_params(), |row| row.get(0))
            .map_err(map_db_err)?;
        let page_count = count.div_ceil(q.limit());
        let page_num = q.offset() / q.limit() + 1;
        let data = PageData {
            count,
            page_size: q.limit(),
            page_num,
            page_count,
            rows: conn
                .prepare(&sql.as_str())
                .map_err(map_db_err)?
                .query_map(&*values.as_params(), |row| data::Proc::from_row(row))
                .map_err(map_db_err)?
                .map(|v| v.unwrap())
                .collect::<Vec<_>>(),
        };
        Ok(data)
    }

    fn create(&self, data: &Self::Item) -> Result<bool> {
        let conn = self.conn.get().unwrap();
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
                CollectionIden::EnvLocal,
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
                data.env_local.into(),
                data.err.into(),
            ])
            .map_err(map_db_err)?
            .build_rusqlite(SqliteQueryBuilder);

        let result = conn
            .execute(sql.as_str(), &*sql_values.as_params())
            .map_err(map_db_err)?;
        Ok(result > 0)
    }

    fn update(&self, data: &Self::Item) -> Result<bool> {
        let conn = self.conn.get().unwrap();
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
                (CollectionIden::EnvLocal, model.env_local.into()),
                (CollectionIden::Err, model.err.into()),
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(data.id()))
            .build_rusqlite(SqliteQueryBuilder);

        let result = conn
            .execute(sql.as_str(), &*sql_values.as_params())
            .map_err(map_db_err)?;
        Ok(result > 0)
    }

    fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.get().unwrap();
        let (sql, values) = SeaQuery::delete()
            .from_table(CollectionIden::Table)
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_rusqlite(SqliteQueryBuilder);

        let result = conn
            .execute(sql.as_str(), &*values.as_params())
            .map_err(map_db_err)?;
        Ok(result > 0)
    }
}

impl DbRow for data::Proc {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get_unwrap("id"),
            state: row.get_unwrap("state"),
            mid: row.get_unwrap("mid"),
            name: row.get_unwrap("name"),
            model: row.get_unwrap("model"),
            env_local: row.get_unwrap("name"),
            err: row.get_unwrap("err"),
            start_time: row.get_unwrap("start_time"),
            end_time: row.get_unwrap("end_time"),
            timestamp: row.get_unwrap("timestamp"),
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
                        .integer()
                        .default(0),
                )
                .col(ColumnDef::new(CollectionIden::EndTime).integer().default(0))
                .col(ColumnDef::new(CollectionIden::Model).string().not_null())
                .col(ColumnDef::new(CollectionIden::EnvLocal).string().not_null())
                .col(ColumnDef::new(CollectionIden::Err).string())
                .col(
                    ColumnDef::new(CollectionIden::Timestamp)
                        .integer()
                        .default(0),
                )
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_procs_state")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::State)
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_procs_mid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Mid)
                .build(SqliteQueryBuilder),
        ]
        .join("; ");
        let conn = self.conn.get().unwrap();
        conn.execute_batch(&sql).unwrap();
    }
}

impl ProcCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
