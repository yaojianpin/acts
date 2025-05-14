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
pub struct ModelCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "models"]
enum CollectionIden {
    Table,
    Id,
    Name,
    Ver,
    Size,
    CreateTime,
    UpdateTime,
    Data,
    Timestamp,
}

impl DbCollection for ModelCollection {
    type Item = data::Model;

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
                CollectionIden::Name,
                CollectionIden::Ver,
                CollectionIden::Size,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::Data,
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
                CollectionIden::Name,
                CollectionIden::Ver,
                CollectionIden::Size,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::Data,
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
                CollectionIden::Name,
                CollectionIden::Ver,
                CollectionIden::Size,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::Data,
                CollectionIden::Timestamp,
            ])
            .values([
                data.id.into(),
                data.name.into(),
                data.ver.into(),
                data.size.into(),
                data.create_time.into(),
                data.update_time.into(),
                data.data.into(),
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
                (CollectionIden::Name, model.name.into()),
                (CollectionIden::Ver, model.ver.into()),
                (CollectionIden::Size, model.size.into()),
                (CollectionIden::CreateTime, model.create_time.into()),
                (CollectionIden::UpdateTime, model.update_time.into()),
                (CollectionIden::Data, model.data.into()),
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

impl DbRow for data::Model {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get_unwrap("id"),
            name: row.get_unwrap("name"),
            ver: row.get_unwrap("ver"),
            size: row.get_unwrap("size"),
            create_time: row.get_unwrap("create_time"),
            update_time: row.get_unwrap("update_time"),
            data: row.get_unwrap("data"),
            timestamp: row.get_unwrap("timestamp"),
        })
    }
}

impl DbInit for ModelCollection {
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
                .col(ColumnDef::new(CollectionIden::Name).string())
                .col(ColumnDef::new(CollectionIden::Ver).integer().default(0))
                .col(ColumnDef::new(CollectionIden::Size).integer().default(0))
                .col(ColumnDef::new(CollectionIden::Data).string().not_null())
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
                    ColumnDef::new(CollectionIden::Timestamp)
                        .big_integer()
                        .default(0),
                )
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_models_name")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Name)
                .build(SqliteQueryBuilder),
        ]
        .join("; ");
        let conn = self.conn.get().unwrap();
        conn.execute_batch(&sql).unwrap();
    }
}

impl ModelCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
