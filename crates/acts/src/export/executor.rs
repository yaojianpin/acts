pub(crate) mod act_executor;
pub(crate) mod event_executor;
pub(crate) mod ext_executor;
pub(crate) mod message_executor;
pub(crate) mod model_executor;
pub(crate) mod package_executor;
pub(crate) mod process_executor;
pub(crate) mod task_executor;

use crate::Principal;
use crate::scheduler::Runtime;
use std::sync::Arc;

/// The engine's operations, bound to one caller.
///
/// Every operation runs [`Principal::check`] for its action name before it
/// touches the engine — `model().deploy()` included — so an embedder holding
/// an `Executor` answers to exactly the policy a transport caller does. There
/// is no path to the runtime that a caller's grants do not stand behind: the
/// action names are the same ones the dispatch table matches, defined once per
/// operation below and used on both sides.
///
/// The principal also decides what an operation *seals*: `proc().start()` and
/// `evt().start()` put its [`crate::ScopePolicy`] (snapshot scopes, workdir
/// root) into the run they start, so a caller cannot widen a run's reading by
/// passing an authority as an input.
///
/// The executor authenticates nothing itself — the caller says who it is.
/// [`Engine::executor`](crate::Engine::executor) takes the principal, and
/// [`Engine::anonymous`](crate::Engine::anonymous) is the identity of a
/// request that carries no token.
#[derive(Clone)]
pub struct Executor {
    principal: Arc<Principal>,
    msg: message_executor::MessageExecutor,
    act: act_executor::ActExecutor,
    model: model_executor::ModelExecutor,
    proc: process_executor::ProcessExecutor,
    task: task_executor::TaskExecutor,
    pack: package_executor::PackageExecutor,
    evt: event_executor::EventExecutor,
    ext: ext_executor::ExtExecutor,
}

impl Executor {
    pub(crate) fn new(rt: &Arc<Runtime>, principal: &Principal) -> Self {
        // One copy of the policy, shared by every sub-executor: the caller is
        // one identity for the whole operation.
        let principal = Arc::new(principal.clone());
        Self {
            msg: message_executor::MessageExecutor::new(rt, &principal),
            act: act_executor::ActExecutor::new(rt, &principal),
            model: model_executor::ModelExecutor::new(rt, &principal),
            proc: process_executor::ProcessExecutor::new(rt, &principal),
            task: task_executor::TaskExecutor::new(rt, &principal),
            pack: package_executor::PackageExecutor::new(rt, &principal),
            evt: event_executor::EventExecutor::new(rt, &principal),
            ext: ext_executor::ExtExecutor::new(rt, &principal),
            principal,
        }
    }

    /// The executor of the engine itself: for the operations the engine
    /// performs on its own behalf rather than for a caller — publishing its
    /// built-in packages while starting up, spawning the subflow of a live
    /// run.
    ///
    /// It is [`Principal::unrestricted`], so it must never stand in for a
    /// request: a request has a token, and
    /// [`Engine::executor`](crate::Engine::executor) is where one becomes a
    /// principal. Being an `Executor` like any other means the engine's own
    /// work goes through the same code path, and only the identity differs.
    pub(crate) fn engine(rt: &Arc<Runtime>) -> Self {
        Self::new(rt, &Principal::unrestricted())
    }

    /// The caller every operation of this executor is checked against.
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    /// executor for related message functions
    pub fn msg(&self) -> &message_executor::MessageExecutor {
        &self.msg
    }

    /// executor for related act operations
    /// such as 'complete', 'back', 'cancel' ..
    pub fn act(&self) -> &act_executor::ActExecutor {
        &self.act
    }

    /// executor for related model functions
    pub fn model(&self) -> &model_executor::ModelExecutor {
        &self.model
    }

    /// executor for related process functions
    pub fn proc(&self) -> &process_executor::ProcessExecutor {
        &self.proc
    }

    /// executor for related task functions
    pub fn task(&self) -> &task_executor::TaskExecutor {
        &self.task
    }

    /// executor for related package functions
    pub fn pack(&self) -> &package_executor::PackageExecutor {
        &self.pack
    }

    /// executor for related event functions
    pub fn evt(&self) -> &event_executor::EventExecutor {
        &self.evt
    }

    /// executor for extending the engine itself: registering a user var
    /// module or publishing a package definition.
    ///
    /// These are the embedder's operations — a deployment that only drives
    /// the engine (a transport, the CLI) never needs them, and they belong to
    /// the caller who owns the process the engine runs in.
    pub fn ext(&self) -> &ext_executor::ExtExecutor {
        &self.ext
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ActError, ActPackage, Config, Engine, Result, Vars, Workflow, data::Package, query::Query,
        utils::test::USES_SET,
    };
    use serde_json::json;

    /// A role that may read the model catalogue and nothing else.
    fn reader_config() -> Config {
        Config {
            data: Default::default(),
            table: toml::from_str(
                r#"
                [acl]
                [[acl.role]]
                name = "reader"
                tokens = ["reader-token"]
                allow = ["model:ls", "model:get"]
                "#,
            )
            .unwrap(),
        }
    }

    async fn engine() -> Engine {
        Engine::builder()
            .set_config(&reader_config())
            .start()
            .await
            .unwrap()
    }

    fn package() -> Package {
        Package {
            id: "test_pack".to_string(),
            version: "0.1.0".to_string(),
            schema: "{}".to_string(),
            resources: "[]".to_string(),
            run_as: crate::ActRunAs::Func,
            catalog: crate::ActPackageCatalog::Core,
            ..Default::default()
        }
    }

    /// Every operation the reader is not granted is refused — including
    /// `model:deploy` and `proc:start`, the two an embedder used to reach
    /// without any check at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_limited_principal_is_refused_every_ungranted_operation() {
        let engine = engine().await;
        let reader = engine.acl().authenticate(Some("reader-token")).unwrap();
        let executor = engine.executor(&reader);

        let model = Workflow::new()
            .with_id("m1")
            .with_step(|step| step.with_id("step1").with_uses(USES_SET, Vars::new()));
        let results: Vec<Result<()>> = vec![
            executor.model().deploy(&model, None).await.map(|_| ()),
            executor.model().rm("m1").await.map(|_| ()),
            executor.pack().publish(&package()).await.map(|_| ()),
            executor.pack().get("test_pack").await.map(|_| ()),
            executor.pack().rm("test_pack").await.map(|_| ()),
            executor.proc().start("m1", Vars::new()).await.map(|_| ()),
            executor.proc().start_from_model("", "yml", Vars::new()).await.map(|_| ()),
            executor.proc().list(&Query::new()).await.map(|_| ()),
            executor.proc().get("p1").await.map(|_| ()),
            executor.proc().get_process("p1").await.map(|_| ()),
            executor.task().list(&Query::new()).await.map(|_| ()),
            executor.task().get("p1", "t1").await.map(|_| ()),
            executor.msg().list(&Query::new()).await.map(|_| ()),
            executor.msg().get("d1").await.map(|_| ()),
            executor.msg().ack("d1").await,
            executor.msg().rm("d1").await.map(|_| ()),
            executor.msg().clear(None).await,
            executor.msg().redo().await,
            executor.msg().clear_delivery("d1").await,
            executor.msg().redeliver("d1").await,
            executor.msg().unsub("c1").await,
            executor.evt().list(&Query::new()).await.map(|_| ()),
            executor.evt().get("e1").await.map(|_| ()),
            executor.evt().start("e1", &json!(null)).await.map(|_| ()),
            executor.act().complete("p1", "t1", Vars::new()).await,
            executor.act().do_action("p1", "t1", crate::event::EventAction::Push, Vars::new()).await,
            executor.ext().register_var(&TestVar).map(|_| ()),
            executor.ext().register_package(&TestVar::definition()).await,
        ];
        for result in results {
            let err = result.expect_err("an ungranted operation must be refused");
            assert!(
                matches!(err, ActError::Denied(_)),
                "expected a denial, got {err:?}"
            );
        }
    }

    /// ...while what it *is* granted still works, so the refusals above are
    /// the policy and not a blanket failure.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_granted_operation_still_runs() {
        let engine = engine().await;
        let reader = engine.acl().authenticate(Some("reader-token")).unwrap();
        let executor = engine.executor(&reader);

        assert!(executor.model().list(&Query::new()).await.is_ok());
    }

    /// An engine with no `[acl]` section: the tokenless caller reads the
    /// catalogue and nothing else, through the same executor.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_anonymous_callers_executor_reads_only_the_catalogue() {
        let engine = Engine::builder().start().await.unwrap();
        let executor = engine.executor(&engine.anonymous());

        assert!(executor.model().list(&Query::new()).await.is_ok());
        assert!(executor.pack().list(&Query::new()).await.is_ok());
        assert!(matches!(
            executor.model().deploy(&Workflow::new().with_id("m1"), None).await,
            Err(ActError::Denied(_))
        ));
        assert!(matches!(
            executor.proc().list(&Query::new()).await,
            Err(ActError::Denied(_))
        ));
    }

    /// The executor seals the caller's authority into the run it starts: the
    /// run's snapshot scopes are the caller's, and a start option naming a
    /// different authority cannot override them.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_started_run_carries_the_callers_authority() {
        let config = Config {
            data: Default::default(),
            table: toml::from_str(
                r#"
                [acl]
                [[acl.role]]
                name = "u1"
                tokens = ["t1"]
                allow = ["proc:start", "proc:get", "model:deploy"]
                snapshot = { secrets = ["$subject"] }
                "#,
            )
            .unwrap(),
        };
        let engine = Engine::builder().set_config(&config).start().await.unwrap();
        let u1 = engine.acl().authenticate(Some("t1")).unwrap();
        let executor = engine.executor(&u1);
        let model = Workflow::new().with_id("m1").with_step(|step| {
            step.with_id("step1")
                .with_uses(crate::utils::test::USES_SET, Vars::new().with("k", 1))
        });
        executor.model().deploy(&model, None).await.unwrap();
        let (send, done) = engine.signal::<bool>(false).double();
        engine.channel().on_complete(move |_| {
            let send = send.clone();
            async move {
                send.send(true);
            }
        });
        // A caller asking for someone else's authority in the options gets its
        // own: the executor sealed its principal's, and `Runtime::start` pops
        // that rather than reading one out of the request.
        let pid = executor
            .proc()
            .start(
                "m1",
                Vars::new().with(
                    crate::utils::consts::PROC_OWNER,
                    crate::ScopePolicy {
                        subject: "u2".to_string(),
                        ..crate::ScopePolicy::unrestricted()
                    },
                ),
            )
            .await
            .unwrap();

        let proc = executor
            .proc()
            .get_process(&pid)
            .await
            .unwrap()
            .expect("the run is live");
        assert_eq!(proc.owner_scope().subject, "u1");
        assert!(proc.owner_scope().allows("secrets", "u1"));
        assert!(!proc.owner_scope().allows("secrets", "u2"));
        assert!(done.recv().await);
    }

    #[derive(Clone)]
    struct TestVar;

    impl crate::ActUserVar for TestVar {
        fn name(&self) -> String {
            "executor_test_var".to_string()
        }
    }

    impl crate::ActPackage for TestVar {
        fn new(_: &Config) -> crate::Result<Self> {
            Ok(Self)
        }

        fn definition() -> crate::ActPackageDefinition {
            crate::ActPackageDefinition {
                id: "executor_test_pack",
                name: "executor test pack",
                desc: "",
                icon: "",
                doc: "",
                version: "0.1.0",
                schema: json!({}),
                options: None,
                run_as: crate::ActRunAs::Func,
                resources: vec![],
                catalog: crate::ActPackageCatalog::Core,
            }
        }
    }
}
