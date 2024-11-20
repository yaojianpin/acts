use crate::{sch::Runtime, utils, Event, Message};
use std::sync::Arc;
use tracing::{debug, error, info};

fn store_if(runtime: &Arc<Runtime>, ack: bool, chan_id: &str, pattern: &str, message: &Message) {
    if ack && !chan_id.is_empty() && message.retry_times == 0 {
        println!("store: {message:?}");
        let msg = message.into(chan_id, pattern);
        runtime
            .cache()
            .store()
            .base()
            .messages()
            .create(&msg)
            .unwrap_or_else(|err| {
                error!("channel.store_if_emit_id: {}", err.to_string());
                eprintln!("channel.store_if_emit_id: {}", err);
                false
            });
    }
}

fn is_match(
    glob: &(
        globset::GlobMatcher,
        globset::GlobMatcher,
        globset::GlobMatcher,
        globset::GlobMatcher,
    ),
    e: &Event<Message>,
) -> bool {
    let (pat_type, pat_state, pat_tag, pat_key) = glob;
    pat_type.is_match(&e.r#type)
        && pat_state.is_match(e.state.to_string())
        && (pat_tag.is_match(&e.tag) || pat_tag.is_match(&e.model.tag))
        && pat_key.is_match(&e.key)
}

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
    /// use the glob pattern to match the message tag or model tag
    /// eg. *tag1*
    pub tag: String,
    /// use the blob pattern to match the message key
    /// eg. key1*
    pub key: String,
}

impl Default for ChannelOptions {
    fn default() -> Self {
        Self {
            id: utils::shortid(),
            ack: false,
            r#type: "*".to_string(),
            state: "*".to_string(),
            tag: "*".to_string(),
            key: "*".to_string(),
        }
    }
}

impl ChannelOptions {
    pub fn pattern(&self) -> String {
        format!("{}:{}:{}:{}", self.r#type, self.state, self.tag, self.key)
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
        globset::GlobMatcher,
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
        let pat_tag = globset::Glob::new(&options.tag).unwrap().compile_matcher();
        let pat_key = globset::Glob::new(&options.key).unwrap().compile_matcher();

        Self {
            runtime: rt.clone(),
            ack: options.ack,
            chan_id: options.id.clone(),
            pattern: options.pattern(),
            glob: (pat_type, pat_state, pat_tag, pat_key),
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
    ///     let engine = Engine::new();
    ///     let workflow = Workflow::new().with_id("m1").with_step(|step| {
    ///             step.with_id("step1").with_act(Act::new().with_act("irq").with_key("act1"))
    ///     });
    ///
    ///     engine.channel().on_message(move |e| {
    ///         if e.r#type == "irq" {
    ///             println!("act message: state={} inputs={:?} outputs={:?}", e.state, e.inputs, e.outputs);
    ///         }
    ///     });
    ///     let exec = engine.executor();
    ///     exec.model().deploy(&workflow).expect("fail to deploy workflow");
    ///     let mut vars = Vars::new();
    ///     vars.insert("pid".into(), "w1".into());
    ///     exec.proc().start(
    ///        &workflow.id,
    ///        &vars,
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
            info!("on_message: chan={} {e:?}", chan_id);
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
