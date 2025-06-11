use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Index, Order as SeaOrder,
    PostgresQueryBuilder, Query as SeaQuery, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Error as DbError, Row, postgres::PgRow};

use super::{DbConnection, into_query, map_db_err};

pub struct TaskCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "tasks"]
enum CollectionIden {
    Table,

    Id,
    Pid,
    Tid,
    NodeData,
    Kind,
    Prev,

    Name,
    State,
    Data,
    Err,
    StartTime,
    EndTime,
    Hooks,
    Timestamp,
}

impl DbCollection for TaskCollection {
    type Item = data::Task;

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
                CollectionIden::Pid,
                CollectionIden::Tid,
                CollectionIden::NodeData,
                CollectionIden::Kind,
                CollectionIden::Prev,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Data,
                CollectionIden::Err,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Hooks,
                CollectionIden::Timestamp,
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
                CollectionIden::Pid,
                CollectionIden::Tid,
                CollectionIden::NodeData,
                CollectionIden::Kind,
                CollectionIden::Prev,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Data,
                CollectionIden::Err,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Hooks,
                CollectionIden::Timestamp,
            ])
            .from(CollectionIden::Table);

        if !filter.is_empty() {
            count_query.cond_where(filter.clone());
            query.cond_where(filter);
        }

        if !q.get_order_by().is_empty() {
            for ob in q.get_order_by().iter() {
                query.order_by(
                    SeaAlias::new(ob.field.clone()),
                    match ob.order {
                        acts::query::Sort::Asc => SeaOrder::Asc,
                        acts::query::Sort::Desc => SeaOrder::Desc,
                    },
                );
            }
        }
        let (sql, values) = query
            .limit(q.limit as u64)
            .offset(q.offset as u64)
            .build_sqlx(PostgresQueryBuilder);

        let (count_sql, count_values) = count_query.build_sqlx(PostgresQueryBuilder);
        let count = self
            .conn
            .query_one(count_sql.as_str(), count_values)
            .map_err(map_db_err)?
            .get::<i64, usize>(0) as usize;
        let page_count = count.div_ceil(q.limit);
        let page_num = q.offset / q.limit + 1;
        let data = PageData {
            count,
            page_size: q.limit,
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
                CollectionIden::Pid,
                CollectionIden::Tid,
                CollectionIden::NodeData,
                CollectionIden::Kind,
                CollectionIden::Prev,
                CollectionIden::Name,
                CollectionIden::State,
                CollectionIden::Data,
                CollectionIden::Err,
                CollectionIden::StartTime,
                CollectionIden::EndTime,
                CollectionIden::Hooks,
                CollectionIden::Timestamp,
            ])
            .values([
                data.id.into(),
                data.pid.into(),
                data.tid.into(),
                data.node_data.into(),
                data.kind.into(),
                data.prev.into(),
                data.name.into(),
                data.state.into(),
                data.data.into(),
                data.err.into(),
                data.start_time.into(),
                data.end_time.into(),
                data.hooks.into(),
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
                (CollectionIden::Pid, model.pid.into()),
                (CollectionIden::Tid, model.tid.into()),
                (CollectionIden::NodeData, model.node_data.into()),
                (CollectionIden::Kind, model.kind.into()),
                (CollectionIden::Prev, model.prev.into()),
                (CollectionIden::Name, model.name.into()),
                (CollectionIden::State, model.state.into()),
                (CollectionIden::Data, model.data.into()),
                (CollectionIden::Err, model.err.into()),
                (CollectionIden::StartTime, model.start_time.into()),
                (CollectionIden::EndTime, model.end_time.into()),
                (CollectionIden::Hooks, model.hooks.into()),
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

impl DbRow for data::Task {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get("id"),
            pid: row.get("pid"),
            tid: row.get("tid"),
            node_data: row.get("node_data"),
            kind: row.get("kind"),
            prev: row.get("prev"),
            name: row.get("name"),
            state: row.get("state"),
            data: row.get("data"),
            err: row.get("err"),
            start_time: row.get("start_time"),
            end_time: row.get("end_time"),
            hooks: row.get("hooks"),
            timestamp: row.get("timestamp"),
        })
    }
}

impl DbInit for TaskCollection {
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
                .col(ColumnDef::new(CollectionIden::Pid).string().not_null())
                .col(ColumnDef::new(CollectionIden::Tid).string().not_null())
                .col(ColumnDef::new(CollectionIden::NodeData).string())
                .col(ColumnDef::new(CollectionIden::Kind).string().not_null())
                .col(ColumnDef::new(CollectionIden::Prev).string())
                .col(ColumnDef::new(CollectionIden::Name).string())
                .col(ColumnDef::new(CollectionIden::State).string().not_null())
                .col(ColumnDef::new(CollectionIden::Data).string())
                .col(ColumnDef::new(CollectionIden::Err).string())
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
                .col(
                    ColumnDef::new(CollectionIden::Timestamp)
                        .big_integer()
                        .default(0),
                )
                .col(ColumnDef::new(CollectionIden::Hooks).string())
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_tasks_pid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Pid)
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_tasks_tid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Tid)
                .build(PostgresQueryBuilder),
        ];

        self.conn.batch_execute(&sql).unwrap();
    }
}

impl TaskCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
