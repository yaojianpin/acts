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
    Groups,
    Catalog,
    BuiltIn,
    CreateTime,
    UpdateTime,
    Timestamp,
}

impl DbCollection for PackageCollection {
    type Item = data::Package;

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
                CollectionIden::Desc,
                CollectionIden::Icon,
                CollectionIden::Doc,
                CollectionIden::Version,
                CollectionIden::Schema,
                CollectionIden::RunAs,
                CollectionIden::Groups,
                CollectionIden::Catalog,
                CollectionIden::BuiltIn,
                CollectionIden::CreateTime,
                CollectionIden::UpdateTime,
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
                CollectionIden::Groups,
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
                CollectionIden::Desc,
                CollectionIden::Icon,
                CollectionIden::Doc,
                CollectionIden::Version,
                CollectionIden::Schema,
                CollectionIden::RunAs,
                CollectionIden::Groups,
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
                data.groups.into(),
                data.catalog.as_ref().into(),
                data.built_in.into(),
                data.create_time.into(),
                data.update_time.into(),
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
                (CollectionIden::Desc, model.desc.into()),
                (CollectionIden::Icon, model.icon.into()),
                (CollectionIden::Doc, model.doc.into()),
                (CollectionIden::Version, model.version.into()),
                (CollectionIden::Schema, model.schema.into()),
                (CollectionIden::RunAs, model.run_as.as_ref().into()),
                (CollectionIden::Groups, model.groups.into()),
                (CollectionIden::Catalog, model.catalog.as_ref().into()),
                (CollectionIden::BuiltIn, model.built_in.into()),
                (CollectionIden::CreateTime, model.create_time.into()),
                (CollectionIden::UpdateTime, model.update_time.into()),
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

impl DbRow for data::Package {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get("id"),
            desc: row.get("desc"),
            icon: row.get("icon"),
            doc: row.get("doc"),
            version: row.get("version"),
            schema: row.get("schema"),
            run_as: acts::ActRunAs::from_str(&row.get::<String, &str>("run_as")).unwrap(),
            groups: row.get("groups"),
            catalog: acts::ActPackageCatalog::from_str(&row.get::<String, &str>("catalog"))
                .unwrap(),
            built_in: row.get("built_in"),
            create_time: row.get("create_time"),
            update_time: row.get("update_time"),
            timestamp: row.get("timestamp"),
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
                .col(ColumnDef::new(CollectionIden::Groups).string().not_null())
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
                .build(PostgresQueryBuilder),
            Index::create()
                .name("idx_packages_version")
                .if_not_exists()
                .table(CollectionIden::Table)
                .col(CollectionIden::Version)
                .build(PostgresQueryBuilder),
        ];
        self.conn.batch_execute(&sql).unwrap();
    }
}

impl PackageCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
