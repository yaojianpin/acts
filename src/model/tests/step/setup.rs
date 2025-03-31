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
            .add(Act::msg(|m| m.with_key("msg1")))
    });
    assert_eq!(step.setup.len(), 2);
}

#[test]
fn model_step_setup_set() {
    let step = Step::new()
        .with_setup(|stmts| stmts.add(Act::set(Vars::new().with("a", 5).with("b", "str"))));

    if let Some(Act { inputs, .. }) = step.setup.first() {
        assert_eq!(inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(inputs.get::<String>("b").unwrap(), "str");
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_expose() {
    let step = Step::new()
        .with_setup(|stmts| stmts.add(Act::expose(Vars::new().with("a", 5).with("b", "str"))));

    if let Some(Act { inputs, .. }) = step.setup.first() {
        assert_eq!(inputs.get::<i32>("a").unwrap(), 5);
        assert_eq!(inputs.get::<String>("b").unwrap(), "str");
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_if() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::r#if(|c| {
            c.with_on("cond")
                .with_then(|stmts| stmts.add(Act::msg(|m| m.with_key("msg1"))))
        }))
    });

    if let Some(Act { act, on, then, .. }) = step.setup.first() {
        assert_eq!(act, "if");
        assert_eq!(on, "cond");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_msg() {
    let step = Step::new().with_setup(|stmts| stmts.add(Act::msg(|m| m.with_key("msg1"))));

    if let Some(Act { key, .. }) = step.setup.first() {
        assert_eq!(key, "msg1");
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_act() {
    let step = Step::new().with_setup(|stmts| stmts.add(Act::irq(|a| a.with_key("act1"))));

    if let Some(Act { act, key, .. }) = step.setup.first() {
        assert_eq!(act, "irq");
        assert_eq!(key, "act1");
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_each() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::each(|a| {
            a.with_in(r#"["a", "b"]"#)
                .with_then(|stmts| stmts.add(Act::msg(|m| m.with_key("msg1"))))
        }))
    });

    if let Some(Act {
        act, r#in, then, ..
    }) = step.setup.first()
    {
        assert_eq!(act, "each");
        assert_eq!(r#in, r#"["a", "b"]"#);
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_on_created() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_created(|stmts| {
            stmts.add(Act::msg(|m| m.with_key("msg1")))
        }))
    });

    if let Some(Act { then, .. }) = step.setup.first() {
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_on_completed() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_completed(|stmts| {
            stmts.add(Act::msg(|m| m.with_key("msg1")))
        }))
    });

    if let Some(Act { then, .. }) = step.setup.first() {
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_on_updated() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_updated(|stmts| {
            stmts.add(Act::msg(|m| m.with_key("msg1")))
        }))
    });

    if let Some(Act { then, act, .. }) = step.setup.first() {
        assert_eq!(act, "on_updated");
        assert_eq!(then.len(), 1);
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_on_error_catch() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_catch(|stmts| {
            stmts.add(Catch::new().with_on("err1"))
        }))
    });

    if let Some(Act { act, catches, .. }) = step.setup.first() {
        assert_eq!(act, "on_catch");
        assert_eq!(catches.len(), 1);
        assert_eq!(catches[0].on.as_ref().unwrap(), "err1");
    } else {
        panic!();
    }
}

#[test]
fn model_step_setup_on_timeout() {
    let step = Step::new().with_setup(|stmts| {
        stmts.add(Act::on_timeout(|stmts| {
            stmts.add(Timeout::new().with_on("2h"))
        }))
    });

    if let Some(Act { timeout, act, .. }) = step.setup.first() {
        assert_eq!(act, "on_timeout");
        assert_eq!(timeout.len(), 1);

        let timeout = timeout.first().unwrap();
        assert_eq!(timeout.on.value, 2);
    } else {
        panic!();
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
            - act: set
              inputs:
                users: ["a", "b"]
            - act: each
              in: $("users")
              then:
                - act: irq
                  key: act2
            - act: on_created
              then:
                - act: msg
                  key: msg1
    "#;
    let m = Workflow::from_yml(text).unwrap();
    let step = m.steps.first().unwrap();
    assert_eq!(step.setup.len(), 3);

    if let Some(Act { inputs, .. }) = step.setup.first() {
        assert_eq!(inputs.get::<Vec<String>>("users").unwrap(), ["a", "b"]);
    }
    if let Some(Act { r#in, then, .. }) = step.setup.get(1) {
        assert_eq!(r#in, r#"$("users")"#);
        assert_eq!(then.len(), 1);
    }
    if let Some(Act { then, act, .. }) = step.setup.get(2) {
        assert_eq!(act, "on_created");
        assert_eq!(then.len(), 1);
    }
}
