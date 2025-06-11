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
            .build_rusqlite(SqliteQueryBuilder);

        let (count_sql, count_values) = count_query.build_rusqlite(SqliteQueryBuilder);
        let count = conn
            .prepare(count_sql.as_str())
            .map_err(map_db_err)?
            .query_row::<usize, _, _>(&*count_values.as_params(), |row| row.get(0))
            .map_err(map_db_err)?;
        let page_count = count.div_ceil(q.limit);
        let page_num = q.offset / q.limit + 1;
        let data = PageData {
            count,
            page_size: q.limit,
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

impl DbRow for data::Task {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get_unwrap("id"),
            pid: row.get_unwrap("pid"),
            tid: row.get_unwrap("tid"),
            node_data: row.get_unwrap("node_data"),
            kind: row.get_unwrap("kind"),
            prev: row.get_unwrap("prev"),
            name: row.get_unwrap("name"),
            state: row.get_unwrap("state"),
            data: row.get_unwrap("data"),
            err: row.get_unwrap("err"),
            start_time: row.get_unwrap("start_time"),
            end_time: row.get_unwrap("end_time"),
            hooks: row.get_unwrap("hooks"),
            timestamp: row.get_unwrap("timestamp"),
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
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_tasks_pid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Pid)
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_tasks_tid")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Tid)
                .build(SqliteQueryBuilder),
        ]
        .join("; ");
        let conn = self.conn.get().unwrap();
        conn.execute_batch(&sql).unwrap();
    }
}

impl TaskCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
