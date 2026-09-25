//! The connection to an acts-server, and the shape of the errors it answers
//! with. Every failure here is an [`anyhow::Error`] that names what the user
//! asked for, so the REPL prints one readable line instead of panicking on a
//! missing payload or an unreachable server.

use crate::{session, util};
use acts_channel::{ActsChannel, Vars};
use serde_json::Value;

/// Connect to the server at `url`, presenting `token` on every request.
pub async fn connect(url: &str, token: Option<String>) -> anyhow::Result<ActsChannel> {
    match ActsChannel::connect_with_token(url, token).await {
        Ok(client) => Ok(client),
        Err(err) => Err(anyhow::anyhow!("failed to connect to {url}: {err}")),
    }
}

/// Open a connection to `url` and resolve an identity for it, using
/// everything the caller (and the machine) has: an explicit `--token`, a
/// stored session for this server, or the `user`/`password` to log in with.
///
/// The order matters:
/// 1. an explicit token is used as given — it is the caller's, not ours to
///    second-guess;
/// 2. otherwise a stored session for exactly this server is reused, and an
///    expired access token is rotated from its refresh token on the first
///    request;
/// 3. a session that cannot authenticate and a user that can log in falls
///    back to `acl:login`, whose answer is stored for the next run.
///
/// Returns the connection and the `acl:whoami` identity it resolved.
pub async fn connect_and_authenticate(
    url: &str,
    token: Option<String>,
    user: Option<String>,
    password: Option<String>,
) -> anyhow::Result<(ActsChannel, Value)> {
    let explicit = token.is_some();
    let mut client = match token {
        Some(token) => connect(url, Some(token)).await?,
        None => match session::load(url) {
            Some(stored) => ActsChannel::connect_with_session(url, stored.tokens)
                .await
                .map_err(|err| action_failed("connect", err))?,
            None => connect(url, None).await?,
        },
    };

    // Resolve the identity right away: a session that no longer authenticates
    // is repaired by a login here, not by a surprise on the first command.
    // `Value::Null` in the answer means "no session": the caller is anonymous.
    match whoami(&mut client).await {
        Ok(who) if is_authenticated(&who) => Ok((client, who)),
        Ok(_) if explicit => Ok((client, Value::Null)),
        Ok(_) | Err(_) => {
            // no usable session: log in when we can, otherwise stay anonymous
            // (the server answers the catalogue reads and refuses the rest)
            let Some(user) = user else {
                let who = if explicit {
                    Value::Null
                } else {
                    whoami(&mut client).await.unwrap_or(Value::Null)
                };
                return Ok((client, who));
            };
            let password = match password {
                Some(password) => password,
                None => util::prompt_password()?,
            };
            let tokens = client
                .login(&user, &password)
                .await
                .map_err(|err| action_failed("acl:login", err))?;
            session::save(url, &user, &tokens)?;
            let who = whoami(&mut client).await?;
            Ok((client, who))
        }
    }
}

/// Whether a `acl:whoami` answer describes a logged-in caller.
fn is_authenticated(who: &Value) -> bool {
    who["authenticated"].as_bool().unwrap_or(true) && !who.is_null()
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
/// greeting prints: the user, marked `(unrestricted)` for an administrator,
/// or `anonymous` for a caller that has not logged in.
pub fn identity(who: &Value) -> String {
    if who.is_null() {
        return "anonymous".to_string();
    }
    let user = who["user"]
        .as_str()
        .or_else(|| who["subject"].as_str())
        .unwrap_or("?");
    if !who["authenticated"].as_bool().unwrap_or(true) {
        return "anonymous".to_string();
    }
    if who["unrestricted"].as_bool().unwrap_or(false) {
        format!("{user} (unrestricted)")
    } else {
        user.to_string()
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
    fn identity_marks_the_unrestricted_principal() {
        let who =
            json!({"user": "root", "subject": "root", "authenticated": true, "unrestricted": true});
        assert_eq!(identity(&who), "root (unrestricted)");
    }

    #[test]
    fn identity_names_an_authenticated_user() {
        let who = json!({"user": "op", "authenticated": true, "unrestricted": false});
        assert_eq!(identity(&who), "op");
    }

    /// An answer with no `user` still names the caller it arrived for.
    #[test]
    fn identity_falls_back_to_the_subject() {
        let who = json!({"subject": "u1", "authenticated": true});
        assert_eq!(identity(&who), "u1");
    }

    /// A session that resolved to no user is the anonymous caller, and the
    /// greeting says so rather than reading like a name.
    #[test]
    fn identity_names_the_anonymous_caller() {
        let who = json!({"user": "anonymous", "authenticated": false, "unrestricted": false});
        assert_eq!(identity(&who), "anonymous");

        // a refusal with nothing else in it reads the same way
        let who = json!({"authenticated": false});
        assert_eq!(identity(&who), "anonymous");
    }

    /// A server that answers nothing at all must not be able to crash the
    /// startup greeting: a null answer is the anonymous caller, not a name.
    #[test]
    fn identity_of_an_empty_answer_is_anonymous() {
        assert_eq!(identity(&Value::Null), "anonymous");
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
