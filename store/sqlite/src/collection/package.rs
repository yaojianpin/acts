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
pub struct PackageCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "packages"]
enum CollectionIden {
    Table,
    Id,
    Desc,
    Icon,
    Doc,
    Version,
    Schema,
    RunAs,
    Resources,
    Catalog,
    BuiltIn,
    CreateTime,
    UpdateTime,
    Timestamp,
}

impl DbCollection for PackageCollection {
    type Item = data::Package;

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
                CollectionIden::Desc,
                CollectionIden::Icon,
                CollectionIden::Doc,
                CollectionIden::Version,
                CollectionIden::Schema,
                CollectionIden::RunAs,
                CollectionIden::Resources,
                CollectionIden::Catalog,
                CollectionIden::BuiltIn,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
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
                CollectionIden::Desc,
                CollectionIden::Icon,
                CollectionIden::Doc,
                CollectionIden::Version,
                CollectionIden::Schema,
                CollectionIden::RunAs,
                CollectionIden::Resources,
                CollectionIden::Catalog,
                CollectionIden::BuiltIn,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
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
                CollectionIden::Desc,
                CollectionIden::Icon,
                CollectionIden::Doc,
                CollectionIden::Version,
                CollectionIden::Schema,
                CollectionIden::RunAs,
                CollectionIden::Resources,
                CollectionIden::Catalog,
                CollectionIden::BuiltIn,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
                CollectionIden::Timestamp,
            ])
            .values([
                data.id.into(),
                data.desc.into(),
                data.icon.into(),
                data.doc.into(),
                data.version.into(),
                data.schema.into(),
                data.run_as.as_ref().into(),
                data.resources.into(),
                data.catalog.as_ref().into(),
                data.built_in.into(),
                data.create_time.into(),
                data.update_time.into(),
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
                (CollectionIden::Desc, model.desc.into()),
                (CollectionIden::Icon, model.icon.into()),
                (CollectionIden::Doc, model.doc.into()),
                (CollectionIden::Version, model.version.into()),
                (CollectionIden::Schema, model.schema.into()),
                (CollectionIden::RunAs, model.run_as.as_ref().into()),
                (CollectionIden::Resources, model.resources.into()),
                (CollectionIden::Catalog, model.catalog.as_ref().into()),
                (CollectionIden::BuiltIn, model.built_in.into()),
                (CollectionIden::CreateTime, model.create_time.into()),
                (CollectionIden::UpdateTime, model.update_time.into()),
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

impl DbRow for data::Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &Row<'_>) -> DbResult<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get_unwrap("id"),
            desc: row.get_unwrap("desc"),
            icon: row.get_unwrap("icon"),
            doc: row.get_unwrap("doc"),
            version: row.get_unwrap("version"),
            schema: row.get_unwrap("schema"),
            run_as: acts::ActRunAs::from_str(&row.get_unwrap::<&str, String>("run_as")).unwrap(),
            resources: row.get_unwrap("resources"),
            catalog: acts::ActPackageCatalog::from_str(&row.get_unwrap::<&str, String>("catalog"))
                .unwrap(),
            built_in: row.get_unwrap("built_in"),
            create_time: row.get_unwrap("create_time"),
            update_time: row.get_unwrap("update_time"),
            timestamp: row.get_unwrap("timestamp"),
        })
    }
}

impl DbInit for PackageCollection {
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
                .col(ColumnDef::new(CollectionIden::Desc).string())
                .col(ColumnDef::new(CollectionIden::Icon).string().not_null())
                .col(ColumnDef::new(CollectionIden::Doc).string())
                .col(ColumnDef::new(CollectionIden::Version).string().not_null())
                .col(ColumnDef::new(CollectionIden::Schema).string().not_null())
                .col(ColumnDef::new(CollectionIden::RunAs).string().not_null())
                .col(
                    ColumnDef::new(CollectionIden::Resources)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(CollectionIden::Catalog).string().not_null())
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
                .col(
                    ColumnDef::new(CollectionIden::BuiltIn)
                        .boolean()
                        .default(false),
                )
                .build(SqliteQueryBuilder),
            Index::create()
                .name("idx_packages_version")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Version)
                .build(SqliteQueryBuilder),
        ]
        .join("; ");
        let conn = self.conn.get().unwrap();
        conn.execute_batch(&sql).unwrap();
    }
}

impl PackageCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
