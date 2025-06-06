use acts::{EngineBuilder, Result, Vars, Workflow};
use acts_package_http::HttpPackagePlugin;
use mockito::Matcher;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<()> {
    let opts = mockito::ServerOpts {
        host: "0.0.0.0",
        port: 1234,
        ..Default::default()
    };
    let mut server = mockito::Server::new_with_opts(opts);
    server
        .mock("GET", "/hello")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("key1".into(), "1".into()),
            Matcher::UrlEncoded("key2".into(), "2".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("hello")
        .create();

    server
        .mock("POST", "/world")
        .with_body(json!({ "key": 2 }).to_string())
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(json!({ "data": "world"}).to_string())
        .create();

    let engine = EngineBuilder::new()
        .add_plugin(&HttpPackagePlugin)
        .build()
        .await?
        .start();
    let text = include_str!("./model.yml");
    let workflow = Workflow::from_yml(text).unwrap();
    workflow.print();
    let (s, s2, sig) = engine.signal(()).triple();
    let executor = engine.executor().clone();
    engine
        .executor()
        .model()
        .deploy(&workflow)
        .expect("deploy model");
    executor
        .proc()
        .start(&workflow.id, &Vars::new())
        .expect("start workflow");

    engine.channel().on_complete(move |e| {
        println!(
            "on_workflow_complete: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.outputs
        );
        s.close();
    });
    engine.channel().on_error(move |e| {
        println!(
            "on_workflow_error: pid={} cost={}ms outputs={:?}",
            e.pid,
            e.cost(),
            e.inputs
        );
        s2.close();
    });
    sig.recv().await;

    Ok(())
}
