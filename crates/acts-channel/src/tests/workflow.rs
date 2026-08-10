use super::start_server;
use crate::{
    ActsChannel, Vars,
    model::{ModelInfo, Package, PageData},
    tests::SERVER_ADDR,
};
use tokio::sync::oneshot;

#[tokio::test]
async fn workflow_deploy() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", SERVER_ADDR);

    let mut client = ActsChannel::connect(&url).await.unwrap();
    let yml = r"
    id: test
    name: test
    ver: '1.0'
    steps:
    - name: act1
    ";
    let ret = client.deploy(yml, Some("1")).await.unwrap();

    assert!(ret.data.unwrap());
    tx.send(()).unwrap();
}

#[tokio::test]
async fn workflow_publish() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", SERVER_ADDR);

    let mut client = ActsChannel::connect(&url).await.unwrap();
    let yml = r#"
    id: test_package
    name: test package
    desc: test package description
    icon: test-icon
    doc: test doc
    version: "0.1.0"
    schema: '{}'
    options: '{}'
    run_as: func
    resources: []
    catalog: app
    "#;

    let package = serde_yaml::from_str::<Package>(yml).unwrap();
    let ret = client.publish(&package).await.unwrap();

    assert!(ret.data.unwrap());
    tx.send(()).unwrap();
}

#[tokio::test]
async fn workflow_start() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", SERVER_ADDR);

    let mut client = ActsChannel::connect(&url).await.unwrap();
    let model = r#"
    id: test
    name: test
    ver: '1.0'
    steps:
    - name: act1
    "#;
    client.deploy(model, Some("1")).await.unwrap();

    let resp = client
        .start("1", Vars::new().with("pid", "123"))
        .await
        .unwrap();
    assert_eq!(resp.data.unwrap(), "123");
    tx.send(()).unwrap();
}

#[tokio::test]
async fn workflow_models() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", SERVER_ADDR);

    let mut client = ActsChannel::connect(&url).await.unwrap();
    let model = r#"
    id: test
    name: test
    ver: '1.0'
    steps:
    - name: act1
    "#;
    client.deploy(model, Some("1")).await.unwrap();

    let ret = client
        .send::<PageData<ModelInfo>>("model:ls", Vars::new())
        .await
        .unwrap();
    assert!(ret.data.is_some());
    tx.send(()).unwrap();
}
