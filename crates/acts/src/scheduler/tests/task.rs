use crate::{
    MessageState, TaskState, Vars, Workflow,
    event::EventAction,
    scheduler::NodeContent,
    utils::{self, test::USES_IRQ, test::auto_complete, test::create_proc},
};

use serial_test::serial;

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_start() {
    let workflow = Workflow::new().with_step(|step| step.with_id("step1"));
    let (engine, proc) = create_proc(&workflow, "w1");
    let rt = engine.runtime();
    let sig = engine.signal(TaskState::default());
    let tx = sig.clone();
    let rx = sig.clone();

    proc.start().unwrap();
    rt.emitter().on_proc(move |e| rx.send(e.state()));

    let ret = tx.recv().await;
    assert_eq!(ret, TaskState::Running);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_steps() {
    let workflow = Workflow::new()
        .with_step(|mut step| {
            step.name = "step1".to_string();
            step
        })
        .with_step(|mut step| {
            step.name = "step2".to_string();
            step
        });
    let id = utils::longid();
    let (engine, proc) = create_proc(&workflow, &id);
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_branch_basic() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_name("step1")
            .with_branch(|branch| {
                branch
                    .with_if("true")
                    .with_name("branch 1")
                    .with_step(|step| step.with_name("step11"))
                    .with_step(|step| step.with_name("step12"))
                    .with_step(|step| step.with_name("step13"))
            })
            .with_branch(|branch| {
                branch
                    .with_name("branch 2")
                    .with_step(|step| step.with_name("step21"))
            })
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(());
    let emitter = engine.channel();
    emitter.on_complete({
        let sig = sig.clone();
        move |_| sig.close()
    });
    emitter.on_error({
        let sig = sig.clone();
        move |_| sig.close()
    });

    rt.launch(&proc).unwrap();
    let _ = sig.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_act_skip_with_inputs_to_next() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_var("v1", 10)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
            .with_uses(USES_IRQ, Vars::new().with("key", "act2"))
    });

    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let sig = engine.signal(Vars::new());
    let rx = sig.clone();
    let rt2 = rt.clone();
    let emitter = engine.channel();
    emitter.on_message(move |e| {
        println!("message: {e:?}");
        if e.params().unwrap().get::<String>("key").as_deref() == Some("act1")
            && e.is_state(MessageState::Created)
        {
            rt2.do_action2(&e.pid, &e.tid, EventAction::Skip, Vars::new())
                .unwrap();
        }

        if e.params().unwrap().get::<String>("key").as_deref() == Some("act2")
            && e.is_state(MessageState::Created)
        {
            rx.send(e.inputs.clone());
        }
    });

    rt.launch(&proc).unwrap();
    let ret = sig.recv().await;
    assert_eq!(ret.get::<i32>("v1").unwrap(), 10);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_name() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step1").with_name("my_step_name"))
        .with_step(|step| step.with_id("step2"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    // verify the step name is preserved on the task node
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    assert_eq!(task.node().name(), "my_step_name");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_desc() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("step1_name")
            .with_desc("this is a step description")
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    // verify the step desc is preserved on the task node
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    if let NodeContent::Step(step) = &task.node().content {
        assert_eq!(step.desc, "this is a step description");
    } else {
        panic!("expected step node content");
    }
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_id() {
    let workflow = Workflow::new()
        .with_step(|step| step.with_id("step_100").with_name("step one"))
        .with_step(|step| step.with_id("step_200").with_name("step two"));
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    // verify task lookup by step id works correctly
    let tasks = proc.task_by_nid("step_100");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks.first().unwrap().node().name(), "step one");
    let tasks = proc.task_by_nid("step_200");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks.first().unwrap().node().name(), "step two");
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_options() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_options("timeout", 30)
            .with_options("retry", 3)
            .with_uses(USES_IRQ, Vars::new().with("key", "act1"))
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vars::new()));
    let captured2 = captured.clone();
    let rt2 = rt.clone();
    engine.channel().on_message(move |e| {
        if e.is_type("step") && e.is_state(MessageState::Created) {
            *captured2.lock().unwrap() = e.inner().inputs.clone();
        }
        if e.is_params_key("act1") && e.is_state(MessageState::Created) {
            rt2.do_action2(&e.pid, &e.tid, EventAction::Next, Vars::new())
                .unwrap();
        }
    });
    rt.launch(&proc).unwrap();
    tx.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    // verify step options set via with_options are in message.inputs
    let inputs = captured.lock().unwrap();
    let options: serde_json::Value = inputs.get("options").unwrap();
    assert_eq!(options["timeout"].as_i64().unwrap(), 30);
    assert_eq!(options["retry"].as_i64().unwrap(), 3);
}

#[serial]
#[tokio::test(flavor = "multi_thread")]
async fn sch_task_step_metadata() {
    let workflow = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_name("meta_step")
            .with_metadata("color", "blue")
            .with_metadata("width", 200)
    });
    let (engine, proc) = create_proc(&workflow, &utils::longid());
    let rt = engine.runtime();
    let (tx, rx) = engine.signal(()).double();
    auto_complete(&engine, &rx);
    rt.launch(&proc).unwrap();
    tx.recv().await;
    assert_eq!(proc.state(), TaskState::Completed);
    // verify step metadata is preserved on the task node
    let tasks = proc.task_by_nid("step1");
    let task = tasks.first().unwrap();
    if let NodeContent::Step(step) = &task.node().content {
        let color: String = step.metadata.get("color").unwrap_or_default();
        assert_eq!(color, "blue");
        let width: i32 = step.metadata.get("width").unwrap_or_default();
        assert_eq!(width, 200);
    } else {
        panic!("expected step node content");
    }
}
