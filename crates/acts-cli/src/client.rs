//! The connection to an acts-server, and the shape of the errors it answers
//! with. Every failure here is an [`anyhow::Error`] that names what the user
//! asked for, so the REPL prints one readable line instead of panicking on a
//! missing payload or an unreachable server.

use acts_channel::{ActsChannel, Vars};
use serde_json::Value;

/// Connect to the server at `url`, presenting `token` on every request.
pub async fn connect(url: &str, token: Option<String>) -> anyhow::Result<ActsChannel> {
    match ActsChannel::connect_with_token(url, token).await {
        Ok(client) => Ok(client),
        Err(err) => Err(anyhow::anyhow!("failed to connect to {url}: {err}")),
    }
}

/// Report the identity the server resolved for this connection
/// (`acl:whoami`), so a missing or stale token fails at startup instead of on
/// the first command. A server without an ACL answers with the unrestricted
/// principal; a server with one refuses the request when no acceptable token
/// was presented.
pub async fn whoami(client: &mut ActsChannel) -> anyhow::Result<Value> {
    let ret = client
        .send::<Value>("acl:whoami", Vars::new())
        .await
        .map_err(|err| action_failed("acl:whoami", err))?;
    // the action answers with the identity object itself; an empty payload is
    // reported by `identity` as `subject ?` rather than crashing the startup
    Ok(ret.data.unwrap_or(Value::Null))
}

/// The identity `acl:whoami` answered with, as the one line the startup
/// greeting prints: `unrestricted`, the caller's roles, or its subject.
pub fn identity(who: &Value) -> String {
    let subject = who["subject"].as_str().unwrap_or("?");
    let roles = who["roles"]
        .as_array()
        .map(|roles| {
            roles
                .iter()
                .filter_map(|role| role.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if who["unrestricted"].as_bool().unwrap_or(false) {
        "unrestricted".to_string()
    } else if roles.is_empty() {
        format!("subject {subject}")
    } else {
        format!("roles: {roles}")
    }
}

/// The error a failed round trip is reported as: the action the user typed,
/// then what the server answered. A `tonic::Status` displays its code and its
/// message, so a refusal (`not_found`, `permission_denied`) reads on one line
/// while a transport fault still says what broke.
pub fn action_failed(name: &str, err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("action '{name}' failed: {err}")
}

/// The payload of an action that answers with data. An action that promised a
/// payload and answered without one is a protocol error — reported, never a
/// panic that would take the REPL down mid-session.
pub fn payload<T>(action: &str, data: Option<T>) -> anyhow::Result<T> {
    data.ok_or_else(|| anyhow::anyhow!("action '{action}' returned no payload"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_names_the_unrestricted_principal() {
        let who = json!({"subject": "root", "roles": [], "unrestricted": true});
        assert_eq!(identity(&who), "unrestricted");
    }

    #[test]
    fn identity_joins_the_roles() {
        let who = json!({"subject": "op", "roles": ["operator", "reader"], "unrestricted": false});
        assert_eq!(identity(&who), "roles: operator, reader");
    }

    #[test]
    fn identity_falls_back_to_the_subject() {
        let who = json!({"subject": "u1", "roles": []});
        assert_eq!(identity(&who), "subject u1");
    }

    /// A server that answers nothing at all must not be able to crash the
    /// startup greeting.
    #[test]
    fn identity_of_an_empty_answer_is_a_placeholder() {
        assert_eq!(identity(&Value::Null), "subject ?");
    }

    #[test]
    fn a_missing_payload_names_the_action() {
        let err = payload("model:get", None::<i32>).unwrap_err();
        assert_eq!(err.to_string(), "action 'model:get' returned no payload");
    }

    #[test]
    fn a_present_payload_is_returned() {
        assert_eq!(payload("model:get", Some(7)).unwrap(), 7);
    }

    #[test]
    fn a_failed_action_names_the_action_and_the_status() {
        let err = action_failed("proc:start", "code: 'not_found', message: \"no model 'x'\"");
        assert_eq!(
            err.to_string(),
            "action 'proc:start' failed: code: 'not_found', message: \"no model 'x'\""
        );
    }
}
