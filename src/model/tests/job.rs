use crate::Job;
use serde_json::json;

#[test]
fn model_job_id() {
    let job = Job::new().with_id("job1");
    assert_eq!(job.id, "job1");
}

#[test]
fn model_job_name() {
    let job = Job::new().with_name("job name");
    assert_eq!(job.name, "job name");
}

#[test]
fn model_job_inputs() {
    let job = Job::new().with_input("p1", json!(5));
    assert_eq!(job.inputs.len(), 1);
    assert_eq!(job.inputs.get("p1"), Some(&json!(5)));
}

#[test]
fn model_job_outputs() {
    let job = Job::new().with_output("p1", json!(5));
    assert_eq!(job.outputs.len(), 1);
    assert!(job.outputs.get("p1").is_some());
}

#[test]
fn model_job_needs() {
    let job = Job::new().with_need("job1");
    assert_eq!(job.needs.len(), 1);
}

#[test]
fn model_job_tag() {
    let job = Job::new().with_tag("tag1");
    assert_eq!(job.tag, "tag1");
}
