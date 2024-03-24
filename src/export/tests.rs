use crate::{data, sch::TaskState, utils, Act, ActPlugin, Engine, StoreAdapter, Vars, Workflow};
use rhai::plugin::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn export_manager_publish_ok() {
    let engine = Engine::new();
    let manager = engine.manager();
    let pack = data::Package {
        id: "pack1".to_string(),
        name: "package 1".to_string(),
        file_data: vec![0x01, 0x02],
        ..Default::default()
    };

    let result = manager.publish(&pack);

    assert_eq!(result.is_ok(), true);
    assert_eq!(manager.publish(&pack).is_ok(), true);
}

#[tokio::test]
async fn export_manager_deploy_ok() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));

    let result = manager.deploy(&model);

    assert_eq!(result.is_ok(), true);
    assert_eq!(manager.model(&model.id, "text").is_ok(), true);
}

#[tokio::test]
async fn export_manager_deploy_many_times() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"));

    let mut result = true;
    for _ in 0..10 {
        let state = manager.deploy(&model);
        result &= state.is_ok();
    }
    assert_eq!(result, true);
}

#[tokio::test]
async fn export_manager_deploy_no_model_id_error() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let result = manager.deploy(&model);
    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn export_manager_deploy_dup_id_error() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step1"));

    let result = manager.deploy(&model);
    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn engine_executor_start_no_pid() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));
    engine.manager().deploy(&workflow).unwrap();
    let options = Vars::new();
    let result = executor.start(&workflow.id, &options);
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn engine_executor_start_with_pid() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));
    engine.manager().deploy(&workflow).unwrap();
    let mut options = Vars::new();
    options.insert("pid".to_string(), "123".into());
    let result = executor.start(&workflow.id, &options);
    assert_eq!(result.is_ok(), true);

    assert_eq!(
        result.unwrap().outputs().get::<String>("pid").unwrap(),
        "123"
    );
}

#[tokio::test]
async fn export_executor_start_empty_pid() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let mid = utils::longid();
    let workflow = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));

    engine.manager().deploy(&workflow).unwrap();
    let mut options = Vars::new();
    options.insert("pid".to_string(), "".into());
    let result = executor.start(&workflow.id, &options);
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn export_executor_start_dup_pid_error() {
    let engine = Engine::new();
    let executor = engine.executor();
    engine.start();

    let pid = utils::longid();
    let mid = utils::longid();
    let model = Workflow::new()
        .with_id(&mid)
        .with_step(|step| step.with_act(Act::req(|act| act.with_id("test"))));

    let store = engine.scher().cache().store();
    let proc = data::Proc {
        id: pid.clone(),
        name: model.name.clone(),
        mid: model.id.clone(),
        state: TaskState::None.to_string(),
        start_time: 0,
        end_time: 0,
        vars: "".to_string(),
        timestamp: 0,
        model: model.to_json().unwrap(),
        root_tid: "".to_string(),
    };
    store.procs().create(&proc).expect("create proc");
    engine
        .manager()
        .deploy(&model)
        .expect("fail to deploy workflow");
    let mut options = Vars::new();
    options.insert("pid".to_string(), json!(pid.to_string()));
    let result = executor.start(&model.id, &options);
    assert_eq!(result.is_err(), true);
}

#[tokio::test]
async fn export_manager_models_get() {
    let engine = Engine::new();
    let manager = engine.manager();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    for _ in 0..5 {
        model.set_id(&utils::longid());
        manager.deploy(&model).unwrap();
    }

    let result = manager.models(10).unwrap();
    assert_eq!(result.len(), 5);
}

#[tokio::test]
async fn export_manager_model_get_text() {
    let engine = Engine::new();
    let manager = engine.manager();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.deploy(&model).unwrap();

    let result = manager.model(&model.id, "text").unwrap();
    assert_eq!(result.id, model.id);
    assert_eq!(result.model.is_empty(), false);
}

#[tokio::test]
async fn export_manager_model_get_tree() {
    let engine = Engine::new();
    let manager = engine.manager();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.deploy(&model).unwrap();

    let result = manager.model(&model.id, "tree").unwrap();
    assert_eq!(result.id, model.id);
    assert_eq!(result.model.is_empty(), false);
}

#[tokio::test]
async fn export_manager_model_remove() {
    let engine = Engine::new();
    let manager = engine.manager();
    let mut model = Workflow::new().with_step(|step| step.with_id("step1"));

    model.set_id(&utils::longid());
    manager.deploy(&model).unwrap();

    manager.remove(&model.id).unwrap();
    assert_eq!(manager.models(10).unwrap().len(), 0);
}

#[tokio::test]
async fn export_manager_procs_get_one() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let scher = engine.scher();
    let proc = scher.create_proc(&utils::longid(), &model);
    scher.emitter().on_start(|e| e.close());
    scher.launch(&proc);
    scher.event_loop().await;

    assert_eq!(manager.procs(10).unwrap().len(), 1);
}

#[tokio::test]
async fn export_manager_procs_get_many() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let scher = engine.scher();

    let count = Arc::new(Mutex::new(0));
    scher.emitter().on_start(move |e| {
        let mut count = count.lock().unwrap();
        *count += 1;

        if *count == 5 {
            e.close()
        }
    });
    for _ in 0..5 {
        let proc = scher.create_proc(&utils::longid(), &model);
        scher.launch(&proc);
    }

    scher.event_loop().await;

    assert_eq!(manager.procs(10).unwrap().len(), 5);
}

#[tokio::test]
async fn export_manager_proc_get_json() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let scher = engine.scher();
    scher.emitter().on_start(move |e| e.close());
    let pid = utils::longid();
    let proc = scher.create_proc(&pid, &model);
    scher.launch(&proc);
    scher.event_loop().await;

    let info = manager.proc(&pid, "json").unwrap();
    assert_eq!(info.id, pid);
    assert_eq!(info.tasks.is_empty(), false);
}

#[tokio::test]
async fn export_manager_proc_get_tree() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| step.with_id("step1"));

    let scher = engine.scher();
    scher.emitter().on_start(move |e| e.close());
    let pid = utils::longid();
    let proc = scher.create_proc(&pid, &model);
    scher.launch(&proc);
    scher.event_loop().await;

    let info = manager.proc(&pid, "tree").unwrap();
    assert_eq!(info.id, pid);
    assert_eq!(info.tasks.is_empty(), false);
}

#[tokio::test]
async fn export_manager_tasks() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") {
            e.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;

    let tasks = manager.tasks(&pid, 10).unwrap();
    assert_eq!(tasks.len(), 3); // 3 means the tasks with workflow step act
}

#[tokio::test]
async fn export_manager_task_get() {
    let engine = Engine::new();
    let manager = engine.manager();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") {
            e.close()
        }
    });
    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    let tasks = manager.tasks(&pid, 10).unwrap();
    let mut result = true;
    for task in tasks {
        result &= manager.task(&pid, &task.id).is_ok();
    }
    assert_eq!(result, true);
}

#[tokio::test]
async fn export_executeor_start() {
    let engine = Engine::new();
    let model = Workflow::new()
        .with_id(&utils::longid())
        .with_step(|step| step.with_id("step1"));

    let scher = engine.scher();
    scher.emitter().on_complete(move |e| e.close());

    engine.manager().deploy(&model).unwrap();

    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    let result = engine.executor().start(&model.id, &vars);
    scher.event_loop().await;
    assert_eq!(result.is_ok(), true);
}

#[tokio::test]
async fn export_executeor_start_not_found_model() {
    let engine = Engine::new();
    let scher = engine.scher();
    scher.emitter().on_complete(move |e| e.close());

    let pid = utils::longid();
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    vars.insert("pid".to_string(), json!(pid));

    let result = engine.executor().start("not_exists", &vars);
    assert_eq!(result.is_ok(), false);
}

#[tokio::test]
async fn export_executeor_complete() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().complete(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_complete_no_uid() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let vars = Vars::new();
            let ret = engine.executor().complete(&e.proc_id, &e.id, &vars);

            // no uid is still ok in version 0.7.0+
            *r.lock().unwrap() = ret.is_ok();
            e.close();
        }
    });
    scher.start(&model, &Vars::new()).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_submit() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().submit(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_skip() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().skip(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_error() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_error(|e| {
        e.close();
    });

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            vars.insert("err_code".to_string(), json!("code_1"));
            let ret = engine.executor().error(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_abort() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        println!("message: {:?}", e.inner());
        if e.is_key("act1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine.executor().abort(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_back() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|act| act.with_id("act2")))
        });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    let count = Arc::new(Mutex::new(0));
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut count = count.lock().unwrap();
            if *count == 1 {
                e.close();
            }
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            engine
                .executor()
                .complete(&e.proc_id, &e.id, &vars)
                .unwrap();

            *count += 1;
        }

        if e.is_key("act2") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            vars.insert("to".to_string(), json!("step1"));
            let ret = engine.executor().back(&e.proc_id, &e.id, &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_cancel() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new()
        .with_step(|step| {
            step.with_id("step1")
                .with_act(Act::req(|act| act.with_id("act1")))
        })
        .with_step(|step| {
            step.with_id("step2")
                .with_act(Act::req(|act| act.with_id("act2")))
        });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    let count = Arc::new(Mutex::new(0));
    let tid = Arc::new(Mutex::new("".to_string()));
    scher.emitter().on_message(move |e| {
        if e.is_key("act1") && e.is_state("created") {
            let mut count = count.lock().unwrap();
            if *count == 1 {
                e.close();
            }
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            engine
                .executor()
                .complete(&e.proc_id, &e.id, &vars)
                .unwrap();

            *tid.lock().unwrap() = e.id.clone();
            *count += 1;
        }

        if e.is_key("act2") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("uid".to_string(), json!("u1"));
            let ret = engine
                .executor()
                .cancel(&e.proc_id, &tid.lock().unwrap(), &vars);
            *r.lock().unwrap() = ret.is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_push() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("step1") && e.is_state("created") {
            let mut vars = Vars::new();
            vars.insert("id".to_string(), json!("act2"));
            engine.executor().push(&e.proc_id, &e.id, &vars).unwrap();
        }

        if e.is_key("act2") && e.is_state("created") {
            *r.lock().unwrap() = true;
            e.close();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_push_no_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("step1") && e.is_state("created") {
            let vars = Vars::new();
            *r.lock().unwrap() = engine.executor().push(&e.proc_id, &e.id, &vars).is_err();
            e.close();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_push_not_step_id_error() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            let vars = Vars::new();
            *r.lock().unwrap() = engine.executor().push(&e.proc_id, &e.id, &vars).is_err();
            e.close();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn export_executeor_remove() {
    let ret = Arc::new(Mutex::new(false));
    let engine = Engine::new();
    let model = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::req(|act| act.with_id("act1")))
    });

    let scher = engine.scher();
    scher.emitter().on_complete(|e| e.close());

    let r = ret.clone();
    scher.emitter().on_message(move |e| {
        println!("message: {e:?}");
        if e.is_key("act1") && e.is_state("created") {
            *r.lock().unwrap() = engine
                .executor()
                .remove(&e.proc_id, &e.id, &Vars::new())
                .is_ok();
        }
    });
    let mut vars = Vars::new();
    vars.insert("uid".to_string(), json!("u1"));
    scher.start(&model, &vars).unwrap();
    scher.event_loop().await;
    assert_eq!(*ret.lock().unwrap(), true);
}

#[tokio::test]
async fn engine_extender_register_plugin() {
    let engine = Engine::new();
    let extender = engine.extender();

    let plugin_count = extender.plugins.lock().unwrap().len();
    extender.register_plugin(&TestPlugin::default());

    assert_eq!(extender.plugins.lock().unwrap().len(), plugin_count + 1);
}

#[tokio::test]
async fn export_extender_register_module() {
    let engine = Engine::new();
    let extender = engine.extender();
    let mut module = Module::new();
    combine_with_exported_module!(&mut module, "role", test_module);
    extender.register_module("test", &module);

    assert!(extender.modules().contains_key("test"));
}

#[derive(Debug, Default, Clone)]
struct TestPlugin;

impl ActPlugin for TestPlugin {
    fn on_init(&self, _engine: &Engine) {
        println!("TestPlugin");
    }
}

#[export_module]
mod test_module {

    #[export_fn]
    pub fn test(_name: &str) {}
}
