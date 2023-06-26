use acts::{Engine, Vars, Workflow};
use criterion::*;
use tokio::runtime::Runtime;

fn start_workflow(c: &mut Criterion) {
    let text = r#"
  id: test1
  env: 
    a: 100
  jobs:
    - id: job1
      steps:
        - name: step 1
        - name: step 2
          run: |
            let v = 50;
        - name: step 3
          env:
            e: abc
          branches:
            - name: branch 1
              if: env.get("a") >= 100
              steps:
                - name: branch 1.1
                - name: branch 1.2
            - name: branch 2
              if: env.get("a") < 100
              steps:
                - name:  branch 2.1
        - name: step 4
  
  "#;
    c.bench_function("start_workflow", |b| {
        let rt = Runtime::new().unwrap();
        let engine = Engine::new();
        let e = engine.clone();
        rt.block_on(async move {
            engine.start();
            let workflow = Workflow::from_str(text).unwrap();
            engine.manager().deploy(&workflow).unwrap();
        });

        b.iter(move || {
            let workflow = Workflow::from_str(text).unwrap();
            let exec = e.executor();
            rt.block_on(async move {
                exec.start(&workflow.id, &Vars::new()).unwrap();
            });
        })
    });
}

criterion_group!(benches, start_workflow);
criterion_main!(benches);
