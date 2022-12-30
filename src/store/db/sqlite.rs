use crate::{
    debug,
    store::{data::*, DataSet, Query},
    ActError, ActResult, StoreAdapter,
};
use once_cell::sync::OnceCell;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Row, SqlitePool};
use std::{fs::File, future::Future, path::Path, sync::Arc};

const DATABASE_PATH: &str = "data/data.db";
static DB: OnceCell<SqlitePool> = OnceCell::new();

fn db<'a>() -> &'static SqlitePool {
    let r = || run(async { init().await });
    DB.get_or_init(r)
}

fn run<F: Future + Send>(f: F) -> F::Output {
    let ret = futures::executor::block_on(f);
    ret
}

async fn init() -> SqlitePool {
    if !Path::new("data").exists() {
        std::fs::create_dir("data").unwrap();
    }
    if !Path::new(DATABASE_PATH).exists() {
        File::create(DATABASE_PATH).unwrap();
    }
    let opt = SqlitePoolOptions::new().max_connections(100);
    let pool = opt.connect(&format!("sqlite://{}", DATABASE_PATH)).await;
    match pool {
        Ok(p) => {
            let sql = include_str!("init.sql");
            p.execute(sql).await.expect("sqlite: exec init.sql");
            // DB.set(p.clone()).expect("sqlite: sqlite db set");

            p
        }
        Err(err) => {
            panic!("{}", err);
        }
    }
}

#[derive(Debug)]
pub struct SqliteStore {
    procs: Arc<ProcSet>,
    tasks: Arc<TaskSet>,
    messages: Arc<MessageSet>,
}

impl SqliteStore {
    #[allow(unused)]
    pub fn new() -> Self {
        let db = Self {
            procs: Arc::new(ProcSet),
            tasks: Arc::new(TaskSet),
            messages: Arc::new(MessageSet),
        };

        db.init();
        db
    }

    fn init(&self) {
        let _ = db();
    }
}

impl StoreAdapter for SqliteStore {
    fn init(&self) {}
    fn flush(&self) {}

    fn procs(&self) -> Arc<dyn DataSet<Proc>> {
        self.procs.clone()
    }

    fn tasks(&self) -> Arc<dyn DataSet<Task>> {
        self.tasks.clone()
    }

    fn messages(&self) -> Arc<dyn DataSet<Message>> {
        self.messages.clone()
    }
}

#[derive(Debug, Clone)]
pub struct ProcSet;

impl DataSet<Proc> for ProcSet {
    fn exists(&self, id: &str) -> bool {
        debug!("sqlite.proc.exists({})", id);
        let pool = db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_proc where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            count > 0
        })
    }

    fn find(&self, id: &str) -> Option<Proc> {
        debug!("sqlite.proc.find({})", id);
        run(async {
            let pool = db();
            match sqlx::query(r#"select id, pid, state, model, vars from act_proc where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
            {
                Ok(row) => {
                    let state: &str = row.get(2);
                    Some(Proc {
                        id: row.get(0),
                        pid: row.get(1),
                        state: state.into(),
                        model: row.get(3),
                        vars: row.get(4),
                    })
                }
                Err(_) => None,
            }
        })
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Proc>> {
        debug!("sqlite.proc.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = db();
            let sql = format!(
                r#"select id, pid, state, model, vars from act_proc {}"#,
                q.sql()
            );
            let query = sqlx::query(&sql);
            match &query.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        let state: &str = row.get(2);
                        ret.push(Proc {
                            id: row.get(0),
                            pid: row.get(1),
                            state: state.into(),
                            model: row.get(3),
                            vars: row.get(4),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }

    fn create(&self, proc: &Proc) -> ActResult<bool> {
        debug!("sqlite.proc.create({})", proc.id);
        let proc = proc.clone();
        run(async move {
            let pool = db();
            let sql = sqlx::query(
                r#"insert into act_proc (id, pid, state, model, vars) values ($1,$2,$3,$4,$5)"#,
            )
            .bind(proc.id)
            .bind(proc.pid)
            .bind(proc.state.to_string())
            .bind(proc.model)
            .bind(proc.vars);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn update(&self, proc: &Proc) -> ActResult<bool> {
        debug!("sqlite.proc.update({})", proc.id);
        run(async {
            let pool = db();
            let sql = sqlx::query(r#"update act_proc set state = $1, vars = $2 where id=$3"#)
                .bind(proc.state.to_string())
                .bind(&proc.vars)
                .bind(&proc.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        debug!("sqlite.proc.delete({})", id);
        run(async {
            let pool = db();
            let sql = sqlx::query(r#"delete from act_proc where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct TaskSet;

impl DataSet<Task> for TaskSet {
    fn exists(&self, id: &str) -> bool {
        debug!("sqlite.task.exists({})", id);
        let pool = db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_task where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            count > 0
        })
    }
    fn find(&self, id: &str) -> Option<Task> {
        debug!("sqlite.task.find({})", id);
        run(async {
            let pool = db();
            match sqlx::query(r#"select tag, id, pid, tid, state,start_time, end_time, user from act_task where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
            {
                Ok(row) => {
                    let tag: &str = row.get(0);
                    let state: &str = row.get(4);
                    Some(Task {
                        tag: tag.into(),
                        id: row.get(1),
                        pid: row.get(2),
                        tid: row.get(3),
                        state: state.into(),
                        start_time: row.get(5),
                        end_time: row.get(6),
                        user: row.get(7),
                    })
                }
                Err(_) => None,
            }
        })
    }
    fn query(&self, q: &Query) -> ActResult<Vec<Task>> {
        debug!("sqlite.task.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = db();

            let a = &format!(
                r#"select tag, id, pid, tid, state, start_time, end_time, user from act_task {}"#,
                q.sql()
            );
            println!("{}", a);
            let sql = sqlx::query(&a);
            match &sql.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        let tag: &str = row.get(0);
                        let state: &str = row.get(4);
                        ret.push(Task {
                            tag: tag.into(),
                            id: row.get(1),
                            pid: row.get(2),
                            tid: row.get(3),
                            state: state.into(),
                            start_time: row.get(5),
                            end_time: row.get(6),
                            user: row.get(7),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }

    fn create(&self, task: &Task) -> ActResult<bool> {
        debug!("sqlite.task.create({})", task.id);
        let task = task.clone();
        run(async move {
            let pool = &*db();

            let tag: &str = task.tag.into();
            let sql = sqlx::query(
                r#"insert into act_task (tag, id, pid, tid, state, start_time, end_time, user) values ($1,$2,$3,$4,$5,$6,$7,$8)"#,
            )
            .bind(tag)
            .bind(task.id)
            .bind(task.pid)
            .bind(task.tid)
            .bind(task.state.to_string())
            .bind(task.start_time)
            .bind(task.end_time)
            .bind(task.user.clone());

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn update(&self, task: &Task) -> ActResult<bool> {
        debug!("sqlite.task.update({})", task.id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"update act_task set state = $1, start_time = $2, end_time = $3, user = $4 where id=$5"#)
                .bind(task.state.to_string())
                .bind(task.start_time)
                .bind(task.end_time)
                .bind(task.user.clone())
                .bind(&task.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        debug!("sqlite.task.delete({})", id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"delete from act_task where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct MessageSet;

impl DataSet<Message> for MessageSet {
    fn exists(&self, id: &str) -> bool {
        debug!("sqlite.message.exists({})", id);
        let pool = &*db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_message where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            count > 0
        })
    }

    fn find(&self, id: &str) -> Option<Message> {
        debug!("sqlite.message.find({})", id);
        run(async {
            let pool = &*db();
            match sqlx::query(
                r#"select id, pid, tid, user, create_time from act_message where id=$1"#,
            )
            .bind(id)
            .fetch_one(pool)
            .await
            {
                Ok(row) => Some(Message {
                    id: row.get(0),
                    pid: row.get(1),
                    tid: row.get(2),
                    user: row.get(3),
                    create_time: row.get(4),
                }),
                Err(_) => None,
            }
        })
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Message>> {
        debug!("sqlite.message.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = &*db();

            let a = &format!(
                r#"select id, pid, tid, user, create_time from act_message {}"#,
                q.sql()
            );
            println!("{}", a);
            let sql = sqlx::query(&a);
            match &sql.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        ret.push(Message {
                            id: row.get(0),
                            pid: row.get(1),
                            tid: row.get(2),
                            user: row.get(3),
                            create_time: row.get(4),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }

    fn create(&self, msg: &Message) -> ActResult<bool> {
        debug!("sqlite.message.create({})", msg.id);
        let msg = msg.clone();
        run(async move {
            let pool = &*db();
            let sql = sqlx::query(
                r#"insert into act_message (id, pid, tid, user, create_time) values ($1,$2,$3,$4,$5)"#,
            )
            .bind(msg.id)
            .bind(msg.pid)
            .bind(msg.tid)
            .bind(msg.user)
            .bind(msg.create_time);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn update(&self, msg: &Message) -> ActResult<bool> {
        debug!("sqlite.message.update({})", msg.id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"update act_message set user = $1 where id=$2"#)
                .bind(&msg.user)
                .bind(&msg.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        debug!("sqlite.message:delete({})", id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"delete from act_message where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::StoreError(err.to_string())),
            }
        })
    }
}
