use crate::{Event, Message, Vars, scheduler::Runtime, utils};
use std::sync::Arc;
use tracing::{debug, error, info};

#[derive(Debug, Clone)]
pub struct ChannelOptions {
    pub id: String,

    /// need ack the message
    pub ack: bool,

    /// use the glob pattern to match the message type
    /// eg. {workflow,step,branch,req,msg}
    pub r#type: String,
    /// use the glob pattern to match the message state
    /// eg. {created,completed}
    pub state: String,

    /// use the glob pattern to match the message uses
    pub uses: String,

    /// use the custom glob pattern
    pub options: Vars,
}

impl Default for ChannelOptions {
    fn default() -> Self {
        Self {
            id: utils::shortid(),
            ack: false,
            r#type: "*".to_string(),
            state: "*".to_string(),
            uses: "*".to_string(),
            options: Vars::new(),
        }
    }
}

impl ChannelOptions {
    pub fn pattern(&self) -> String {
        let mut options = Vars::new()
            .with("ack", self.ack)
            .with("type", self.r#type.clone())
            .with("state", self.state.clone())
            .with("uses", self.uses.clone());

        for (key, value) in self.options.iter() {
            options.set(key, value);
        }
        options.to_string()
    }
}

/// Just a export struct for the event::Emitter
///
pub struct Channel {
    runtime: Arc<Runtime>,
    ack: bool,
    chan_id: String,
    pattern: String,
    glob: (
        globset::GlobMatcher,
        globset::GlobMatcher,
        globset::GlobMatcher,
        Vec<(String, globset::GlobMatcher)>,
    ),
}

impl Channel {
    pub fn new(rt: &Arc<Runtime>) -> Self {
        Self::channel(rt, &ChannelOptions::default())
    }

    /// create a emit channel to receive message
    /// if the message is not received by client, the engine will re-send at the next time interval
    #[allow(clippy::self_named_constructors)]
    pub fn channel(rt: &Arc<Runtime>, options: &ChannelOptions) -> Self {
        debug!("channel: {options:?}");
        let pat_type = globset::Glob::new(&options.r#type)
            .unwrap()
            .compile_matcher();
        let pat_state = globset::Glob::new(&options.state)
            .unwrap()
            .compile_matcher();
        let pat_uses = globset::Glob::new(&options.uses).unwrap().compile_matcher();
        let opt_globs: Vec<(String, globset::GlobMatcher)> = options
            .options
            .iter()
            .filter_map(|(k, v)| {
                v.as_str()
                    .and_then(|pattern| globset::Glob::new(pattern).ok())
                    .map(|g| (k.clone(), g.compile_matcher()))
            })
            .collect();
        Self {
            runtime: rt.clone(),
            ack: options.ack,
            chan_id: options.id.clone(),
            pattern: options.pattern(),
            glob: (pat_type, pat_state, pat_uses, opt_globs),
        }
    }

    ///  Receive act message
    ///
    /// Example
    /// ```rust,no_run
    /// use acts::{Engine, Act, Workflow, Vars, Message};
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let engine = Engine::new().start().unwrap();
    ///     let workflow = Workflow::new().with_id("m1").with_step(|step| {
    ///             step.with_id("step1").with_uses("acts.core.irq", Vars::new().with("var1", 10))
    ///     });
    ///
    ///     engine.channel().on_message(move |e| {
    ///         if let Some(uses) = &e.uses && e.r#type == "act" && uses == "acts.core.irq" {
    ///             println!("act message: state={} inputs={:?} outputs={:?}", e.state, e.inputs, e.outputs);
    ///         }
    ///     });
    ///     let exec = engine.executor();
    ///     exec.model().deploy(&workflow, None).expect("fail to deploy workflow");
    ///     let mut vars = Vars::new();
    ///     vars.set("pid", "w1");
    ///     exec.proc().start(
    ///        &workflow.id,
    ///        vars,
    ///    );
    /// }
    /// ```
    pub fn on_message(self: &Arc<Self>, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let glob = self.glob.clone();
        let runtime = self.runtime.clone();
        let ack = self.ack;
        let chan_id = self.chan_id.clone();
        let pattern = self.pattern.clone();
        self.runtime.emitter().on_message(&self.chan_id, move |e| {
            debug!("on_message: chan={} {e:?}", chan_id);
            if is_match(&glob, e) {
                store_if(&runtime, ack, &chan_id, &pattern, e);
                f(e);
            }
        });
    }

    pub fn on_start(self: &Arc<Self>, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let glob = self.glob.clone();
        let runtime = self.runtime.clone();
        let ack = self.ack;
        let chan_id = self.chan_id.clone();
        let pattern = self.pattern.clone();
        self.runtime.emitter().on_start(&self.chan_id, move |e| {
            if is_match(&glob, e) {
                store_if(&runtime, ack, &chan_id, &pattern, e);
                f(e);
            }
        });
    }

    pub fn on_complete(self: &Arc<Self>, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let glob = self.glob.clone();
        let runtime = self.runtime.clone();
        let ack = self.ack;
        let chan_id = self.chan_id.clone();
        let pattern = self.pattern.clone();
        self.runtime.emitter().on_complete(&self.chan_id, move |e| {
            debug!("on_complete: chan={} {e:?}", chan_id);
            if is_match(&glob, e) {
                store_if(&runtime, ack, &chan_id, &pattern, e);
                f(e);
            }
        });
    }

    pub fn on_error(self: &Arc<Self>, f: impl Fn(&Event<Message>) + Send + Sync + 'static) {
        let glob = self.glob.clone();
        let runtime = self.runtime.clone();
        let ack = self.ack;
        let chan_id = self.chan_id.clone();
        let pattern = self.pattern.clone();
        self.runtime.emitter().on_error(&self.chan_id, move |e| {
            if is_match(&glob, e) {
                store_if(&runtime, ack, &chan_id, &pattern, e);
                f(e);
            }
        });
    }

    pub fn close(&self) {
        self.runtime.emitter().remove(&self.chan_id);
    }
}

fn store_if(runtime: &Arc<Runtime>, ack: bool, chan_id: &str, pattern: &str, message: &Message) {
    if ack && !chan_id.is_empty() && message.retry_times == 0 {
        info!("store: {message:?}");
        let msg = message.into(chan_id, pattern);
        let store = runtime.cache().store();
        store.messages().create(&msg).unwrap_or_else(|err| {
            error!("channel.store_if_emit_id: {err}");
            eprintln!("channel.store_if_emit_id: {err}");
            false
        });
    }
}

fn is_match(
    glob: &(
        globset::GlobMatcher,
        globset::GlobMatcher,
        globset::GlobMatcher,
        Vec<(String, globset::GlobMatcher)>,
    ),
    e: &Event<Message>,
) -> bool {
    let (pat_type, pat_state, pat_uses, pat_options) = glob;
    if !pat_type.is_match(&e.r#type)
        || !pat_state.is_match(e.state.as_ref())
        || !pat_uses.is_match(e.uses.as_deref().unwrap_or_default())
    {
        return false;
    }

    let msg_options = e.options();
    for (key, pat) in pat_options {
        let value = msg_options
            .as_ref()
            .and_then(|o| o.get::<String>(key))
            .unwrap_or_default();
        if !pat.is_match(&value) {
            return false;
        }
    }
    true
}
