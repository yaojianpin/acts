use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use rusqlite::{Error as DbError, Result as DbResult, Row};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Index, Order as SeaOrder,
    Query as SeaQuery, SqliteQueryBuilder, Table,
};
use sea_query_rusqlite::RusqliteBinder;
use std::str::FromStr;

use super::{DbConnection, into_query, map_db_err};

#[derive(Debug)]
pub struct MessageCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "messages"]
enum CollectionIden {
    Table,

    Id,
    Tid,
    Name,
    State,
    Type,
    Model,
    Pid,
    Nid,
    Mid,
    Key,
    Uses,
    Inputs,
    Outputs,
    Tag,
    StartTime,
    EndTime,
    ChanId,
    ChanPattern,
    CreateTime,
    UpdateTime,
    RetryTimes,
    Status,
    Timestamp,
}

impl DbCollection for MessageCollection {
    type Item = data::Message;

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
                CollectionIden::Tid,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Type,
                CollectionIden::Model,
                CollectionIden::Pid,
                CollectionIden::Nid,
                CollectionIden::Mid,
                CollectionIden::Key,
                CollectionIden::Uses,
                CollectionIden::Inputs,
                CollectionIden::Outputs,
                CollectionIden::Tag,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::ChanId,
                CollectionIden::ChanPattern,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::RetryTimes,
                CollectionIden::Status,
                CollectionIden::Timestamp,
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_rusqlite(SqliteQueryBuilder);

        let mut stmt = conn.prepare(sql.as_str()).map_err(map_db_err)?;
        let row = stmt
            .query_row(&*values.as_params(), Self::Item::from_row)
            .map_err(map_db_err)?;

        Ok(row)
    }

    fn query(&self, q: &acts::query::Query) -> Result<acts::PageData<Self::Item>> {
        let conn = self.conn.get().unwrap();
        let filter = into_query(q);

        let mut count_query = SeaQuery::select();
        count_query
            .from(CollectionIden::Table)
            .expr(SeaFunc::count(SeaExpr::col(CollectionIden::Id)));

        let mut query = SeaQuery::select();
        query
            .columns([
                CollectionIden::Id,
                CollectionIden::Tid,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Type,
                CollectionIden::Model,
                CollectionIden::Pid,
                CollectionIden::Nid,
                CollectionIden::Mid,
                CollectionIden::Key,
                CollectionIden::Uses,
                CollectionIden::Inputs,
                CollectionIden::Outputs,
                CollectionIden::Tag,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::ChanId,
                CollectionIden::ChanPattern,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::RetryTimes,
                CollectionIden::Status,
                CollectionIden::Timestamp,
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
                .prepare(&sql)
                .map_err(map_db_err)?
                .query_map(&*values.as_params(), Self::Item::from_row)
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
                CollectionIden::Tid,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Type,
                CollectionIden::Model,
                CollectionIden::Pid,
                CollectionIden::Nid,
                CollectionIden::Mid,
                CollectionIden::Key,
                CollectionIden::Uses,
                CollectionIden::Inputs,
                CollectionIden::Outputs,
                CollectionIden::Tag,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::ChanId,
                CollectionIden::ChanPattern,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::RetryTimes,
                CollectionIden::Status,
                CollectionIden::Timestamp,
            ])
            .values([
                data.id.into(),
                data.tid.into(),
                data.name.into(),
                data.state.as_ref().into(),
                data.r#type.into(),
                data.model.into(),
                data.pid.into(),
                data.nid.into(),
                data.mid.into(),
                data.key.into(),
                data.uses.into(),
                data.inputs.into(),
                data.outputs.into(),
                data.tag.into(),
                data.start_time.into(),
                data.end_time.into(),
                data.chan_id.into(),
                data.chan_pattern.into(),
                data.create_time.into(),
                data.update_time.into(),
                data.retry_times.into(),
                Into::<i8>::into(data.status).into(),
                data.timestamp.into(),
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
                (CollectionIden::Tid, model.tid.into()),
                (CollectionIden::Name, model.name.into()),
                (CollectionIden::State, model.state.as_ref().into()),
                (CollectionIden::Type, model.r#type.into()),
                (CollectionIden::Model, model.model.into()),
                (CollectionIden::Pid, model.pid.into()),
                (CollectionIden::Nid, model.nid.into()),
                (CollectionIden::Mid, model.mid.into()),
                (CollectionIden::Key, model.key.into()),
                (CollectionIden::Uses, model.uses.into()),
                (CollectionIden::Inputs, model.inputs.into()),
                (CollectionIden::Outputs, model.outputs.into()),
                (CollectionIden::Tag, model.tag.into()),
                (CollectionIden::StartTime, model.start_time.into()),
                (CollectionIden::EndTime, model.end_time.into()),
                (CollectionIden::ChanId, model.chan_id.into()),
                (CollectionIden::ChanPattern, model.chan_pattern.into()),
                (CollectionIden::CreateTime, model.create_time.into()),
                (CollectionIden::UpdateTime, model.update_time.into()),
                (CollectionIden::RetryTimes, model.retry_times.into()),
                (
                    CollectionIden::Status,
                    Into::<i8>::into(model.status).into(),
                ),
                (CollectionIden::Timestamp, model.timestamp.into()),
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

impl DbRow for data::Message {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get_unwrap("id"),
            tid: row.get_unwrap("tid"),
            name: row.get_unwrap("name"),
            state: acts::MessageState::from_str(&row.get_unwrap::<&str, String>("state")).unwrap(),
            r#type: row.get_unwrap("type"),
            model: row.get_unwrap("model"),
            pid: row.get_unwrap("pid"),
            nid: row.get_unwrap("nid"),
            mid: row.get_unwrap("mid"),
            key: row.get_unwrap("key"),
            uses: row.get_unwrap("uses"),
            inputs: row.get_unwrap("inputs"),
            outputs: row.get_unwrap("outputs"),
            tag: row.get_unwrap("tag"),
            start_time: row.get_unwrap("start_time"),
            end_time: row.get_unwrap("end_time"),
            chan_id: row.get_unwrap("chan_id"),
            chan_pattern: row.get_unwrap("chan_pattern"),
            create_time: row.get_unwrap("timestamp"),
            update_time: row.get_unwrap("create_time"),
            retry_times: row.get_unwrap("retry_times"),
            status: row.get_unwrap::<&str, i8>("status").into(),
            timestamp: row.get_unwrap("timestamp"),
        })
    }
}

impl DbInit for MessageCollection {
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
                .col(ColumnDef::new(CollectionIden::Tid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Name).string())
                .col(ColumnDef::new(CollectionIden::State).string().not_null())
                .col(ColumnDef::new(CollectionIden::Type).string().not_null())
                .col(ColumnDef::new(CollectionIden::Model).string().not_null())
                .col(ColumnDef::new(CollectionIden::Pid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Nid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Mid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Key).string())
                .col(ColumnDef::new(CollectionIden::Uses).string())
                .col(ColumnDef::new(CollectionIden::Inputs).string())
                .col(ColumnDef::new(CollectionIden::Outputs).string())
                .col(ColumnDef::new(CollectionIden::Tag).string())
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
                .col(ColumnDef::new(CollectionIden::ChanId).string())
                .col(ColumnDef::new(CollectionIden::ChanPattern).string())
                .col(
                    ColumnDef::new(CollectionIden::CreateTime)
                        .big_integer()
                        .default(0),
                )
                .col(
                    ColumnDef::new(CollectionIden::UpdateTime)
                        .big_integer()
                        .default(0),
                )
                .col(
                    ColumnDef::new(CollectionIden::RetryTimes)
                        .integer()
                        .default(0),
                )
                .col(ColumnDef::new(CollectionIden::Status).integer().default(0))
                .col(
                    ColumnDef::new(CollectionIden::Timestamp)
                        .big_integer()
                        .default(0),
                )
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_messages_mid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Mid)
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_messages_nid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Nid)
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_messages_pid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Pid)
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_messages_status")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Status)
                .build(SqliteQueryBuilder),
        ]
        .join("; ");
        let conn = self.conn.get().unwrap();
        conn.execute_batch(&sql).unwrap();
    }
}

impl MessageCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
