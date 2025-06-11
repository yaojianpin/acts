use crate::database::{DbInit, DbRow};
use acts::{DbCollection, PageData, Result, data};
use sea_query::{
    Alias as SeaAlias, ColumnDef, Expr as SeaExpr, Func as SeaFunc, Iden, Order as SeaOrder,
    PostgresQueryBuilder, Query as SeaQuery, Table,
};
use sea_query_binder::SqlxBinder;
use sqlx::{Error as DbError, Row, postgres::PgRow};

use super::{DbConnection, into_query, map_db_err};

#[derive(Debug)]
pub struct EventCollection {
    conn: DbConnection,
}

#[derive(Iden)]
#[iden = "events"]
enum CollectionIden {
    Table,
    Id,
    Name,
    Mid,
    Ver,
    Uses,
    Params,
    CreateTime,
    Timestamp,
}

impl DbCollection for EventCollection {
    type Item = data::Event;

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
                CollectionIden::Name,
                CollectionIden::Mid,
                CollectionIden::Ver,
                CollectionIden::Uses,
                CollectionIden::Params,
                CollectionIden::CreateTime,
                CollectionIden::Timestamp,
            ])
            .and_where(SeaExpr::col(CollectionIden::Id).eq(id))
            .build_sqlx(PostgresQueryBuilder);

        self.conn
            .query_one(sql.as_str(), values)
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
                CollectionIden::Name,
                CollectionIden::Mid,
                CollectionIden::Ver,
                CollectionIden::Uses,
                CollectionIden::Params,
                CollectionIden::CreateTime,
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
                CollectionIden::Name,
                CollectionIden::Mid,
                CollectionIden::Ver,
                CollectionIden::Uses,
                CollectionIden::Params,
                CollectionIden::CreateTime,
                CollectionIden::Timestamp,
            ])
            .values([
                data.id.into(),
                data.name.into(),
                data.mid.into(),
                data.create_time.into(),
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
                (CollectionIden::Name, model.name.into()),
                (CollectionIden::Mid, model.mid.into()),
                (CollectionIden::Ver, model.ver.into()),
                (CollectionIden::Uses, model.uses.into()),
                (CollectionIden::Params, model.params.into()),
                (CollectionIden::CreateTime, model.create_time.into()),
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

impl DbRow for data::Event {
    fn id(&self) -> &str {
        &self.id
    }

    fn from_row(row: &PgRow) -> std::result::Result<Self, DbError>
    where
        Self: Sized,
    {
        Ok(Self {
            id: row.get("id"),
            name: row.get("name"),
            mid: row.get("mid"),
            ver: row.get("ver"),
            uses: row.get("uses"),
            params: row.get("params"),
            create_time: row.get("create_time"),
            timestamp: row.get("timestamp"),
        })
    }
}

impl DbInit for EventCollection {
    fn init(&self) {
        let sql = [Table::create()
            .table(CollectionIden::Table)
            .if_not_exists()
            .col(
                ColumnDef::new(CollectionIden::Id)
                    .string()
                    .not_null()
                    .primary_key(),
            )
            .col(ColumnDef::new(CollectionIden::Name).string().not_null())
            .col(ColumnDef::new(CollectionIden::Mid).string().not_null())
            .col(ColumnDef::new(CollectionIden::Ver).integer().not_null())
            .col(ColumnDef::new(CollectionIden::Uses).string().not_null())
            .col(ColumnDef::new(CollectionIden::Params).string())
            .col(
                ColumnDef::new(CollectionIden::CreateTime)
                    .big_integer()
                    .default(0),
            )
            .col(
                ColumnDef::new(CollectionIden::Timestamp)
                    .big_integer()
                    .default(0),
            )
            .build(PostgresQueryBuilder)];

        self.conn.batch_execute(&sql).unwrap();
    }
}

impl EventCollection {
    pub fn new(conn: &DbConnection) -> Self {
        Self { conn: conn.clone() }
    }
}
