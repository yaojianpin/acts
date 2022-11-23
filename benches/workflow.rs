use act::{Engine, Workflow};
use criterion::*;
use tokio::runtime::Runtime;

fn simple_workflow(c: &mut Criterion) {
    let text = r#"
  name: test1
  ver: 1.0
  env: 
    a: 100
  jobs:
    - id: job1
      steps:
        - name: step 1
          run: |
            print("step 1")
        - name: step 2
          run: |
            print("step 2");
            let v = 50;
            console::log(`v=${v}`);
            console::dbg(`v=${v}`);
            console::info(`v=${v}`);
            console::wran(`v=${v}`);
            console::error(`v=${v}`);
        - name: step 3
          env:
            e: abc
          branches:
            - name: branch 1
              if: env.get("a") >= 100
              steps:
                - name: branch 1.1
                  run: |
                    print("branch 1.1");
                - name: branch 1.2
                  run: print("branch 1.2")
            - name: branch 2
              if: env.get("a") < 100
              steps:
                - name:  branch 2.1
                  run: print("branch 2.1")
          run: |
            print("step 3");

        - name: step 4
          run: 
            print(`step 4`);
  
  "#;
    c.bench_function("simple_workflow", |b| {
        let rt = Runtime::new().unwrap();
        b.iter(move || {
            let engine = Engine::new();
            let workflow = Workflow::from_str(text).unwrap();
            rt.block_on(async move {
                engine.push(&workflow);
                let e = engine.clone();
                engine.on_workflow_complete(move |_w: &Workflow| {
                    e.close();
                });
                engine.start().await;
            });
        })
    });
}

criterion_group!(benches, simple_workflow);
criterion_main!(benches);
