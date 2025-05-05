use crate::{Act, ActEvent, StmtBuild, Vars, Workflow};

#[test]
fn model_act_setup_one() {
    let act = Act::new()
        .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 5).with("b", "str"))));
    assert_eq!(act.setup.len(), 1);
}

#[test]
fn model_act_setup_many_same() {
    let act = Act::new().with_setup(|stmts| {
        stmts
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
    });
    assert_eq!(act.setup.len(), 2);
}

#[test]
fn model_act_setup_many_diff() {
    let act = Act::new().with_setup(|stmts| {
        stmts
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
            .add(Act::msg(|m| m.with_key("msg1")))
    });
    assert_eq!(act.setup.len(), 2);
}

#[test]
fn model_act_setup_set() {
    let act = Act::new()
        .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 5).with("b", "str"))));

    if let Some(Act { params, .. }) = act.setup.first() {
        let vars: Vars = params.clone().into();
        assert_eq!(vars.get::<i32>("a").unwrap(), 5);
        assert_eq!(vars.get::<String>("b").unwrap(), "str");
    } else {
        panic!();
    }
}

#[test]
fn model_act_setup_if() {
    let act =
        Act::new().with_setup(|stmts| stmts.add(Act::irq(|c| c.with_if("cond").with_key("msg1"))));

    if let Some(Act {
        uses, r#if, key, ..
    }) = act.setup.first()
    {
        assert_eq!(uses, "acts.core.irq");
        assert_eq!(r#if.clone().unwrap(), "cond");
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_setup_msg() {
    let act = Act::new().with_setup(|stmts| stmts.add(Act::msg(|m| m.with_key("msg1"))));

    if let Some(Act { key, .. }) = act.setup.first() {
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_setup_act() {
    let act = Act::new().with_setup(|stmts| stmts.add(Act::irq(|a| a.with_key("act1"))));

    if let Some(Act { uses, key, .. }) = act.setup.first() {
        assert_eq!(uses, "acts.core.irq");
        assert_eq!(key, "act1");
    } else {
        panic!();
    }
}

#[test]
fn model_act_setup_yml_parse() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: step1
          acts:
            - uses: acts.core.irq
              setup:
                  - uses: acts.core.block
                    on: completed
                    params:
                        mode: sequence
                        acts:
                          - uses: acts.transform.set
                            params:
                                users: ["a", "b"]
                          - uses: acts.transform.parallel
                            params:
                                in: $("users")
                                acts:
                                    - uses: acts.core.irq
                                      key: act2
                  - uses: acts.core.msg
                    on: created
                    key: msg1

    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.acts.len(), 1);

    let act = step.acts.first().unwrap();
    if let Some(Act { on, params, .. }) = act.setup.first() {
        assert_eq!(on.unwrap(), ActEvent::Completed);

        let params: Vars = params.clone().into();
        assert_eq!(params.get::<String>("mode").unwrap(), "sequence");
        assert_eq!(params.get::<Vec<Act>>("acts").unwrap().len(), 2);
    }
    if let Some(Act { on, key, .. }) = act.setup.get(1) {
        assert_eq!(on.unwrap(), ActEvent::Created);
        assert_eq!(key, "msg1");
    }
}
