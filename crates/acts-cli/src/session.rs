//! The session this CLI keeps between runs.
//!
//! A login (`auth login`) writes `session.json` under the config directory —
//! `$ACTS_CONFIG_DIR`, else `$HOME/.acts` (`$USERPROFILE` on Windows), else
//! `./.acts`. The next run of the CLI against the same server picks it up, so
//! the token does not have to be pasted into every invocation; an expired
//! access token is refreshed from the stored refresh token, and a session
//! whose refresh token is spent is discarded.
//!
//! The file holds credentials: it is written with owner-only permissions on
//! unix, and its existence is the reason `auth logout` removes it.

use acts_channel::SessionTokens;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Environment variable naming the directory the CLI keeps its session in.
pub const CONFIG_DIR_ENV: &str = "ACTS_CONFIG_DIR";
const DEFAULT_DIR: &str = ".acts";
const SESSION_FILE: &str = "session.json";

/// One stored session: which server it is for, who logged in, and the tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSession {
    /// The server the tokens were issued by (`host:port`), so a session is
    /// never presented to a different one.
    pub server: String,
    /// The user that logged in.
    pub user: String,
    #[serde(flatten)]
    pub tokens: SessionTokens,
}

/// The directory the CLI keeps its state in.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV) {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join(DEFAULT_DIR))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DIR))
}

fn session_path(dir: &Path) -> PathBuf {
    dir.join(SESSION_FILE)
}

/// The session stored for `server`, if one is and its refresh token has not
/// expired. A spent session is removed on the way out.
pub fn load(server: &str) -> Option<StoredSession> {
    load_from(&config_dir(), server)
}

pub fn load_from(dir: &Path, server: &str) -> Option<StoredSession> {
    let path = session_path(dir);
    let text = std::fs::read_to_string(&path).ok()?;
    let session: StoredSession = serde_json::from_str(&text).ok()?;
    if session.server != server || session.tokens.is_refresh_expired() {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    Some(session)
}

/// Store `session` for `server`.
pub fn save(server: &str, user: &str, tokens: &SessionTokens) -> std::io::Result<()> {
    save_to(&config_dir(), server, user, tokens)
}

pub fn save_to(
    dir: &Path,
    server: &str,
    user: &str,
    tokens: &SessionTokens,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = session_path(dir);
    let session = StoredSession {
        server: server.to_string(),
        user: user.to_string(),
        tokens: tokens.clone(),
    };
    let text = serde_json::to_string_pretty(&session)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    std::fs::write(&path, text)?;
    restrict(&path);
    Ok(())
}

/// Forget the stored session.
pub fn clear(server: &str) -> std::io::Result<()> {
    clear_from(&config_dir(), server)
}

pub fn clear_from(dir: &Path, server: &str) -> std::io::Result<()> {
    let path = session_path(dir);
    if !path.exists() {
        return Ok(());
    }
    let same_server = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<StoredSession>(&text).ok())
        .map(|session| session.server == server)
        .unwrap_or(false);
    if same_server {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Owner-only permissions on unix; Windows has no equivalent here, which is
/// why the file's location (a per-user directory) is the protection there.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(expires_at: i64) -> SessionTokens {
        SessionTokens {
            token: "at_x".to_string(),
            refresh_token: "rt_x".to_string(),
            expires_at,
            refresh_expires_at: expires_at,
        }
    }

    #[test]
    fn round_trips_a_session_and_scopes_it_to_its_server() {
        let dir = std::env::temp_dir().join(format!("acts_cli_session_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let far = chrono::Utc::now().timestamp_millis() + 60_000;

        save_to(&dir, "127.0.0.1:10080", "admin", &tokens(far)).unwrap();
        let loaded = load_from(&dir, "127.0.0.1:10080").expect("stored session loads");
        assert_eq!(loaded.user, "admin");
        assert_eq!(loaded.tokens.token, "at_x");

        // another server must not be handed this session
        assert!(load_from(&dir, "127.0.0.1:10081").is_none());
        // ...and asking for it removed it
        assert!(!session_path(&dir).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_expired_session_is_discarded() {
        let dir = std::env::temp_dir().join(format!("acts_cli_session_exp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let past = chrono::Utc::now().timestamp_millis() - 1;

        save_to(&dir, "srv", "admin", &tokens(past)).unwrap();
        assert!(load_from(&dir, "srv").is_none());
        assert!(!session_path(&dir).exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_only_removes_another_servers_file_never() {
        let dir = std::env::temp_dir().join(format!("acts_cli_session_clr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let far = chrono::Utc::now().timestamp_millis() + 60_000;

        save_to(&dir, "srv", "admin", &tokens(far)).unwrap();
        clear_from(&dir, "other").unwrap();
        assert!(load_from(&dir, "srv").is_some());
        clear_from(&dir, "srv").unwrap();
        assert!(load_from(&dir, "srv").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }
}
