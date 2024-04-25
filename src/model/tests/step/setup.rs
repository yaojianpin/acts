use crate::{Act, Catch, Step, StmtBuild, Timeout, Vars, Workflow};

#[test]
fn model_step_setup_one() {
    let step = Step::new()
        .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 5).with("b", "str"))));
    assert_eq!(step.setup.len(), 1);
}

#[test]
fn model_step_setup_many_same() {
    let step = Step::new().with_setup(|stmts| {
        stmts
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
    });
    assert_eq!(step.setup.len(), 2);
}

#[test]
fn model_step_setup_many_diff() {
    let step = Step::new().with_setup(|stmts| {
        stmts
            .add(Act::set(Vars::new().with("a", 5).with("b", "str")))
            .add(Act::msg(|m| m.with_id("msg1")))
    });
    assert_eq!(step.setup.len(), 2);
}

#[test]
fn model_step_setup_set() {
    let step = Step::new()
        .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 5).with("b", "str"))));

    if let Some(Act::Set(vars)) = step.setup.get(0) {
        assert_eq!(vars.get::<i32>("a").unwrap(), 5);
        assert_eq!(vars.get::<String>("b").unwrap(), "str");
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_expose() {
    let step = Step::new()
        .with_setup(|stmts| stmts.add(Act::expose(Vars::new().with("a", 5).with("b", "str"))));

    if let Some(Act::Expose(vars)) = step.setup.get(0) {
        assert_eq!(vars.get::<i32>("a").unwrap(), 5);
        assert_eq!(vars.get::<String>("b").unwrap(), "str");
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_if() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::r#if(|c| {
            c.with_on("cond")
                .with_then(|stmts| stmts.add(Act::msg(|m| m.with_id("msg1"))))
        }))
    });

    if let Some(Act::If(c)) = step.setup.get(0) {
        assert_eq!(c.on, "cond");
        assert_eq!(c.then.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_msg() {
    let step = Step::new().with_setup(|stmts| stmts.add(Act::msg(|m| m.with_id("msg1"))));

    if let Some(Act::Msg(m)) = step.setup.get(0) {
        assert_eq!(m.id, "msg1");
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_act() {
    let step = Step::new().with_setup(|stmts| stmts.add(Act::req(|a| a.with_id("act1"))));

    if let Some(Act::Req(a)) = step.setup.get(0) {
        assert_eq!(a.id, "act1");
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_each() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::each(|a| {
            a.with_in(r#"["a", "b"]"#)
                .with_run(|stmts| stmts.add(Act::msg(|m| m.with_id("msg1"))))
        }))
    });

    if let Some(Act::Each(each)) = step.setup.get(0) {
        assert_eq!(each.r#in, r#"["a", "b"]"#);
        assert_eq!(each.run.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_on_created() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_created(|stmts| {
            stmts.add(Act::msg(|m| m.with_id("msg1")))
        }))
    });

    if let Some(Act::OnCreated(stmts)) = step.setup.get(0) {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_on_completed() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_completed(|stmts| {
            stmts.add(Act::msg(|m| m.with_id("msg1")))
        }))
    });

    if let Some(Act::OnCompleted(stmts)) = step.setup.get(0) {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_on_updated() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_updated(|stmts| {
            stmts.add(Act::msg(|m| m.with_id("msg1")))
        }))
    });

    if let Some(Act::OnUpdated(stmts)) = step.setup.get(0) {
        assert_eq!(stmts.len(), 1);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_on_error_catch() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_error_catch(|stmts| {
            stmts.add(Catch::new().with_err("err1"))
        }))
    });

    if let Some(Act::OnErrorCatch(stmts)) = step.setup.get(0) {
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts.get(0).unwrap().err.as_ref().unwrap(), "err1");
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_on_timeout() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_timeout(|stmts| {
            stmts.add(Timeout::new().with_on("2h"))
        }))
    });

    if let Some(Act::OnTimeout(stmts)) = step.setup.get(0) {
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts.get(0).unwrap().on.value, 2);
    } else {
        assert!(false);
    }
}

#[test]
fn model_step_setup_yml_parse() {
    let text = r#"
    name: workflow
    id: m1
    steps:
        - id: act1
          setup:
            - !set
              users: ["a", "b"]
            - !each
              in: $("users")
              run:
                - !req
                  id: act2
            - !on_created
              - !msg
                id: msg1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.get(0).unwrap();
    assert_eq!(step.setup.len(), 3);

    if let Act::Set(vars) = step.setup.get(0).unwrap() {
        assert_eq!(vars.get::<Vec<String>>("users").unwrap(), ["a", "b"]);
    }
    if let Act::Each(each) = step.setup.get(1).unwrap() {
        assert_eq!(each.r#in, r#"$("users")"#);
        assert_eq!(each.run.len(), 1);
    }
    if let Act::OnCreated(stmts) = step.setup.get(2).unwrap() {
        assert_eq!(stmts.len(), 1);
    }
}
