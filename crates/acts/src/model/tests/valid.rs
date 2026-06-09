use crate::Workflow;

#[test]
fn model_valid_step_id() {
    let m = Workflow::new()
        .with_step(|step| step.with_id("step1"))
        .with_step(|step| step.with_id("step1"));
    assert!(m.valid().is_err());
}

#[test]
fn model_valid_act_id() {
    // Test that duplicate act IDs in catches cause validation error
    let m = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_id("act1"))
            .with_catch(|step| step.with_id("act1"))
    });

    assert!(m.valid().is_err());
}

#[test]
fn model_valid_same_tag() {
    let m = Workflow::new().with_step(|step| {
        step.with_id("step1")
            .with_catch(|step| step.with_tag("tag1"))
            .with_catch(|step| step.with_tag("tag1"))
    });
    assert!(m.valid().is_ok());
}
