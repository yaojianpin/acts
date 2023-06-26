use crate::{
    store::{data::*, DbSet, Query, StoreAdapter},
    ActError, ActResult,
};
use once_cell::sync::OnceCell;
use sqlx::{sqlite::SqlitePoolOptions, Executor, Row, SqlitePool};
use std::{fs::File, future::Future, path::Path, sync::Arc};
use tracing::trace;

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
    models: Arc<ModelSet>,
    procs: Arc<ProcSet>,
    tasks: Arc<TaskSet>,
    messages: Arc<ActSet>,
}

impl SqliteStore {
    #[allow(unused)]
    pub fn new() -> Self {
        let db = Self {
            models: Arc::new(ModelSet),
            procs: Arc::new(ProcSet),
            tasks: Arc::new(TaskSet),
            messages: Arc::new(ActSet),
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

    fn models(&self) -> Arc<dyn DbSet<Item = Model>> {
        self.models.clone()
    }

    fn procs(&self) -> Arc<dyn DbSet<Item = Proc>> {
        self.procs.clone()
    }

    fn tasks(&self) -> Arc<dyn DbSet<Item = Task>> {
        self.tasks.clone()
    }

    fn acts(&self) -> Arc<dyn DbSet<Item = Act>> {
        self.messages.clone()
    }
}

#[derive(Debug, Clone)]
pub struct ModelSet;

impl DbSet for ModelSet {
    type Item = Model;
    fn exists(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.model.exists({})", id);
        let pool = db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_model where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            Ok(count > 0)
        })
    }

    fn find(&self, id: &str) -> ActResult<Model> {
        trace!("sqlite.Model.find({})", id);
        run(async {
            let pool = db();
            match sqlx::query(
                r#"select id, name, ver, size, time, model, topic from act_model where id=$1"#,
            )
            .bind(id)
            .fetch_one(pool)
            .await
            {
                Ok(row) => Ok(Model {
                    id: row.get(0),
                    name: row.get(1),
                    ver: row.get(2),
                    size: row.get(3),
                    time: row.get(4),
                    model: row.get(5),
                    topic: row.get(6),
                }),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Model>> {
        trace!("sqlite.Model.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = db();
            let sql = format!(
                r#"select id, name, ver, size, time, model, topic from act_model {}"#,
                q.sql()
            );
            let query = sqlx::query(&sql);
            match &query.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        ret.push(Model {
                            id: row.get(0),
                            name: row.get(1),
                            ver: row.get(2),
                            size: row.get(3),
                            time: row.get(4),
                            model: row.get(5),
                            topic: row.get(6),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn create(&self, model: &Model) -> ActResult<bool> {
        trace!("sqlite.Model.create({})", model.id);
        let model = model.clone();
        run(async move {
            let pool = db();
            let sql = sqlx::query(
                r#"insert into act_model (id, model, ver, size, time, topic) values ($1,$2,$3,$4,$5,$6)"#,
            )
            .bind(model.id)
            .bind(model.model)
            .bind(model.ver)
            .bind(model.size)
            .bind(model.time)
            .bind(model.topic);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn update(&self, model: &Model) -> ActResult<bool> {
        trace!("sqlite.Model.update({})", model.id);
        run(async {
            let pool = db();
            let sql =
                sqlx::query(r#"update act_model set model = $1, ver = $2, topic = $3 where id=$4"#)
                    .bind(model.model.to_string())
                    .bind(&model.ver)
                    .bind(&model.topic)
                    .bind(&model.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.Model.delete({})", id);
        run(async {
            let pool = db();
            let sql = sqlx::query(r#"delete from act_model where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProcSet;

impl DbSet for ProcSet {
    type Item = Proc;
    fn exists(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.proc.exists({})", id);
        let pool = db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_proc where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            Ok(count > 0)
        })
    }

    fn find(&self, id: &str) -> ActResult<Proc> {
        trace!("sqlite.proc.find({})", id);
        run(async {
            let pool = db();
            match sqlx::query(r#"select id, pid, state, model, vars, start_time, end_time from act_proc where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
            {
                Ok(row) => {
                    Ok(Proc {
                        id: row.get(0),
                        pid: row.get(1),
                        state: row.get(2),
                        model: row.get(3),
                        vars: row.get(4),
                        start_time: row.get(5),
                        end_time: row.get(6),
                    })
                }
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Proc>> {
        trace!("sqlite.proc.query({})", q.sql());
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
                            start_time: row.get(5),
                            end_time: row.get(6),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn create(&self, proc: &Proc) -> ActResult<bool> {
        trace!("sqlite.proc.create({})", proc.id);
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
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn update(&self, proc: &Proc) -> ActResult<bool> {
        trace!("sqlite.proc.update({})", proc.id);
        run(async {
            let pool = db();
            let sql = sqlx::query(r#"update act_proc set state = $1, vars = $2 where id=$4"#)
                .bind(proc.state.to_string())
                .bind(&proc.vars)
                .bind(&proc.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.proc.delete({})", id);
        run(async {
            let pool = db();
            let sql = sqlx::query(r#"delete from act_proc where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct TaskSet;

impl DbSet for TaskSet {
    type Item = Task;
    fn exists(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.task.exists({})", id);
        let pool = db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_task where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            Ok(count > 0)
        })
    }
    fn find(&self, id: &str) -> ActResult<Task> {
        trace!("sqlite.task.find({})", id);
        run(async {
            let pool = db();
            match sqlx::query(
                r#"select tag, id, pid, tid, state,start_time, end_time from act_task where id=$1"#,
            )
            .bind(id)
            .fetch_one(pool)
            .await
            {
                Ok(row) => Ok(Task {
                    kind: row.get(0),
                    id: row.get(1),
                    pid: row.get(2),
                    tid: row.get(3),
                    nid: row.get(4),
                    state: row.get(5),
                    start_time: row.get(6),
                    end_time: row.get(7),
                }),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn query(&self, q: &Query) -> ActResult<Vec<Task>> {
        trace!("sqlite.task.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = db();

            let a = &format!(
                r#"select kind, id, pid, tid, nid, state, start_time, end_time from act_task {}"#,
                q.sql()
            );
            println!("{}", a);
            let sql = sqlx::query(&a);
            match &sql.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        ret.push(Task {
                            kind: row.get(0),
                            id: row.get(1),
                            pid: row.get(2),
                            tid: row.get(3),
                            nid: row.get(4),
                            state: row.get(5),
                            start_time: row.get(6),
                            end_time: row.get(7),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn create(&self, task: &Task) -> ActResult<bool> {
        trace!("sqlite.task.create({})", task.id);
        let task = task.clone();
        run(async move {
            let pool = &*db();
            let sql = sqlx::query(
                r#"insert into act_task (kind, id, pid, tid, nid, state, start_time, end_time) values ($1,$2,$3,$4,$5,$6,$7,$8)"#,
            )
            .bind(task.kind)
            .bind(task.id)
            .bind(task.pid)
            .bind(task.tid)
            .bind(task.nid)
            .bind(task.state.to_string())
            .bind(task.start_time)
            .bind(task.end_time);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn update(&self, task: &Task) -> ActResult<bool> {
        trace!("sqlite.task.update({})", task.id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"update act_task set state = $1, start_time = $2, end_time = $3, kind = $4, where id=$5"#)
                .bind(task.state.to_string())
                .bind(task.start_time)
                .bind(task.end_time)
                .bind(task.kind.clone())
                .bind(&task.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.task.delete({})", id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"delete from act_task where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
}

#[derive(Debug, Clone)]
pub struct ActSet;

impl DbSet for ActSet {
    type Item = Act;
    fn exists(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.act.exists({})", id);
        let pool = &*db();
        run(async {
            let row = sqlx::query(r#"select count(id) from act_act where id=$1"#)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
            let count: i32 = row.get(0);
            Ok(count > 0)
        })
    }

    fn find(&self, id: &str) -> ActResult<Act> {
        trace!("sqlite.act.find({})", id);
        run(async {
            let pool = &*db();
            match sqlx::query(
                r#"select id, kind, event, pid, tid, create_time,update_time, state, vars, active from act_act where id=$1"#,
            )
            .bind(id)
            .fetch_one(pool)
            .await
            {
                Ok(row) => Ok(Act {
                    id: row.get(0),
                    kind: row.get(1),
                    event: row.get(2),
                    pid: row.get(3),
                    tid: row.get(4),
                    start_time: row.get(5),
                    end_time: row.get(6),
                    state: row.get(7),
                    vars: row.get(8),
                    active: row.get(9),
                }),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn query(&self, q: &Query) -> ActResult<Vec<Act>> {
        trace!("sqlite.act.query({})", q.sql());
        run(async {
            let mut ret = Vec::new();
            let pool = &*db();

            let a = &format!(
                r#"select id, kind, event, pid, tid, create_time,update_time,state, vars, active from act_act {}"#,
                q.sql()
            );
            println!("{}", a);
            let sql = sqlx::query(&a);
            match &sql.fetch_all(pool).await {
                Ok(rows) => {
                    for row in rows {
                        ret.push(Act {
                            id: row.get(0),
                            kind: row.get(1),
                            event: row.get(2),
                            pid: row.get(3),
                            tid: row.get(4),
                            start_time: row.get(5),
                            end_time: row.get(6),
                            state: row.get(7),
                            vars: row.get(8),
                            active: row.get(9),
                        });
                    }

                    Ok(ret)
                }
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }

    fn create(&self, msg: &Act) -> ActResult<bool> {
        trace!("sqlite.act.create({})", msg.id);
        let act = msg.clone();
        run(async move {
            let pool = &*db();
            let sql = sqlx::query(
                r#"insert into act_act (id, kind, event, pid, tid, create_time, vars, active) values ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#,
            )
            .bind(act.id)
            .bind(act.kind)
            .bind(act.event)
            .bind(act.pid)
            .bind(act.tid)
            .bind(act.start_time)
            .bind(act.vars)
            .bind(act.active);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn update(&self, msg: &Act) -> ActResult<bool> {
        trace!("sqlite.act.update({})", msg.id);
        run(async {
            let pool = &*db();
            let sql =
                sqlx::query(r#"update act_act set vars=$1, update_time=$2, active=$3 where id=$4"#)
                    .bind(&msg.vars)
                    .bind(&msg.end_time)
                    .bind(&msg.active)
                    .bind(&msg.id);

            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
    fn delete(&self, id: &str) -> ActResult<bool> {
        trace!("sqlite.message:delete({})", id);
        run(async {
            let pool = &*db();
            let sql = sqlx::query(r#"delete from act_act where id=$1"#).bind(id);
            match sql.execute(pool).await {
                Ok(_) => Ok(true),
                Err(err) => Err(ActError::Store(err.to_string())),
            }
        })
    }
}
