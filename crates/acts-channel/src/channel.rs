use crate::{
    ActionResult, Message, MessageOptions, Vars,
    acts_service_client::*,
    model::{self, Package},
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tonic::{
    Code, Request, Status,
    transport::{Channel, Endpoint},
};

#[derive(Debug, Clone, Default)]
pub struct ActsOptions {
    pub r#type: Option<String>,
    pub state: Option<String>,
    pub uses: Option<String>,
    /// custom key-value filtering options (e.g. {"tag": "xxx"})
    pub options: std::collections::HashMap<String, String>,
    /// auto ack message when receiving by client
    pub ack: Option<bool>,
}

/// The authenticated transport the client speaks over: the token interceptor
/// is part of the type, so no request can be built without it.
pub type AuthChannel = tonic::service::interceptor::InterceptedService<Channel, Auth>;

#[derive(Debug, Clone)]
pub struct ActsChannel {
    client: ActsServiceClient<AuthChannel>,
    /// the endpoint this channel was opened against, for a caller that keys
    /// state (a stored session, a log line) by server
    url: String,
    /// the same handle the interceptor reads: swapping the token here applies
    /// to every following request, including the ones opened by clones
    auth: Auth,
    auto_ack: bool,
    /// the refresh half of the session in hand (with `expires_at`), kept so
    /// an expired access token is rotated and retried instead of surfacing
    session: Option<SessionTokens>,
}

/// A logged-in session: the access token to present, the refresh token that
/// rotates it, and both expiries (millis since epoch).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionTokens {
    pub token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub refresh_expires_at: i64,
}

/// The payload `acl:login` and `acl:refresh` answer with (declared here so
/// the client crate keeps no dependency on the engine).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoginTokens {
    token: String,
    refresh_token: String,
    expires_in: i64,
    refresh_expires_in: i64,
}

impl SessionTokens {
    /// Stamp `acl:login`/`acl:refresh`'s answer with the wall clock, so a
    /// persisted session can tell an expired token from a live one. The
    /// payload it reads is this module's own decoding, not part of the API.
    fn from_login(tokens: &LoginTokens) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            token: tokens.token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: now + tokens.expires_in * 1000,
            refresh_expires_at: now + tokens.refresh_expires_in * 1000,
        }
    }

    /// Whether the access token is past its expiry (a cheap guard: the server
    /// is authoritative, and a false negative just costs one refused request
    /// that `refresh` then repairs).
    pub fn is_expired(&self) -> bool {
        self.expires_at <= chrono::Utc::now().timestamp_millis()
    }

    /// Whether the refresh token is past its expiry: such a session cannot be
    /// revived and needs a fresh login.
    pub fn is_refresh_expired(&self) -> bool {
        self.refresh_expires_at <= chrono::Utc::now().timestamp_millis()
    }
}

/// Attaches the caller's ACL token to every request as an
/// `authorization: Bearer …` metadata entry.
///
/// A client interceptor covers the unary actions and the `on_message` stream
/// alike, so no call path can silently omit the credential.
#[derive(Debug, Clone, Default)]
pub struct Auth {
    token: Arc<RwLock<Option<String>>>,
}

impl Auth {
    /// An interceptor that presents `token` (or nothing when it is `None`).
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: Arc::new(RwLock::new(token)),
        }
    }

    /// The token presented on the next request.
    pub fn token(&self) -> Option<String> {
        self.token.read().clone()
    }

    /// Replace the token. A `None` clears it (the caller is anonymous again).
    pub fn set_token(&self, token: Option<String>) {
        *self.token.write() = token;
    }
}

impl tonic::service::Interceptor for Auth {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        if let Some(token) = self.token.read().clone() {
            let value = format!("Bearer {token}")
                .parse()
                .map_err(|_| Status::invalid_argument("invalid acl token"))?;
            request.metadata_mut().insert("authorization", value);
        }
        Ok(request)
    }
}

impl ActsChannel {
    /// Connect without a credential: the anonymous catalogue-only caller.
    /// Log in ([`ActsChannel::login`]) to do more.
    pub async fn connect(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::connect_with_token(url, None).await
    }

    /// Connect and present `token` on every request.
    pub async fn connect_with_token(
        url: &str,
        token: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let addr = Endpoint::from_str(url)?;
        let channel = addr.connect().await?;
        let auth = Auth::new(token);
        let client = ActsServiceClient::with_interceptor(channel, auth.clone());
        Ok(Self {
            client,
            url: url.to_string(),
            auth,
            auto_ack: true,
            session: None,
        })
    }

    /// Connect with a session in hand (a token and its refresh token, as
    /// `acl:login` answered and a caller may have persisted): the token rides
    /// every request, and an expired one is refreshed and retried through
    /// `refresh_token`.
    pub async fn connect_with_session(
        url: &str,
        session: SessionTokens,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut chan = Self::connect_with_token(url, Some(session.token.clone())).await?;
        chan.session = Some(session);
        Ok(chan)
    }

    /// Connect and log in as `user`. The credentials never become part of the
    /// transport: only the session `acl:login` answers with does.
    pub async fn connect_with_password(
        url: &str,
        user: &str,
        password: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut chan = Self::connect(url).await?;
        chan.login(user, password).await?;
        Ok(chan)
    }

    /// The endpoint this channel was opened against.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The session in hand, for a caller that persists it.
    pub fn session(&self) -> Option<SessionTokens> {
        self.session.clone()
    }

    /// The access token presented on the next request.
    pub fn token(&self) -> Option<String> {
        self.auth.token()
    }

    /// Log in as `user`/`password` (`acl:login`) and present the session on
    /// every following request.
    pub async fn login(&mut self, user: &str, password: &str) -> Result<SessionTokens, Status> {
        let ret: ActionResult<LoginTokens> = self
            .raw_send(
                "acl:login",
                Vars::new().with("user", user).with("password", password),
                None,
            )
            .await?;
        let tokens = ret
            .data
            .ok_or_else(|| Status::internal("acl:login returned no payload"))?;
        let session = SessionTokens::from_login(&tokens);
        self.auth.set_token(Some(session.token.clone()));
        self.session = Some(session.clone());
        Ok(session)
    }

    /// Rotate the session with its refresh token (`acl:refresh`). Called
    /// automatically when a request is refused as unauthenticated and a
    /// refresh token is in hand.
    pub async fn refresh(&mut self) -> Result<SessionTokens, Status> {
        let refresh_token = self
            .session
            .as_ref()
            .map(|session| session.refresh_token.clone())
            .ok_or_else(|| Status::unauthenticated("no refresh token in hand"))?;
        let ret: ActionResult<LoginTokens> = self
            .raw_send(
                "acl:refresh",
                Vars::new().with("refresh_token", refresh_token),
                None,
            )
            .await?;
        let tokens = ret
            .data
            .ok_or_else(|| Status::internal("acl:refresh returned no payload"))?;
        let session = SessionTokens::from_login(&tokens);
        self.auth.set_token(Some(session.token.clone()));
        self.session = Some(session.clone());
        Ok(session)
    }

    /// Revoke the session in hand (`acl:logout`) and clear it.
    pub async fn logout(&mut self) -> Result<bool, Status> {
        let token = match self.auth.token() {
            Some(token) => token,
            None => return Ok(false),
        };
        let ret: ActionResult<bool> = self
            .raw_send("acl:logout", Vars::new().with("token", token), None)
            .await?;
        self.auth.set_token(None);
        self.session = None;
        Ok(ret.data.unwrap_or(false))
    }

    /// Subscribe to the server messages matching `options`; decoded messages
    /// are handed to `on_message` in arrival order.
    ///
    /// Faults that do not end the subscription — a payload that fails to
    /// decode, a failed auto-ack — are reported to `on_error`. A failure to
    /// establish the subscription RPC is returned here, and the end of the
    /// feed itself is reported by [`Subscription::wait`], so a dropped feed
    /// never passes unnoticed.
    pub async fn subscribe<F, E>(
        &mut self,
        client_id: &str,
        on_message: F,
        on_error: E,
        options: &ActsOptions,
    ) -> Result<Subscription, Status>
    where
        F: FnMut(&model::Message) + Send + Sync + 'static,
        E: Fn(SubscriptionError) + Send + Sync + 'static,
    {
        let mut client = self.client.clone();
        if let Some(auto_ack) = options.ack {
            self.auto_ack = auto_ack;
        }
        self.on_message(&mut client, client_id, on_message, on_error, options)
            .await
    }

    pub async fn deploy(
        &mut self,
        model: &str,
        mid: Option<&str>,
    ) -> Result<ActionResult<bool>, Status> {
        let mut options = Vars::new();
        options.set("model", model.to_string());

        if let Some(mid) = mid {
            options.set("mid", mid.to_string());
        }

        self.send("model:deploy", options).await
    }

    pub async fn publish(&mut self, package: &Package) -> Result<ActionResult<bool>, Status> {
        let options = Vars::new()
            .with("id", package.id.clone())
            .with("desc", package.desc.clone())
            .with("icon", package.icon.clone())
            .with("doc", package.doc.clone())
            .with("version", package.version.clone())
            .with("schema", package.schema.clone())
            .with("options", package.options.clone())
            .with("run_as", package.run_as.clone())
            .with("resources", package.resources.clone())
            .with("catalog", package.catalog.clone());

        self.send("pack:publish", options).await
    }
    /// Update or insert one snapshot value on the server (feed write). An
    /// unknown snapshot target is auto-registered on the server with default
    /// options (`snap:upsert`), so feeding a name that is missing from the
    /// server config still lands; scope key and revision are supplied by the
    /// caller, and a revision not newer than the server's cached one for that
    /// scope is ignored.
    pub async fn upsert_snapshot(
        &mut self,
        name: &str,
        scope: &str,
        rev: u64,
        data: Vars,
    ) -> Result<ActionResult<bool>, Status> {
        let options = Vars::new()
            .with("name", name)
            .with("scope", scope)
            .with("rev", rev)
            .with("data", data);
        self.send("snap:upsert", options).await
    }

    /// Remove one snapshot value on the server (tombstone). Fails when the
    /// snapshot target is not registered on the server — unlike
    /// [`upsert_snapshot`](Self::upsert_snapshot), a remove never registers.
    pub async fn remove_snapshot(
        &mut self,
        name: &str,
        scope: &str,
    ) -> Result<ActionResult<bool>, Status> {
        let options = Vars::new().with("name", name).with("scope", scope);
        self.send("snap:remove", options).await
    }

    pub async fn start(&mut self, id: &str, vars: Vars) -> Result<ActionResult<String>, Status> {
        let options = Vars::new().with("id", id).extend(&vars);
        let mut ret = ActionResult::<String>::begin();
        let resp = self
            .client
            .send(Request::new(crate::Message {
                name: "proc:start".to_string(),
                seq: crate::create_seq(),
                ack: None,
                data: Some(options.to_bytes()),
            }))
            .await?;

        ret.data = resp
            .into_inner()
            .data
            .as_deref()
            .map(|data| decode_payload::<String>("proc:start", data))
            .transpose()?;
        ret.end()
    }

    pub async fn ack(&mut self, ack_id: &str) -> Result<ActionResult<()>, Status> {
        self.send_with_ack(
            "msg:ack",
            Vars::new().with("id", ack_id),
            Some(ack_id.to_string()),
        )
        .await
    }

    pub async fn send<T>(&mut self, name: &str, data: Vars) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send_with_ack(name, data, None).await
    }

    async fn send_with_ack<T>(
        &mut self,
        name: &str,
        data: Vars,
        ack: Option<String>,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        match self.raw_send(name, data.clone(), ack.clone()).await {
            Ok(ret) => Ok(ret),
            // an expired access token is not a failure the caller has to see:
            // rotate the session with the refresh token and try once more
            Err(status) if status.code() == Code::Unauthenticated && self.session.is_some() => {
                self.refresh().await.map_err(|err| {
                    if err.code() == Code::Unauthenticated {
                        err
                    } else {
                        status.clone()
                    }
                })?;
                self.raw_send(name, data, ack).await
            }
            Err(status) => Err(status),
        }
    }

    /// One request with the credentials currently in hand, no refresh.
    async fn raw_send<T>(
        &mut self,
        name: &str,
        data: Vars,
        ack: Option<String>,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        let mut ret = ActionResult::begin();
        let resp = self
            .client
            .send(Request::new(Message {
                name: name.to_string(),
                seq: crate::create_seq(),
                ack,
                data: Some(data.to_bytes()),
            }))
            .await?;
        ret.data = resp
            .into_inner()
            .data
            .as_deref()
            .map(|data| decode_payload::<T>(name, data))
            .transpose()?;
        ret.end()
    }

    async fn on_message<F, E>(
        &self,
        client: &mut ActsServiceClient<AuthChannel>,
        client_id: &str,
        mut handle: F,
        on_error: E,
        options: &ActsOptions,
    ) -> Result<Subscription, Status>
    where
        F: FnMut(&model::Message) + Send + Sync + 'static,
        E: Fn(SubscriptionError) + Send + Sync + 'static,
    {
        let request = tonic::Request::new(MessageOptions {
            client_id: client_id.to_string(),
            r#type: options.r#type.as_deref().unwrap_or("*").to_string(),
            state: options.state.as_deref().unwrap_or("*").to_string(),
            options: options.options.clone(),
            uses: options.uses.as_deref().unwrap_or("*").to_string(),
        });
        // a failed handshake is the caller's error; the stream itself is read
        // by a task that reports every fault and its end
        let mut stream = client.on_message(request).await?.into_inner();
        let chan = self.clone();
        let auto_ack = self.auto_ack;
        let (closed_tx, closed) = oneshot::channel();
        tokio::spawn(async move {
            let mut chan = chan;
            let mut end = Ok(());
            while let Some(item) = stream.next().await {
                let raw = match item {
                    Ok(m) => m,
                    // a stream status ends the feed: reported by `wait`
                    Err(status) => {
                        end = Err(status);
                        break;
                    }
                };
                let message = match decode_payload::<model::Message>("on_message", raw.data()) {
                    Ok(message) => message,
                    Err(err) => {
                        // a corrupt payload must not end the feed; the message
                        // stays unacked so the server may redeliver it
                        on_error(SubscriptionError::Decode {
                            seq: raw.seq.clone(),
                            source: err.message().to_string(),
                        });
                        continue;
                    }
                };

                if auto_ack && let Err(err) = chan.ack(&raw.seq).await {
                    on_error(SubscriptionError::Ack {
                        seq: raw.seq.clone(),
                        source: err,
                    });
                    continue;
                }
                handle(&message);
            }
            // the receiver is gone when the caller dropped its handle
            closed_tx.send(end).ok();
        });
        Ok(Subscription { closed })
    }

    pub async fn complete<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:complete",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn submit<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:submit",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn fail<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:fail",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn back<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:back",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn cancel<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:cancel",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn skip<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:skip",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn abort<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:remove",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn push<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:push",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }

    pub async fn remove<T>(
        &mut self,
        pid: &str,
        tid: &str,
        data: Vars,
    ) -> Result<ActionResult<T>, Status>
    where
        T: Serialize + DeserializeOwned,
    {
        self.send(
            "act:remove",
            Vars::new().with("pid", pid).with("tid", tid).extend(&data),
        )
        .await
    }
}

/// A fault of a running subscription that does not end its feed.
///
/// Reported to the `on_error` callback of [`ActsChannel::subscribe`]; the end
/// of the feed itself is reported by [`Subscription::wait`].
#[derive(Debug, Clone)]
pub enum SubscriptionError {
    /// A message payload could not be decoded. The message is skipped and left
    /// unacked, so the server may redeliver it.
    Decode { seq: String, source: String },
    /// Acking a message failed. The message was not handed to the message
    /// callback.
    Ack { seq: String, source: Status },
}

impl std::fmt::Display for SubscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode { seq, source } => {
                write!(f, "message '{seq}' skipped: invalid payload: {source}")
            }
            Self::Ack { seq, source } => write!(f, "message '{seq}' dropped: ack failed: {source}"),
        }
    }
}

impl std::error::Error for SubscriptionError {}

/// Handle of a subscription started by [`ActsChannel::subscribe`].
///
/// The feed runs on its own task; [`wait`](Self::wait) reports its end, so a
/// dead subscription cannot pass unnoticed. Dropping the handle leaves the
/// feed running until the server closes it (or the engine unsubscribes it).
#[derive(Debug)]
pub struct Subscription {
    closed: oneshot::Receiver<Result<(), Status>>,
}

impl Subscription {
    /// Wait until the subscription ends: `Ok(())` when the server closed the
    /// stream, `Err(status)` when the stream or the connection failed.
    pub async fn wait(self) -> Result<(), Status> {
        match self.closed.await {
            Ok(end) => end,
            // the feed task was dropped before it could report its end
            Err(_) => Err(Status::cancelled(
                "subscription task ended without a status",
            )),
        }
    }
}

/// Decode a message payload, mapping a malformed body to a `Status` instead of
/// panicking the client.
fn decode_payload<T: DeserializeOwned>(action: &str, data: &[u8]) -> Result<T, Status> {
    serde_json::from_slice(data)
        .map_err(|err| Status::internal(format!("{action}: invalid response payload: {err}")))
}
