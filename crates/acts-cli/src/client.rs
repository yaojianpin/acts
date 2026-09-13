use acts_channel::{ActsChannel, Vars};

pub async fn connect(
    url: &str,
    token: Option<String>,
) -> Result<ActsChannel, Box<dyn std::error::Error>> {
    let client = ActsChannel::connect_with_token(url, token).await?;
    Ok(client)
}

/// Report the identity the server resolved for this connection
/// (`acl:whoami`), so a missing or stale token fails at startup instead of on
/// the first command. A server without an ACL answers with the unrestricted
pub async fn whoami(
    client: &mut ActsChannel,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let ret = client
        .send::<serde_json::Value>("acl:whoami", Vars::new())
        .await?;
    Ok(ret.data.unwrap_or(serde_json::Value::Null))
}
