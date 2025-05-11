use crate::{
    Act, ChannelOptions, Engine, Message, Vars, Workflow,
    data::{self, Package},
    event::{MessageState, Model},
    export::ExecutorQuery,
    scheduler::TaskState,
    store::query::*,
    utils,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn export_manager_publish_ok() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let pack = data::Package {
        id: "pack1".to_string(),
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        groups: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        ..Default::default()
    };

    let result = manager.pack().publish(&pack);

    assert!(result.is_ok());
    assert!(manager.pack().publish(&pack).is_ok());
}

#[tokio::test]
async fn export_manager_deploy_ok() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("test"))));

    let result = manager.model().deploy(&model);

    assert!(result.is_ok());
    assert!(manager.model().get(&model.id, "text").is_ok());
}

#[tokio::test]
async fn export_manager_deploy_many_times() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"));

    let mut result = true;
    for _ in 0..10 {
        let state = manager.model().deploy(&model);
        result &= state.is_ok();
    }
    assert!(result);
}

#[tokio::test]
async fn export_manager_deploy_no_model_id_error() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let result = manager.model().deploy(&model);
    assert!(result.is_err());
}

#[tokio::test]
async fn export_manager_deploy_dup_id_error() {
    let engine = Engine::new().start();
    let executor = engine.executor();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step1"));

    let result = executor.model().deploy(&model);
    assert!(result.is_err());
}

#[tokio::test]
async fn engine_executor_start_no_pid() {
    let engine = Engine::new().start();
    let executor = engine.executor();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("test"))));
    engine.executor().model().deploy(&workflow).unwrap();
    let options = Vars::new();
    let result = executor.proc().start(&workflow.id, &options);
    assert!(result.is_ok());
}

#[tokio::test]
async fn engine_executor_start_with_pid() {
    let engine = Engine::new().start();
    let executor = engine.executor();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("test"))));
    engine.executor().model().deploy(&workflow).unwrap();
    let mut options = Vars::new();
    options.insert("pid".to_string(), "123".into());
    let result = executor.proc().start(&workflow.id, &options);
    assert!(result.is_ok());

    assert_eq!(result.unwrap(), "123");
}

#[tokio::test]
async fn export_executor_start_empty_pid() {
    let engine = Engine::new().start();
    let executor = engine.executor();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("test"))));

    engine.executor().model().deploy(&workflow).unwrap();
    let mut options = Vars::new();
    options.insert("pid".to_string(), "".into());
    let result = executor.proc().start(&workflow.id, &options);
    assert!(result.is_ok());
}

#[tokio::test]
async fn export_executor_start_dup_pid_error() {
    let engine = Engine::new().start();
    let executor = engine.executor();

    let pid = utils::longid();
    let mid = utils::longid();
    let model = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::irq(|act| act.with_key("test"))));

    let store = engine.runtime().cache().store();
    let proc = data::Proc {
        id: pid.clone(),
        name: model.name.clone(),
        mid: model.id.clone(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        model: model.to_json().unwrap(),
        env_local: "{}".to_string(),
        err: None,
    };
    store.procs().create(&proc).expect("create process");
    engine
        .executor()
        .model()
        .deploy(&model)
        .expect("fail to deploy workflow");
    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(pid.to_string()));
    let result = executor.proc().start(&model.id, &options);
    assert!(result.is_err());
}

#[tokio::test]
async fn export_manager_models_get_count() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    for _ in 0..5 {
        model.set_id(&utils::longid());
        manager.model().deploy(&model).unwrap();
    }

    let result = manager
        .model()
        .list(&ExecutorQuery::new().with_offset(0).with_count(10))
        .unwrap();
    assert_eq!(result.count, 5);
}

#[tokio::test]
async fn export_manager_models_order() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    for i in 0..5 {
        model.set_id(&utils::longid());
        model.name = format!("model-{}", i + 1);
        manager.model().deploy(&model).unwrap();
    }

    let result = manager
        .model()
        .list(&ExecutorQuery::new().with_order("timestamp", true))
        .unwrap();
    assert_eq!(result.rows.first().unwrap().name, "model-5");
}

#[tokio::test]
async fn export_manager_models_get_rows() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    for _ in 0..5 {
        model.set_id(&utils::longid());
        manager.model().deploy(&model).unwrap();
    }

    let result = manager
        .model()
        .list(&ExecutorQuery::new().with_offset(4).with_count(10))
        .unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[tokio::test]
async fn export_manager_models_query() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    for i in 0..5 {
        model.set_id(&utils::longid());
        model.name = format!("model-{}", i + 1);
        manager.model().deploy(&model).unwrap();
    }

    let result = manager
        .model()
        .list(&ExecutorQuery::new().with_query("name", "model-3"))
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows.first().unwrap().name, "model-3");
}

#[tokio::test]
async fn export_manager_model_get_text() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.model().deploy(&model).unwrap();

    let result = manager.model().get(&model.id, "text").unwrap();
    assert_eq!(result.id, model.id);
    assert!(!result.data.is_empty());
}

#[tokio::test]
async fn export_manager_model_get_tree() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.model().deploy(&model).unwrap();

    let result = manager.model().get(&model.id, "tree").unwrap();
    assert_eq!(result.id, model.id);
    assert!(!result.data.is_empty());
}

#[tokio::test]
async fn export_manager_model_remove() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.model().deploy(&model).unwrap();

    manager.model().rm(&model.id).unwrap();
    assert_eq!(
        manager
            .model()
            .list(&ExecutorQuery::new().with_offset(0).with_count(10))
            .unwrap()
            .count,
        0
    );
}

#[tokio::test]
async fn export_manager_procs_one() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let proc = rt.create_proc(&utils::longid(), &model);
    engine.channel().on_start(move |_| s1.close());
    rt.launch(&proc);
    sig.recv().await;

    assert_eq!(
        manager
            .proc()
            .list(&ExecutorQuery::new().with_offset(0).with_count(10))
            .unwrap()
            .count,
        1
    );
}

#[tokio::test]
async fn export_manager_procs_count() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let count = Arc::new(Mutex::new(0));
    engine.channel().on_start(move |_e| {
        println!("message:{_e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            s1.close();
        }
    });
    for _ in 0..5 {
        let proc = rt.create_proc(&utils::longid(), &model);
        rt.launch(&proc);
    }
    sig.recv().await;
    assert_eq!(
        manager
            .proc()
            .list(&ExecutorQuery::new().with_offset(0).with_count(10))
            .unwrap()
            .count,
        5
    );
}

#[tokio::test]
async fn export_manager_procs_offset_in_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let count = Arc::new(Mutex::new(0));
    engine.channel().on_start(move |_e| {
        println!("message:{_e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            s1.close();
        }
    });
    for _ in 0..5 {
        let proc = rt.create_proc(&utils::longid(), &model);
        rt.launch(&proc);
    }
    sig.recv().await;
    assert_eq!(
        manager
            .proc()
            .list(&ExecutorQuery::new().with_offset(4).with_count(10))
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[tokio::test]
async fn export_manager_procs_offset_out_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let count = Arc::new(Mutex::new(0));
    engine.channel().on_start(move |_e| {
        println!("message:{_e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            s1.close();
        }
    });
    for _ in 0..5 {
        let proc = rt.create_proc(&utils::longid(), &model);
        rt.launch(&proc);
    }
    sig.recv().await;
    assert_eq!(
        manager
            .proc()
            .list(&ExecutorQuery::new().with_offset(1000).with_count(10))
            .unwrap()
            .rows
            .len(),
        0
    );
}

#[tokio::test]
async fn export_manager_procs_query() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let count = Arc::new(Mutex::new(0));
    engine.channel().on_start(move |_e| {
        println!("message:{_e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            s1.close();
        }
    });
    let pid = utils::longid();
    for i in 0..5 {
        let proc = rt.create_proc(&format!("{pid}-{}", i + 1), &model);
        rt.launch(&proc);
    }
    sig.recv().await;

    let rows = manager
        .proc()
        .list(&ExecutorQuery::new().with_query("id", format!("{pid}-3")))
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().unwrap().id, format!("{pid}-3"));
}

#[tokio::test]
async fn export_manager_procs_order() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    let count = Arc::new(Mutex::new(0));
    engine.channel().on_start(move |_e| {
        println!("message:{_e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            s1.close();
        }
    });
    let pid = utils::longid();
    for i in 0..5 {
        let proc = rt.create_proc(&format!("{pid}-{}", i + 1), &model);
        rt.launch(&proc);
    }
    sig.recv().await;

    let rows = manager
        .proc()
        .list(&ExecutorQuery::new().with_order("timestamp", true))
        .unwrap()
        .rows;
    assert_eq!(rows.first().unwrap().id, format!("{pid}-5"));
}

#[tokio::test]
async fn export_manager_proc_get() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_start(move |_| s1.close());
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    sig.recv().await;

    let info = manager.proc().get(&pid).unwrap();
    assert_eq!(info.id, pid);
    assert!(!info.tasks.is_empty());
}

#[tokio::test]
async fn export_manager_tasks_count() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;

    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(0)
                .with_count(10),
        )
        .unwrap();
    assert_eq!(tasks.count, 3); // 3 means the tasks with workflow step act
}

#[tokio::test]
async fn export_manager_tasks_offset_in_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;

    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(2)
                .with_count(10),
        )
        .unwrap();
    assert_eq!(tasks.rows.len(), 1); // 3 means the tasks with workflow step act
}

#[tokio::test]
async fn export_manager_tasks_offset_out_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;

    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(1000)
                .with_count(10),
        )
        .unwrap();
    assert_eq!(tasks.rows.len(), 0); // 3 means the tasks with workflow step act
}

#[tokio::test]
async fn export_manager_tasks_query() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;

    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_query("state", "interrupted"),
        )
        .unwrap();
    assert_eq!(tasks.rows.first().unwrap().r#type, "act");
    assert_eq!(tasks.rows.first().unwrap().key, "act1");
}

#[tokio::test]
async fn export_manager_tasks_order() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;

    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_order("timestamp", true),
        )
        .unwrap();
    assert_eq!(tasks.rows.first().unwrap().r#type, "act");
}

#[tokio::test]
async fn export_manager_task_get() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") {
            s1.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    rt.start(&model, &vars).unwrap();
    sig.recv().await;
    let tasks = manager
        .task()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(0)
                .with_count(10),
        )
        .unwrap();
    let mut result = true;
    for task in tasks.rows.iter() {
        result &= manager.task().get(&pid, &task.id).is_ok();
    }
    assert!(result);
}

#[tokio::test]
async fn export_manager_messages_all() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    let count = sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(&ExecutorQuery::default().with_offset(0).with_count(1000))
            .unwrap()
            .count,
        count
    );
}

#[tokio::test]
async fn export_manager_messages_query() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    let count = sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(
                &ExecutorQuery::new()
                    .with_query("pid", &pid)
                    .with_offset(0)
                    .with_count(1000)
            )
            .unwrap()
            .count,
        count
    );
}

#[tokio::test]
async fn export_manager_messages_order() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.close();
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(
                &ExecutorQuery::new()
                    .with_query("pid", &pid)
                    .with_order("timestamp", true)
            )
            .unwrap()
            .rows
            .first()
            .unwrap()
            .r#type,
        "act"
    );
}

#[tokio::test]
async fn export_manager_messages_count() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    let _ = sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(
                &ExecutorQuery::new()
                    .with_query("pid", &pid)
                    .with_offset(0)
                    .with_count(1)
            )
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[tokio::test]
async fn export_manager_messages_offset_in_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    let _ = sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(
                &ExecutorQuery::new()
                    .with_query("pid", &pid)
                    .with_offset(1)
                    .with_count(2)
            )
            .unwrap()
            .rows
            .len(),
        2
    );
}

#[tokio::test]
async fn export_manager_messages_offset_out_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    let _ = sig.recv().await;
    assert_eq!(
        manager
            .msg()
            .list(
                &ExecutorQuery::new()
                    .with_query("pid", &pid)
                    .with_offset(1000)
                    .with_count(100)
            )
            .unwrap()
            .rows
            .len(),
        0
    );
}

#[tokio::test]
async fn export_manager_message_get() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    sig.recv().await;

    let messages = manager
        .msg()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(0)
                .with_count(1000),
        )
        .unwrap();
    let message = messages.rows[0].clone();

    let m = manager.msg().get(&message.id).unwrap();
    assert_eq!(m.id, message.id);
    assert_eq!(m.name, message.name);
}

#[tokio::test]
async fn export_manager_message_rm() {
    let engine = Engine::new().start();
    let manager = engine.executor();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let (sig, s1) = engine.signal(0).double();
    let count = Arc::new(Mutex::new(0));
    let chan = engine.channel_with_options(&ChannelOptions {
        ack: true,
        ..Default::default()
    });
    chan.on_message(move |e| {
        println!("message:{e:?}");
        let mut count = count.lock().unwrap();
        *count += 1;

        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(*count);
        }
    });
    let pid = utils::longid();
    let proc = rt.create_proc(&pid, &model);
    rt.launch(&proc);
    sig.recv().await;

    let messages = manager
        .msg()
        .list(
            &ExecutorQuery::new()
                .with_query("pid", &pid)
                .with_offset(0)
                .with_count(1),
        )
        .unwrap();
    let message = messages.rows[0].clone();

    let ret = manager.msg().rm(&message.id).unwrap();
    assert!(ret);
}

#[tokio::test]
async fn export_manager_packages_count() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let count = 5;
    for i in 0..count {
        let package = Package {
            id: utils::longid(),
            desc: i.to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        manager.pack().publish(&package).unwrap();
    }
    assert_eq!(
        manager
            .pack()
            .list(
                &ExecutorQuery::new()
                    .with_query("built_in", false)
                    .with_offset(0)
                    .with_count(1000)
            )
            .unwrap()
            .count,
        count
    );
}

#[tokio::test]
async fn export_manager_packages_order() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let count = 5;
    for i in 0..count {
        let package = Package {
            id: utils::longid(),
            desc: format!("test-{}", i + 1),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        manager.pack().publish(&package).unwrap();
    }
    assert_eq!(
        manager
            .pack()
            .list(&ExecutorQuery::new().with_order("timestamp", true))
            .unwrap()
            .rows
            .first()
            .unwrap()
            .desc,
        "test-5"
    );
}

#[tokio::test]
async fn export_manager_packages_query() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let count = 5;
    for i in 0..count {
        let package = Package {
            id: utils::longid(),
            desc: format!("test-{}", i + 1),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        manager.pack().publish(&package).unwrap();
    }
    let rows = manager
        .pack()
        .list(&ExecutorQuery::new().with_query("desc", "test-3"))
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().unwrap().desc, "test-3");
}

#[tokio::test]
async fn export_manager_packages_offset_in_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let count = 5;
    for i in 0..count {
        let package = Package {
            id: utils::longid(),
            desc: i.to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        manager.pack().publish(&package).unwrap();
    }
    assert_eq!(
        manager
            .pack()
            .list(
                &ExecutorQuery::new()
                    .with_query("built_in", false)
                    .with_offset(4)
                    .with_count(1000)
            )
            .unwrap()
            .rows
            .len(),
        1
    );
}

#[tokio::test]
async fn export_manager_packages_offset_out_range() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let count = 5;
    for _i in 0..count {
        let package = Package {
            id: utils::longid(),
            desc: "desc".to_string(),
            icon: "icon".to_string(),
            doc: "doc".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            run_as: crate::ActRunAs::Func,
            groups: "[]".to_string(),
            catalog: crate::package::ActPackageCatalog::Core,
            create_time: utils::time::time_millis(),
            update_time: 0,
            timestamp: utils::time::timestamp(),
            built_in: false,
        };
        manager.pack().publish(&package).unwrap();
    }
    assert_eq!(
        manager
            .pack()
            .list(&ExecutorQuery::new().with_offset(1000).with_count(1000))
            .unwrap()
            .rows
            .len(),
        0
    );
}

#[tokio::test]
async fn export_manager_package_rm() {
    let engine = Engine::new().start();
    let manager = engine.executor();

    let package = Package {
        id: utils::longid(),
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        run_as: crate::ActRunAs::Func,
        groups: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: utils::time::time_millis(),
        update_time: 0,
        timestamp: utils::time::timestamp(),
        built_in: false,
    };
    manager.pack().publish(&package).unwrap();
    assert!(manager.pack().rm(&package.id).unwrap());
}

#[tokio::test]
async fn export_executor_start() {
    let engine = Engine::new().start();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"));

    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_complete(move |_| s1.close());

    engine.executor().model().deploy(&model).unwrap();

    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    let result = engine.executor().proc().start(&model.id, &vars);
    sig.recv().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn export_executor_start_not_found_model() {
    let engine = Engine::new().start();
    let sig = engine.signal(());
    let s1 = sig.clone();
    engine.channel().on_complete(move |_| s1.close());

    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    let result = engine.executor().proc().start("not_exists", &vars);
    assert!(result.is_err());
}

#[tokio::test]
async fn export_executor_complete() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().act().complete(&e.pid, &e.tid, &vars);
            s1.send(ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_complete_no_uid() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| rx.close());

    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let vars = Vars::new();
            let ret = engine.executor().act().complete(&e.pid, &e.tid, &vars);

            // no uid is still ok in version 0.7.0+
            s1.send(ret.is_ok());
        }
    });
    rt.start(&model, &Vars::new()).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_submit() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());

    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().act().submit(&e.pid, &e.tid, &vars);
            s1.send(ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_skip() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().act().skip(&e.pid, &e.tid, &vars);
            s1.send(ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_error() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            vars.insert("ecode".to_string(), json!("code_1"));
            let ret = engine.executor().act().error(&e.pid, &e.tid, &vars);
            s1.send(ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_abort() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    engine.channel().on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().act().abort(&e.pid, &e.tid, &vars);
            s1.send(ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_back() {
    let engine = Engine::new().start();
    let model = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::irq(|act| act.with_key("act2")))
        });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());

    let count = Arc::new(Mutex::new(0));
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut count = count.lock().unwrap();
            if *count == 1 {
                s1.close();
            }
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            engine
                .executor()
                .act()
                .complete(&e.pid, &e.tid, &vars)
                .unwrap();

            *count += 1;
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            vars.insert("to".to_string(), json!("step1"));
            let ret = engine.executor().act().back(&e.pid, &e.tid, &vars);
            s1.update(|data| *data = ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_cancel() {
    let engine = Engine::new().start();
    let model = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::irq(|act| act.with_key("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::irq(|act| act.with_key("act2")))
        });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    let count = Arc::new(Mutex::new(0));
    let tid = Arc::new(Mutex::new("".to_string()));
    engine.channel().on_message(move |e| {
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let mut count = count.lock().unwrap();
            if *count == 1 {
                s1.close();
            }
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            engine
                .executor()
                .act()
                .complete(&e.pid, &e.tid, &vars)
                .unwrap();

            *tid.lock().unwrap() = e.tid.clone();
            *count += 1;
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine
                .executor()
                .act()
                .cancel(&e.pid, &tid.lock().unwrap(), &vars);
            s1.update(|data| *data = ret.is_ok());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_push() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    engine.channel().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("step1") && e.is_state(MessageState::Created) {
            let vars = Vars::new()
                .with("uses", "acts.core.irq")
                .with("key", "act2");
            engine.executor().act().push(&e.pid, &e.tid, &vars).unwrap();
        }

        if e.is_key("act2") && e.is_state(MessageState::Created) {
            s1.send(true);
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_push_no_key_error() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    engine.channel().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("step1") && e.is_state(MessageState::Created) {
            s1.send(
                engine
                    .executor()
                    .act()
                    .push(&e.pid, &e.tid, &Vars::new())
                    .is_err(),
            );
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_push_not_step_id_error() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    // scher.emitter().on_complete(|e| e.close());
    engine.channel().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            let vars = Vars::new();
            s1.send(engine.executor().act().push(&e.pid, &e.tid, &vars).is_err());
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_executor_remove() {
    let engine = Engine::new().start();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("act1")))
    });

    let rt = engine.runtime();
    let sig = engine.signal(false);
    let s1 = sig.clone();
    engine.channel().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state(MessageState::Created) {
            s1.send(
                engine
                    .executor()
                    .act()
                    .remove(&e.pid, &e.tid, &Vars::new())
                    .is_ok(),
            );
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    rt.start(&model, &vars).unwrap();
    let ret = sig.recv().await;
    assert!(ret);
}

#[tokio::test]
async fn export_extender_register_module() {
    let engine = Engine::new().start();
    let extender = engine.extender();

    let before_count = engine.runtime().env().modules_count();
    let module = test_module::TestModule;
    extender.register_module(&module);
    let count = engine.runtime().env().modules_count();
    assert_eq!(count, before_count + 1);
}

#[tokio::test]
async fn export_emitter_default() {
    let engine = Engine::new().start();
    let emitter = engine.channel();
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message::default();
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.recv().await;
    assert_eq!(ret.len(), 1);
}

#[tokio::test]
async fn export_emitter_type_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        r#type: "a*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        r#type: "abc".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.recv().await;
    assert_eq!(ret.len(), 1);
}

#[tokio::test]
async fn export_emitter_type_not_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        r#type: "a*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        r#type: "bac".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.timeout(100).await;
    assert_eq!(ret.len(), 0);
}

#[tokio::test]
async fn export_emitter_state_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        state: "completed".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        state: MessageState::Completed,
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.recv().await;
    assert_eq!(ret.len(), 1);
}

#[tokio::test]
async fn export_emitter_state_not_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        r#type: "error".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        state: MessageState::Completed,
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.timeout(100).await;
    assert_eq!(ret.len(), 0);
}

#[tokio::test]
async fn export_emitter_tag_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        tag: "tag*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
    });

    let msg = Message {
        tag: "tag1".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);

    let msg = Message {
        tag: "aaaa".to_string(),
        model: Model {
            tag: "tag2".to_string(),
            ..Default::default()
        },
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);

    let ret = sig.timeout(100).await;
    assert_eq!(ret.len(), 2);
}

#[tokio::test]
async fn export_emitter_tag_not_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        tag: "tag*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        tag: "aaaa".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.timeout(100).await;
    assert_eq!(ret.len(), 0);
}

#[tokio::test]
async fn export_emitter_key_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        key: "key*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        key: "key1".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.recv().await;
    assert_eq!(ret.len(), 1);
}

#[tokio::test]
async fn export_emitter_key_not_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        key: "key*".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });

    let msg = Message {
        key: "aaaa".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.timeout(100).await;
    assert_eq!(ret.len(), 0);
}

#[tokio::test]
async fn export_emitter_on_message_with_dup_id() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        id: "dup_id".to_string(),
        ..Default::default()
    });
    let sig = engine.signal::<Vec<Message>>(Vec::new());
    let s = sig.clone();
    emitter.on_message(move |e| {
        s.update(|data| data.push(e.inner().clone()));
        s.close();
    });
    let s2 = sig.clone();
    // engine.runtime().emitter().remove("dup_id");
    let emitter2 = engine.channel_with_options(&ChannelOptions {
        id: "dup_id".to_string(),
        ..Default::default()
    });
    emitter2.on_message(move |e| {
        println!("message: {e:?}");
        s2.update(|data| data.push(e.inner().clone()));
        s2.close();
    });

    let msg = Message {
        key: "aaaa".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = sig.recv().await;
    assert_eq!(ret.len(), 1);
}

#[tokio::test]
async fn export_message_store_with_emit_id() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        id: "my_emit_id".to_string(),
        ack: true,
        ..Default::default()
    });
    let (s1, s2) = engine.signal::<Message>(Message::default()).double();
    emitter.on_message(move |e| {
        s1.send(e.inner().clone());
    });

    let msg = Message {
        id: "1".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = s2.recv().await;
    assert_eq!(ret.id, "1");
    assert!(
        engine
            .runtime()
            .cache()
            .store()
            .messages()
            .exists("1")
            .unwrap()
    );
}

#[tokio::test]
async fn export_message_store_with_emit_id_and_options() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        id: "my_emit_id".to_string(),
        tag: "tag*".to_string(),
        ack: true,
        ..Default::default()
    });
    let (s1, s2) = engine.signal::<Message>(Message::default()).double();
    emitter.on_message(move |e| {
        s1.send(e.inner().clone());
    });

    let msg = Message {
        id: utils::longid(),
        tag: "tagaaaa".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    let ret = s2.recv().await;
    assert_eq!(ret.id, msg.id);
    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&msg.id)
        .unwrap();
    assert_eq!(message.tag, msg.tag);
    assert_eq!(message.chan_id, "my_emit_id");
    assert_eq!(message.chan_pattern, "*:*:tag*:*");
}

#[tokio::test]
async fn export_message_not_store_without_match() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        id: "my_emit_id".to_string(),
        tag: "tag*".to_string(),
        ..Default::default()
    });
    let (s1, s2) = engine.signal::<Message>(Message::default()).double();
    emitter.on_message(move |e| {
        s1.send(e.inner().clone());
    });

    let msg = Message {
        id: utils::longid(),
        tag: "not_match_tag".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    s2.timeout(20).await;
    assert!(
        !engine
            .runtime()
            .cache()
            .store()
            .messages()
            .exists(&msg.id)
            .unwrap()
    );
}

#[tokio::test]
async fn export_message_not_store_with_empty_emit_id_and_not_match_option() {
    let engine = Engine::new().start();
    let emitter = engine.channel_with_options(&ChannelOptions {
        id: "".to_string(),
        ..Default::default()
    });
    let (s1, s2) = engine.signal::<Message>(Message::default()).double();
    emitter.on_message(move |e| {
        s1.send(e.inner().clone());
    });

    let msg = Message {
        id: "1".to_string(),
        ..Message::default()
    };
    engine.runtime().emitter().emit_message(&msg);
    s2.timeout(20).await;
    assert!(
        !engine
            .runtime()
            .cache()
            .store()
            .messages()
            .exists("1")
            .unwrap()
    );
}

#[tokio::test]
async fn export_message_clear_error_messages_by_none() {
    let engine = Engine::new().start();
    let msg = data::Message {
        id: utils::longid(),
        status: data::MessageStatus::Error,
        ..data::Message::default()
    };
    engine
        .runtime()
        .cache()
        .store()
        .messages()
        .create(&msg)
        .unwrap();
    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&msg.id)
        .unwrap();
    assert_eq!(message.status, data::MessageStatus::Error);
    engine.executor().msg().clear(None).unwrap();
    assert!(
        !engine
            .runtime()
            .cache()
            .store()
            .messages()
            .exists(&msg.id)
            .unwrap()
    );
}

#[tokio::test]
async fn export_message_clear_error_messages_by_pid() {
    let engine = Engine::new().start();
    let pid = utils::longid();
    engine
        .runtime()
        .cache()
        .store()
        .messages()
        .create(&data::Message {
            id: utils::longid(),
            status: data::MessageStatus::Error,
            pid: pid.clone(),
            ..data::Message::default()
        })
        .unwrap();

    engine
        .runtime()
        .cache()
        .store()
        .messages()
        .create(&data::Message {
            id: utils::longid(),
            status: data::MessageStatus::Error,
            pid: pid.clone(),
            ..data::Message::default()
        })
        .unwrap();
    engine.executor().msg().clear(Some(pid.clone())).unwrap();
    let messages = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .query(
            &Query::new().push(
                Cond::and()
                    .push(Expr::eq("pid", pid))
                    .push(Expr::eq("status", data::MessageStatus::Error)),
            ),
        )
        .unwrap();

    assert_eq!(messages.rows.len(), 0);
}

#[tokio::test]
async fn export_message_resend_error_messages() {
    let engine = Engine::new().start();
    let msg = data::Message {
        id: utils::longid(),
        status: data::MessageStatus::Error,
        ..data::Message::default()
    };
    engine
        .runtime()
        .cache()
        .store()
        .messages()
        .create(&msg)
        .unwrap();
    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&msg.id)
        .unwrap();
    assert_eq!(message.status, data::MessageStatus::Error);
    engine.executor().msg().redo().unwrap();

    let message = engine
        .runtime()
        .cache()
        .store()
        .messages()
        .find(&msg.id)
        .unwrap();
    assert_eq!(message.status, data::MessageStatus::Created);
    assert_eq!(message.retry_times, 0);
}

mod test_module {
    use crate::ActModule;

    #[derive(Clone)]
    pub struct TestModule;
    impl ActModule for TestModule {
        fn init(&self, _ctx: &rquickjs::Ctx<'_>) -> crate::Result<()> {
            Ok(())
        }
    }
}
