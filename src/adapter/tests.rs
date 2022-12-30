use crate::{
    sch::{Proc, Scheduler},
    Context, Engine, OrgAdapter, RoleAdapter, RuleAdapter, Workflow,
};
use std::sync::Arc;

#[tokio::test]
async fn adapter_role() {
    let engine = create_engine_with_adapter();
    let users = engine.adapter().role("admin");
    assert_eq!(users, vec!["a1".to_string()]);
}

#[tokio::test]
async fn adapter_dept() {
    let engine = create_engine_with_adapter();
    let users = engine.adapter().dept("d1");
    assert_eq!(users, vec!["u1".to_string(), "u2".to_string()]);
}

#[tokio::test]
async fn adapter_unit() {
    let engine = create_engine_with_adapter();
    let users = engine.adapter().unit("u1");
    assert_eq!(
        users,
        vec![
            "u1".to_string(),
            "u2".to_string(),
            "u3".to_string(),
            "u4".to_string()
        ]
    );
}

#[tokio::test]
async fn adapter_some() {
    let engine = create_engine_with_adapter();

    let workflow = create_workflow();

    let job = workflow.jobs.get(0).unwrap();
    let step = job.steps.get(0).unwrap();

    let (task, _) = create_proc(&workflow);
    let ctx = Context::new(&task);

    let ret = engine.adapter().some("some1", step, &ctx);
    assert_eq!(ret, Ok(true));
}

#[tokio::test]
async fn adapter_ord() {
    let engine = create_engine_with_adapter();

    let acts = vec!["u1".to_string(), "u2".to_string()];

    let users = engine.adapter().ord("ord1", &acts).unwrap();
    assert_eq!(users, vec!["u2".to_string(), "u1".to_string(),]);
}

#[tokio::test]
async fn adapter_relate() {
    let engine = create_engine_with_adapter();

    let users = engine.adapter().relate("p", "p1", "d.owner");
    assert_eq!(users, vec!["p1".to_string()]);
}

fn create_engine_with_adapter() -> Engine {
    let engine = Engine::new();

    let adapter = TestAdapter::new();
    engine
        .adapter()
        .set_role_adapter("test_role", adapter.clone());
    engine
        .adapter()
        .set_org_adapter("test_org", adapter.clone());
    engine
        .adapter()
        .set_rule_adapter("test_rule", adapter.clone());

    engine
}

fn create_proc(workflow: &Workflow) -> (Proc, Arc<Scheduler>) {
    let engine = Engine::new();
    let scher = engine.scher();
    // let env = Arc::new(Enviroment::new());
    // let scher = Arc::new(Scheduler::new(&config));

    let task = scher.create_raw_proc(workflow);
    (task.clone(), scher.clone())
}

fn create_workflow() -> Workflow {
    let text = include_str!("../../examples/basic.yml");
    let workflow = Workflow::from_str(text).unwrap();

    workflow
}

#[derive(Debug, Clone)]
struct TestAdapter {}
impl TestAdapter {
    fn new() -> Self {
        Self {}
    }
}

impl RuleAdapter for TestAdapter {
    fn ord(&self, _name: &str, acts: &Vec<String>) -> crate::ActResult<Vec<String>> {
        let mut ret = acts.clone();
        ret.reverse();

        Ok(ret)
    }

    fn some(
        &self,
        _name: &str,
        _step: &crate::Step,
        _ctx: &crate::Context,
    ) -> crate::ActResult<bool> {
        Ok(true)
    }
}

impl OrgAdapter for TestAdapter {
    fn dept(&self, _name: &str) -> Vec<String> {
        vec!["u1".to_string(), "u2".to_string()]
    }

    fn unit(&self, _name: &str) -> Vec<String> {
        vec![
            "u1".to_string(),
            "u2".to_string(),
            "u3".to_string(),
            "u4".to_string(),
        ]
    }

    fn relate(&self, _t: &str, _id: &str, _r: &str) -> Vec<String> {
        vec!["p1".to_string()]
    }
}

impl RoleAdapter for TestAdapter {
    fn role(&self, _name: &str) -> Vec<String> {
        vec!["a1".to_string()]
    }
}

// impl ActAdapter for TestAdapter {}
