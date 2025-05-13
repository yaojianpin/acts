use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Index, Order as SeaOrder,
    PostgresQueryBuilder, Query as SeaQuery, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Error as DbError, Row, postgres::PgRow};
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
            .build_sqlx(PostgresQueryBuilder);

        let row = self
            .conn
            .query_one(&sql, values)
            .map(|row| Self::Item::from_row(&row).map_err(map_db_err))
            .map_err(map_db_err)?;

        row
    }

    fn query(&self, q: &acts::query::Query) -> Result<acts::PageData<Self::Item>> {
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
                (Into::<i8>::into(data.status) as u8).into(),
                data.timestamp.into(),
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
                    (Into::<i8>::into(model.status) as i32).into(),
                ),
                (CollectionIden::Timestamp, model.timestamp.into()),
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

impl DbRow for data::Message {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get("id"),
            tid: row.get("tid"),
            name: row.get("name"),
            state: acts::MessageState::from_str(&row.get::<String, &str>("state")).unwrap(),
            r#type: row.get("type"),
            model: row.get("model"),
            pid: row.get("pid"),
            nid: row.get("nid"),
            mid: row.get("mid"),
            key: row.get("key"),
            uses: row.get("uses"),
            inputs: row.get("inputs"),
            outputs: row.get("outputs"),
            tag: row.get("tag"),
            start_time: row.get("start_time"),
            end_time: row.get("end_time"),
            chan_id: row.get("chan_id"),
            chan_pattern: row.get("chan_pattern"),
            create_time: row.get("timestamp"),
            update_time: row.get("create_time"),
            retry_times: row.get("retry_times"),
            status: (row.get::<i32, &str>("status") as i8).into(),
            timestamp: row.get("timestamp"),
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
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_messages_mid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Mid)
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_messages_nid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Nid)
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_messages_pid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Pid)
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_messages_status")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Status)
                .build(PostgresQueryBuilder),
        ];
        self.conn.batch_execute(&sql).unwrap();
    }
}

impl MessageCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
