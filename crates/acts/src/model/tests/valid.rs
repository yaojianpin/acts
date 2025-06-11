use crate::{Act, Workflow};

#[test]
fn model_valid_step_id() {
    let m = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step1"));
    assert!(m.valid().is_err());
}

#[test]
fn model_valid_act_id() {
    let m = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_act(Act::irq(|act| act.with_key("key1")).with_id("act1"))
            .with_act(Act::irq(|act| act.with_key("key1")).with_id("act1"))
    });

    assert!(m.valid().is_err());
}

#[test]
fn model_valid_same_tag() {
    let m = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_tag("tag1")
            .with_act(Act::irq(|act| act.with_tag("tag1")))
            .with_act(Act::irq(|act| act.with_tag("tag1")))
    });
    assert!(m.valid().is_ok());
}

// no check in current version
// #[test]
// fn model_valid_stmt_id_in_same_step() {
//     let m = Workflow::new().with_step(|step| {
//         step.with_id("step1").with_setup(|stmts| {
//             stmts
//                 .add(Statement::act(|act| act.with_id("act1")))
//                 .add(Statement::act(|act| act.with_id("act1")))
//         })
//     });
//     assert_eq!(m.valid().is_err(), true);
// }
