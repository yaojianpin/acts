use serde_json::Value;
use std::sync::Arc;

use crate::{
    ActRunAs, LegacyTaskState, ModelInfo, NodeKind, PackageInfo, ProcInfo, TaskInfo, Workflow,
    data, package::ActPackageCatalog, scheduler::NodeData, utils,
};

#[test]
fn model_info_package() {
    let package = &data::Package {
        id: utils::shortid(),
        name: "test_package".to_string(),
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        options: None,
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        built_in: false,
        v: 0,
    };
    let info: PackageInfo = package.into();
    assert_eq!(info.id, package.id);
    assert_eq!(info.name, package.name);
    assert_eq!(info.desc, package.desc);
    assert_eq!(info.icon, package.icon);
    assert_eq!(info.doc, package.doc);
    assert_eq!(info.version, package.version);
    assert_eq!(info.schema, package.schema);
    assert_eq!(info.run_as, package.run_as);
    assert_eq!(info.resources, package.resources);
    assert_eq!(info.catalog, package.catalog);
    assert_eq!(info.create_time, package.create_time);
    assert_eq!(info.update_time, package.update_time);
    assert_eq!(info.timestamp, package.timestamp);
}

#[test]
fn model_info_proc() {
    let proc = &data::Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: LegacyTaskState::None.into(),
        start_time: 1234,
        end_time: 2345,
        timestamp: 11111,
        model: "{}".to_string(),
        env: "".to_string(),
        err: None,
        removable: false,
        source_revision: String::new(),
        v: 0,
    };
    let info: ProcInfo = proc.into();
    assert_eq!(info.id, proc.id);
    assert_eq!(info.name, proc.name);
    assert_eq!(info.state, proc.state);
    assert_eq!(info.start_time, proc.start_time);
    assert_eq!(info.end_time, proc.end_time);
    assert_eq!(info.timestamp, proc.timestamp);
}

#[test]
fn model_info_task() {
    let workflow = Workflow::new();
    let node_data = NodeData {
        id: "nid".to_string(),
        content: crate::scheduler::NodeContent::Workflow(Arc::new(workflow)),
        level: 0,
        parent_id: None,
        prev_id: None,
        next_id: None,
    };
    let task = data::Task {
        id: utils::shortid(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: serde_json::to_string(&node_data).unwrap(),
        state: LegacyTaskState::None.into(),
        prev: None,
        next: vec![],
        parent: None,
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        err: None,
        source_revision: String::new(),
        v: 0,
    };
    let info: TaskInfo = task.clone().into();
    assert_eq!(info.id, task.tid);
    assert_eq!(info.r#type, task.kind);
    assert_eq!(info.name, task.name);
    assert_eq!(info.state, task.state);
    assert_eq!(info.start_time, task.start_time);
    assert_eq!(info.end_time, task.end_time);
    assert_eq!(info.timestamp, task.timestamp);
    assert_eq!(info.pid, task.pid);
    // durable lifecycle rows carry no scope vars — the manager view joins the
    // paired vars row (see `TaskExecutor::join_vars`)
    assert!(info.data.is_empty());
    assert_eq!(info.nid, "nid");
}

#[test]
fn model_info_model() {
    let model = data::Model {
        id: utils::longid(),
        name: "test_model".to_string(),
        desc: "test model desc".to_string(),
        ver: "0.1.0".to_string(),
        size: 1245,
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        view: None,
        timestamp: 0,
        v: 0,
    };
    let info: ModelInfo = model.clone().into();
    assert_eq!(info.id, model.id);
    assert_eq!(info.name, model.name);
    assert_eq!(info.ver, model.ver);
    assert_eq!(info.size, model.size);
    assert_eq!(info.create_time, model.create_time);
    assert_eq!(info.update_time, model.update_time);
    assert_eq!(info.data, model.data);
}

#[test]
fn model_info_package_arr_to_value() {
    let package = &data::Package {
        id: utils::shortid(),
        name: "name".to_string(),
        desc: "desc".to_string(),
        icon: "icon".to_string(),
        doc: "doc".to_string(),
        version: "0.1.0".to_string(),
        schema: "{}".to_string(),
        options: None,
        run_as: crate::ActRunAs::Func,
        resources: "[]".to_string(),
        catalog: crate::package::ActPackageCatalog::Core,
        create_time: 0,
        update_time: 0,
        timestamp: 0,
        built_in: false,
        v: 0,
    };
    let info: PackageInfo = package.into();

    let arr: Vec<PackageInfo> = vec![info.clone()];

    let v: Value = arr.into();
    assert!(v.is_array());

    let v = &v[0];
    assert_eq!(v.get("id").unwrap().as_str().unwrap(), info.id);
    assert_eq!(v.get("desc").unwrap().as_str().unwrap(), info.desc);
    assert_eq!(v.get("icon").unwrap().as_str().unwrap(), info.icon);
    assert_eq!(v.get("doc").unwrap().as_str().unwrap(), info.doc);
    assert_eq!(v.get("version").unwrap().as_str().unwrap(), info.version);
    assert_eq!(v.get("schema").unwrap().as_str().unwrap(), info.schema);
    assert_eq!(
        serde_json::from_value::<ActRunAs>(v.get("run_as").unwrap().clone()).unwrap(),
        info.run_as
    );
    assert_eq!(
        v.get("resources").unwrap().as_str().unwrap(),
        info.resources
    );
    assert_eq!(
        serde_json::from_value::<ActPackageCatalog>(v.get("catalog").unwrap().clone()).unwrap(),
        info.catalog
    );
    assert_eq!(
        v.get("create_time").unwrap().as_i64().unwrap(),
        info.create_time
    );
    assert_eq!(
        v.get("update_time").unwrap().as_i64().unwrap(),
        info.update_time
    );
    assert_eq!(
        v.get("timestamp").unwrap().as_i64().unwrap(),
        info.timestamp
    );
}

#[test]
fn model_info_proc_arr_to_value() {
    let proc = &data::Proc {
        id: utils::shortid(),
        name: "test".to_string(),
        mid: "m1".to_string(),
        state: LegacyTaskState::None.into(),
        start_time: 1234,
        end_time: 2345,
        timestamp: 11111,
        model: "{}".to_string(),
        env: "".to_string(),
        err: None,
        removable: false,
        source_revision: String::new(),
        v: 0,
    };
    let info: ProcInfo = proc.into();

    let arr: Vec<ProcInfo> = vec![info.clone()];

    let v: Value = arr.into();
    assert!(v.is_array());

    let v = &v[0];
    assert_eq!(v.get("id").unwrap().as_str().unwrap(), info.id);
    assert_eq!(v.get("mid").unwrap().as_str().unwrap(), info.mid);
    assert_eq!(v.get("name").unwrap().as_str().unwrap(), info.name);
    assert_eq!(v.get("state").unwrap().as_str().unwrap(), info.state);
    assert_eq!(
        v.get("start_time").unwrap().as_i64().unwrap(),
        info.start_time
    );
    assert_eq!(v.get("end_time").unwrap().as_i64().unwrap(), info.end_time);
    assert_eq!(
        v.get("timestamp").unwrap().as_i64().unwrap(),
        info.timestamp
    );
}

#[test]
fn model_info_task_arr_to_value() {
    let workflow = Workflow::new();
    let node_data = NodeData {
        id: "nid".to_string(),
        content: crate::scheduler::NodeContent::Workflow(Arc::new(workflow)),
        level: 0,
        parent_id: None,
        prev_id: None,
        next_id: None,
    };
    let task = data::Task {
        id: utils::shortid(),
        kind: NodeKind::Workflow.into(),
        name: "test".to_string(),
        pid: "pid".to_string(),
        tid: "tid".to_string(),
        node_data: serde_json::to_string(&node_data).unwrap(),
        state: LegacyTaskState::None.into(),
        prev: None,
        next: vec![],
        parent: None,
        start_time: 0,
        end_time: 0,
        timestamp: 0,
        err: None,
        source_revision: String::new(),
        v: 0,
    };
    let info: TaskInfo = task.clone().into();

    let arr: Vec<TaskInfo> = vec![info.clone()];

    let v: Value = arr.into();
    assert!(v.is_array());

    let v = &v[0];
    assert_eq!(v.get("id").unwrap().as_str().unwrap(), info.id);
    assert_eq!(v.get("type").unwrap().as_str().unwrap(), info.r#type);
    assert_eq!(v.get("name").unwrap().as_str().unwrap(), info.name);
    assert_eq!(v.get("pid").unwrap().as_str().unwrap(), info.pid);
    assert_eq!(v.get("state").unwrap().as_str().unwrap(), info.state);
    assert_eq!(v.get("nid").unwrap().as_str().unwrap(), info.nid);

    assert_eq!(
        v.get("start_time").unwrap().as_i64().unwrap(),
        info.start_time
    );
    assert_eq!(v.get("end_time").unwrap().as_i64().unwrap(), info.end_time);
    assert_eq!(v.get("data").unwrap().as_str().unwrap(), info.data);
}

#[test]
fn model_info_model_arr_to_value() {
    let model = data::Model {
        id: utils::longid(),
        name: "test_model".to_string(),
        desc: "test model desc".to_string(),
        ver: "0.1.0".to_string(),
        size: 1245,
        create_time: 3333,
        update_time: 0,
        data: "{}".to_string(),
        view: None,
        timestamp: 0,
        v: 0,
    };

    let arr: Vec<ModelInfo> = vec![model.clone().into()];

    let v: Value = arr.into();

    assert!(v.is_array());

    let v = &v[0];
    assert_eq!(v.get("name").unwrap().as_str().unwrap(), model.name);
    assert_eq!(v.get("ver").unwrap().as_str().unwrap(), model.ver);
    assert_eq!(v.get("size").unwrap().as_u64().unwrap(), model.size as u64);
    assert_eq!(
        v.get("create_time").unwrap().as_i64().unwrap(),
        model.create_time
    );
    assert_eq!(
        v.get("update_time").unwrap().as_i64().unwrap(),
        model.update_time
    );
    assert_eq!(v.get("data").unwrap().as_str().unwrap(), model.data);
}

#[test]
fn model_info_workflow_parses_and_overrides_ver() {
    let info = ModelInfo {
        id: "m1".to_string(),
        name: "model".to_string(),
        desc: String::new(),
        ver: "2.0.0".to_string(),
        size: 0,
        create_time: 0,
        update_time: 0,
        data: "id: m1\nver: 0.1.0\nsteps:\n  - id: s1\n    uses: acts.core.irq\n".to_string(),
        view: None,
    };
    let workflow = info.workflow().unwrap();
    assert_eq!(workflow.id, "m1");
    assert_eq!(workflow.ver, "2.0.0");
}

#[test]
fn model_info_workflow_invalid_data_is_a_convert_error() {
    let info = ModelInfo {
        id: "m1".to_string(),
        name: "model".to_string(),
        desc: String::new(),
        ver: "1.0.0".to_string(),
        size: 0,
        create_time: 0,
        update_time: 0,
        data: "steps: not-a-list".to_string(),
        view: None,
    };
    let err = info.workflow().unwrap_err();
    assert!(matches!(err, crate::ActError::Convert(_)), "{err}");
}

#[test]
fn model_info_message_from_delivery() {
    let delivery = data::Delivery {
        id: "d1".to_string(),
        msg_id: "m1".to_string(),
        pid: "p1".to_string(),
        tid: "t1".to_string(),
        chan_id: "chan".to_string(),
        chan_pattern: "*".to_string(),
        retry_times: 2,
        create_time: 10,
        update_time: 20,
        ..Default::default()
    };
    let message = data::Message {
        id: "m1".to_string(),
        tid: "t1".to_string(),
        name: "msg".to_string(),
        r#type: "act".to_string(),
        pid: "p1".to_string(),
        nid: "n1".to_string(),
        uses: Some("acts.core.msg".to_string()),
        inputs: "{}".to_string(),
        outputs: "{}".to_string(),
        timestamp: 30,
        ..Default::default()
    };
    let info = crate::MessageInfo::from_delivery(&delivery, &message);
    assert_eq!(info.id, "d1");
    assert_eq!(info.msg_id, "m1");
    assert_eq!(info.chan_id, "chan");
    assert_eq!(info.tid, "t1");
    assert_eq!(info.name, "msg");
    assert_eq!(info.r#type, "act");
    assert_eq!(info.pid, "p1");
    assert_eq!(info.nid, "n1");
    assert_eq!(info.uses.as_deref(), Some("acts.core.msg"));
    assert_eq!(info.create_time, 10);
    assert_eq!(info.update_time, 20);
    assert_eq!(info.retry_times, 2);
    assert_eq!(info.timestamp, 30);

    let value: Value = info.into();
    assert_eq!(value["id"], "d1");
    assert_eq!(value["retry_times"], 2);
}

#[test]
fn model_info_event_to_value() {
    let event = data::Event {
        id: "e1".to_string(),
        name: "event".to_string(),
        mid: "m1".to_string(),
        ver: "0.1.0".to_string(),
        kind: "schedule".to_string(),
        params: "{}".to_string(),
        schedule: Some("* * * * * *".to_string()),
        last_run: 1,
        next_run: 2,
        create_time: 3,
        timestamp: 4,
        ..Default::default()
    };
    let info: crate::EventInfo = (&event).into();
    assert_eq!(info.id, "e1");
    assert_eq!(info.mid, "m1");
    assert_eq!(info.kind, "schedule");
    assert_eq!(info.schedule.as_deref(), Some("* * * * * *"));
    assert_eq!(info.last_run, 1);
    assert_eq!(info.next_run, 2);

    let value: Value = info.into();
    assert_eq!(value["id"], "e1");
    assert_eq!(value["kind"], "schedule");
    assert_eq!(value["next_run"], 2);
}
