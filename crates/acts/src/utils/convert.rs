use crate::{Context, Vars};
use regex::Regex;
use serde_json::Value as JsonValue;
use std::sync::LazyLock;

pub fn fill_params(params: &JsonValue, ctx: &Context) -> JsonValue {
    match params {
        JsonValue::String(value) => {
            let exprs = get_exprs(value);
            if !exprs.is_empty() {
                let mut value = value.clone();
                for (range, expr, content) in &exprs {
                    let result =
                        Context::scope(ctx, move || ctx.runtime.env().eval::<JsonValue>(content))
                            .unwrap_or_else(|err| {
                                eprintln!("fill_params: expr:{value}, err={err}");
                                JsonValue::Null
                            });
                    // just return json for only one express
                    if range.start == 0 && range.end == value.len() {
                        return result;
                    }

                    match result {
                        JsonValue::Bool(v) => {
                            value = value.replace(expr, &v.to_string());
                        }
                        JsonValue::Number(v) => {
                            value = value.replace(expr, &v.to_string());
                        }
                        JsonValue::String(v) => {
                            value = value.replace(expr, &v);
                        }
                        v => {
                            value = value.replace(expr, &v.to_string());
                        }
                    }
                }
                // return string json for multiple expressions
                return JsonValue::String(value);
            }

            // return params itself for no expression
            params.clone()
        }
        JsonValue::Array(values) => {
            let mut arr = Vec::new();
            for value in values {
                arr.push(fill_params(value, ctx));
            }
            JsonValue::Array(arr)
        }
        JsonValue::Object(map) => {
            let mut obj = serde_json::Map::new();
            for (k, value) in map {
                obj.insert(k.clone(), fill_params(value, ctx));
            }
            JsonValue::Object(obj)
        }
        v => v.clone(),
    }
}

/// fill the vars
/// 1. if the inputs is an expression, just calculate it
///    or insert the input itself
pub fn fill_inputs(inputs: &Vars, ctx: &Context) -> Vars {
    let mut ret = Vars::new();
    for (k, v) in inputs.iter() {
        if let JsonValue::String(value) = v {
            if let Some(expr) = get_expr(value) {
                let result =
                    Context::scope(ctx, move || ctx.runtime.env().eval::<JsonValue>(&expr));

                let new_value = result.unwrap_or_else(|err| {
                    eprintln!("fill_inputs: expr:{value}, err={err}");
                    JsonValue::Null
                });

                // satisfies the rule 1
                ret.insert(k.clone(), new_value);
                continue;
            }
        } else if let JsonValue::Object(obj) = v {
            ret.insert(k.clone(), fill_inputs(&Vars::from(obj.clone()), ctx).into());
            continue;
        }
        ret.insert(k.clone(), v.clone());
    }

    ret
}

/// fill the outputs
/// 1. if the outputs is an expression, just calculate it
/// 2. if the env and the outputs both has the same key, using the local outputs
pub fn fill_outputs(outputs: &Vars, ctx: &Context) -> Vars {
    // println!("fill_outputs: outputs={outputs}");
    let mut ret = Vars::new();
    for (k, v) in outputs.iter() {
        if let JsonValue::String(string) = v
            && let Some(expr) = get_expr(string)
        {
            let result = Context::scope(ctx, move || ctx.runtime.env().eval::<JsonValue>(&expr));
            let new_value = result.unwrap_or_else(|err| {
                eprintln!("fill_outputs: expr:{string}, err={err}");
                JsonValue::Null
            });

            // satisfies the rule 1
            ret.insert(k.clone(), new_value);
            continue;
        }

        // rule 2
        if v.is_null() {
            // the env value
            match ctx.task().find(&ctx.proc, k) {
                Some(v) => ret.insert(k.clone(), v),
                None => ret.insert(k.clone(), v.clone()),
            };
        } else {
            // insert the orign value
            ret.insert(k.clone(), v.clone());
        }
    }

    ret
}

pub fn get_expr(text: &str) -> Option<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\$\{\{(.+)\}\}$").unwrap());
    let caps = RE.captures(text);

    if let Some(caps) = caps {
        let value = caps.get(1).map_or("", |m| m.as_str());
        return Some(value.trim().to_string());
    }

    None
}

pub fn get_exprs(text: &str) -> Vec<(core::ops::Range<usize>, String, String)> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{\{(.*)\}\}").unwrap());
    RE.captures_iter(text)
        .map(|caps| {
            let input = caps.get(0).unwrap();
            let content = caps.get(1).unwrap();
            (
                input.range(),
                input.as_str().to_string(),
                content.as_str().to_string(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Workflow};
    use serde_json::json;

    /// Run a trivial workflow and hand back the finished root context. The
    /// workflow vars are the task data expressions resolve against.
    async fn completed_context() -> crate::Context {
        let engine = Engine::builder().start().await.unwrap();
        let sig = engine.signal(());
        let done = sig.clone();
        engine.channel().on_complete(move |_| {
            let done = done.clone();
            async move { done.close() }
        });
        let workflow = Workflow::new()
            .with_id("convert-model")
            .with_var("a", 5)
            .with_var("arr", json!([1, 2]))
            .with_var("obj", json!({ "k": 1 }))
            .with_step(|step| {
                step.with_id("s1")
                    .with_uses(crate::utils::test::USES_SET, Vars::new())
            });
        let pid = engine
            .runtime()
            .start(&workflow, Vars::new())
            .await
            .unwrap();
        sig.recv().await;

        let runtime = engine.runtime();
        let read = runtime.read_proc(&pid).await.unwrap().unwrap();
        crate::scheduler::read_context(&runtime, &read).unwrap()
    }

    #[test]
    fn get_expr_and_exprs() {
        assert_eq!(get_expr("${{  a + 1  }}").as_deref(), Some("a + 1"));
        assert!(get_expr("${{ a }} trailing").is_none());
        assert!(get_expr("plain").is_none());

        let exprs = get_exprs("a${{ 1 }}\nb${{ 2 }}");
        assert_eq!(exprs.len(), 2);
        assert_eq!(exprs[0].1, "${{ 1 }}");
        assert_eq!(exprs[0].2, " 1 ");
        assert_eq!(exprs[0].0, 1..9);
        assert_eq!(exprs[1].2, " 2 ");

        assert!(get_exprs("no expression").is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fill_params_evaluates_and_passes_through_values() {
        let ctx = completed_context().await;

        // a single expression returns the typed json value itself
        assert_eq!(fill_params(&json!("${{ a }}"), &ctx), json!(5));
        assert_eq!(fill_params(&json!("${{ a > 1 }}"), &ctx), json!(true));

        // an embedded expression is substituted as text
        assert_eq!(
            fill_params(&json!("value=${{ a }}"), &ctx),
            json!("value=5")
        );
        assert_eq!(
            fill_params(&json!("ok=${{ a > 1 }}"), &ctx),
            json!("ok=true")
        );
        assert_eq!(
            fill_params(&json!("s=${{ 'text' }}"), &ctx),
            json!("s=text")
        );
        // non-scalar results are stringified through their json text
        assert_eq!(
            fill_params(&json!("arr=${{ arr }}"), &ctx),
            json!("arr=[1,2]")
        );
        assert_eq!(
            fill_params(&json!("obj=${{ obj }}"), &ctx),
            json!("obj={\"k\":1}")
        );

        // several expressions are each substituted
        assert_eq!(
            fill_params(&json!("${{ a }}\n${{ a > 1 }}"), &ctx),
            json!("5\ntrue")
        );

        // an evaluation error substitutes null instead of failing
        assert_eq!(fill_params(&json!("x=${{ 1 + }}"), &ctx), json!("x=null"));

        // no expression: the value is returned untouched
        assert_eq!(fill_params(&json!("plain"), &ctx), json!("plain"));

        // arrays and objects are filled recursively, scalars cloned
        let params = json!({
            "a": "${{ a }}",
            "list": ["${{ a }}", 2, true, null],
            "nested": { "b": "${{ a > 1 }}" },
        });
        assert_eq!(
            fill_params(&params, &ctx),
            json!({
                "a": 5,
                "list": [5, 2, true, null],
                "nested": { "b": true },
            })
        );
        assert_eq!(fill_params(&json!(3), &ctx), json!(3));
        assert_eq!(fill_params(&json!(null), &ctx), json!(null));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fill_inputs_evaluates_expressions_and_nested_objects() {
        let ctx = completed_context().await;
        let inputs = Vars::new()
            .with("expr", "${{ a + 1 }}")
            .with("bad", "${{ 1 + }}")
            .with("plain", "text")
            .with("nested", json!({ "k": "${{ a }}", "v": 2 }))
            .with("num", 3);

        let out = fill_inputs(&inputs, &ctx);
        assert_eq!(out.get::<i32>("expr").unwrap(), 6);
        assert_eq!(out.get_value("bad").unwrap(), &json!(null));
        assert_eq!(out.get::<String>("plain").unwrap(), "text");
        assert_eq!(out.get_value("nested").unwrap(), &json!({ "k": 5, "v": 2 }));
        assert_eq!(out.get::<i32>("num").unwrap(), 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fill_outputs_evaluates_and_falls_back_to_task_data() {
        let ctx = completed_context().await;
        ctx.task().set_data_with(|data| data.set("only_in_task", 7));

        let outputs = Vars::new()
            .with("expr", "${{ a + 2 }}")
            .with("bad", "${{ 1 + }}")
            .with("only_in_task", json!(null))
            .with("missing", json!(null))
            .with("value", 9);

        let out = fill_outputs(&outputs, &ctx);
        assert_eq!(out.get::<i32>("expr").unwrap(), 7);
        assert_eq!(out.get_value("bad").unwrap(), &json!(null));
        assert_eq!(out.get::<i32>("only_in_task").unwrap(), 7);
        assert_eq!(out.get_value("missing").unwrap(), &json!(null));
        assert_eq!(out.get::<i32>("value").unwrap(), 9);
    }
}
