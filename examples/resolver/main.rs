use acts::{ConfigResolver, Engine, Result, Vars, Workflow};
use std::sync::Arc;

/// A resolver that injects tenant-scoped configuration (e.g. secrets, feature flags)
/// into each task's sealed data, accessible via `$profile` in JS expressions.
struct ProfileResolver {
    data: Vars,
}

impl ProfileResolver {
    fn new(tenant: &str) -> Self {
        Self {
            data: Vars::new()
                .with("tenant", tenant)
                .with(
                    "secrets",
                    Vars::new()
                        .with("API_KEY", "sk-abc123")
                        .with("DB_PASS", "s3cr3t"),
                )
                .with(
                    "features",
                    Vars::new().with("beta", true).with("rate_limit", 100),
                ),
        }
    }
}

#[async_trait::async_trait]
impl ConfigResolver for ProfileResolver {
    async fn resolve(&self, _ctx: &Vars) -> Result<Vars> {
        Ok(self.data.clone())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let resolver = Arc::new(ProfileResolver::new("acme-corp"));

    // Register resolver via EngineBuilder before starting the engine
    let engine = Engine::builder()
        .add_resolver("profile", resolver)
        .build()
        .start()
        .await?;

    let (s, sig) = engine.signal(()).double();

    let workflow = Workflow::new()
        .with_id("resolver_demo")
        .with_ver("0.1.0")
        .with_step(|step| {
            step.with_id("step1")
                .with_name("access sealed config")
                .with_uses_code(
                    "acts.transform.code",
                    r#"
                // Access sealed data injected by the resolver
                let tenant = $profile.tenant;
                let apiKey = $profile.secrets.API_KEY;
                let beta = $profile.features.beta;

                console.log("tenant:", tenant);
                console.log("apiKey:", apiKey);
                console.log("beta:", beta);

                $set("output", "resolved: " + tenant + ", beta=" + beta);
                "#,
                )
        });

    workflow.print();

    let executor = engine.executor();
    executor.model().deploy(&workflow, None).await?;

    let vars = Vars::new().with("pid", "r1");
    executor.proc().start(&workflow.id, vars).await?;

    engine.channel().on_complete(move |e| {
        let s = s.clone();
        async move {
            println!("on_complete: {:?}, cost={}ms", e.outputs, e.cost());
            s.close();
        }
    });

    engine.channel().on_error(move |e| async move {
        println!("on_error: {:?}", e.state);
    });

    sig.recv().await;

    Ok(())
}
