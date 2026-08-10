use crate::{
    ActsChannel,
    tests::{SERVER_ADDR, start_server},
};
use tokio::sync::oneshot;

#[tokio::test]
async fn grpc_client_connect() {
    let (tx, rx) = oneshot::channel();
    let port = start_server(rx).await;
    let url = format!("http://{}:{port}", SERVER_ADDR);

    let client = ActsChannel::connect(&url).await;
    assert!(client.is_ok());
    tx.send(()).unwrap();
}
