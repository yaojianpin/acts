//! The evaluator's contract: what parses, what evaluates to what, and what is
//! rejected.

use crate::{Context, Error, Expr, Result, Value, eval};

/// Evaluate `source` against an empty context.
fn value(source: &str) -> Result<Value> {
    eval(source, &Context::new())
}

fn int(source: &str) -> i64 {
    match value(source) {
        Ok(Value::Int(i)) => i,
        other => panic!("{source} = {other:?}"),
    }
}

fn float(source: &str) -> f64 {
    match value(source) {
        Ok(Value::Float(f)) => f,
        other => panic!("{source} = {other:?}"),
    }
}

fn boolean(source: &str) -> bool {
    match value(source) {
        Ok(Value::Bool(b)) => b,
        other => panic!("{source} = {other:?}"),
    }
}

fn text(source: &str) -> String {
    match value(source) {
        Ok(Value::Str(s)) => s.to_string(),
        other => panic!("{source} = {other:?}"),
    }
}

fn error(source: &str) -> Error {
    value(source).expect_err(source)
}

// ---- literals and precedence ----

#[test]
fn literals() {
    assert_eq!(int("12"), 12);
    assert_eq!(int("0"), 0);
    assert_eq!(float("1.5"), 1.5);
    assert_eq!(float("1e3"), 1000.0);
    assert_eq!(float("1.5e-1"), 0.15);
    assert_eq!(text("'text'"), "text");
    assert_eq!(text("\"text\""), "text");
    assert_eq!(text("'it\\'s'"), "it's");
    assert_eq!(text("'a\\nb'"), "a\nb");
    assert_eq!(text("'\\u0041'"), "A");
    assert_eq!(value("true").unwrap(), Value::Bool(true));
    assert_eq!(value("false").unwrap(), Value::Bool(false));
    assert!(value("null").unwrap().is_null());
}

#[test]
fn precedence_and_grouping() {
    assert_eq!(int("1 + 2 * 3"), 7);
    assert_eq!(int("(1 + 2) * 3"), 9);
    assert_eq!(int("2 * 3 % 4"), 2);
    assert_eq!(int("10 - 3 - 2"), 5);
    assert_eq!(int("-2 * 3"), -6);
    assert_eq!(int("-(2 + 3)"), -5);
    assert!(boolean("!true == false"), "`!` binds tighter than `==`");
    assert!(boolean("1 + 1 == 2 && 3 > 2"));
    assert!(
        boolean("1 == 1 || 1 / 0 == 0"),
        "`||` wins before the divide"
    );

    // Unary binds tighter than every binary operator, so the operand is the
    // next operand and not the rest of the expression.
    assert_eq!(int("-1 - 2"), -3);
    assert_eq!(int("-2 + 3"), 1);
    assert!(
        !boolean("!false && false"),
        "`(!false) && false`, not `!(...)`"
    );
    assert_eq!(int("- -3"), 3);
}

#[test]
fn whitespace_is_insignificant() {
    assert_eq!(int("  1+\t2  "), 3);
    assert_eq!(int("1 +\n2"), 3);
}

// ---- arithmetic ----

#[test]
fn arithmetic_keeps_ints_ints() {
    assert_eq!(int("7 / 2"), 3, "int division truncates");
    assert_eq!(int("-7 / 2"), -3);
    assert_eq!(int("7 % 2"), 1);
    assert_eq!(int("6 / 3"), 2);
    assert_eq!(int("3 * 4"), 12);
}

#[test]
fn arithmetic_with_a_float_is_a_float() {
    assert_eq!(float("7 / 2.0"), 3.5);
    assert_eq!(float("1 + 0.5"), 1.5);
    assert_eq!(float("2.0 * 3"), 6.0);
    assert_eq!(float("7.5 % 2"), 1.5);
}

#[test]
fn arithmetic_overflows_are_errors_not_wrapping() {
    assert_eq!(
        error("9223372036854775807 + 1"),
        Error::Overflow { operation: "+" }
    );
    assert_eq!(
        error("9223372036854775807 * 2"),
        Error::Overflow { operation: "*" }
    );
    assert_eq!(
        error("-9223372036854775807 - 2"),
        Error::Overflow { operation: "-" }
    );
    assert_eq!(
        error("-(-9223372036854775807 - 1)"),
        Error::Overflow { operation: "-" },
        "negating i64::MIN"
    );
}

#[test]
fn division_by_zero_is_an_error() {
    assert_eq!(error("1 / 0"), Error::DivideByZero { operation: "/" });
    assert_eq!(error("1 % 0"), Error::DivideByZero { operation: "%" });
    assert_eq!(error("1.0 / 0.0"), Error::DivideByZero { operation: "/" });
    assert_eq!(error("1.0 % 0.0"), Error::DivideByZero { operation: "%" });
}

#[test]
fn a_uint_above_i64_stays_exact() {
    let mut context = Context::new();
    context
        .set("big", u64::MAX)
        .set("snowflake", 1_234_567_890_123_456_789i64)
        .set("min", i64::MIN);

    // What JSON gives a host for an integer past `i64`, and back out the same.
    assert_eq!(eval("big", &context).unwrap(), Value::UInt(u64::MAX));
    assert_eq!(
        eval("big", &context).unwrap().to_json().unwrap(),
        serde_json::json!(u64::MAX)
    );
    assert_eq!(eval("min", &context).unwrap(), Value::Int(i64::MIN));

    // Arithmetic across the two integer kinds is exact, and only a result past
    // both of them overflows.
    assert_eq!(
        eval("big - 1", &context).unwrap(),
        Value::UInt(u64::MAX - 1)
    );
    assert_eq!(
        eval("snowflake - 1", &context).unwrap(),
        Value::from(1_234_567_890_123_456_788i64)
    );
    assert_eq!(
        eval("big > snowflake", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(eval("big == big", &context).unwrap(), Value::Bool(true));
    assert_eq!(
        eval("big != snowflake", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("big + 1", &context),
        Err(Error::Overflow { operation: "+" })
    );
    assert_eq!(
        eval("big * 2", &context),
        Err(Error::Overflow { operation: "*" })
    );
    assert_eq!(
        eval("-big", &context),
        Err(Error::Type {
            operation: "-",
            got: "uint"
        })
    );
    assert_eq!(eval("big % 2", &context).unwrap(), Value::from(1u64));
}

#[test]
fn plus_concatenates_strings() {
    assert_eq!(text("'a' + 'b' + 'c'"), "abc");
    assert_eq!(
        error("1 + 'a'"),
        Error::Type {
            operation: "+",
            got: "string"
        },
        "the operand that has no overload is the one named"
    );
}

#[test]
fn arithmetic_on_non_numbers_is_a_type_error() {
    assert_eq!(
        error("true - 1"),
        Error::Type {
            operation: "-",
            got: "bool"
        }
    );
    assert_eq!(
        error("null * 2"),
        Error::Type {
            operation: "*",
            got: "null"
        }
    );
}

// ---- comparison and equality ----

#[test]
fn comparison_orders_numbers_and_strings() {
    assert!(boolean("1 < 2"));
    assert!(boolean("2 <= 2"));
    assert!(boolean("3 > 2.5"));
    assert!(boolean("1.5 >= 1.5"));
    assert!(boolean("'a' < 'b'"));
    assert!(boolean("'abc' <= 'abc'"));
}

#[test]
fn equality_compares_across_number_kinds() {
    assert!(boolean("1 == 1.0"));
    assert!(boolean("1 != 2"));
    assert!(boolean("'1' != 1"), "different kinds are simply not equal");
    assert!(boolean("null == null"));
    assert!(boolean("null != 0"));
}

#[test]
fn equality_is_structural_for_injected_values() {
    let mut context = Context::new();
    context
        .set("a", Value::list([1, 2, 3]))
        .set("b", Value::list([1, 2, 3]))
        .set("c", Value::list([1, 2, 4]))
        .set("x", Value::map([("k", 1)]))
        .set("y", Value::map([("k", 1)]));

    assert_eq!(eval("a == b", &context).unwrap(), Value::Bool(true));
    assert_eq!(eval("a != c", &context).unwrap(), Value::Bool(true));
    assert_eq!(eval("x == y", &context).unwrap(), Value::Bool(true));
    assert_eq!(eval("a == x", &context).unwrap(), Value::Bool(false));
}

#[test]
fn comparison_of_unorderable_values_is_a_type_error() {
    assert_eq!(
        error("true < false"),
        Error::Type {
            operation: "<",
            got: "bool"
        }
    );
    assert_eq!(
        error("'a' < 1"),
        Error::Type {
            operation: "<",
            got: "int"
        }
    );
    assert_eq!(
        error("null > null"),
        Error::Type {
            operation: ">",
            got: "null"
        }
    );
}

// ---- logic ----

#[test]
fn logic_requires_bools() {
    assert!(boolean("true && true"));
    assert!(boolean("true || false"));
    assert!(!boolean("!true"));
    assert!(boolean("!(1 > 2)"));

    assert_eq!(
        error("1 && true"),
        Error::Type {
            operation: "&&",
            got: "int"
        }
    );
    assert_eq!(
        error("!1"),
        Error::Type {
            operation: "!",
            got: "int"
        }
    );
}

#[test]
fn logic_short_circuits() {
    let mut context = Context::new();
    context.set(
        "boom",
        Value::function(|_| {
            Err(Error::Function {
                name: "boom".to_string(),
                message: "should not have been called".to_string(),
            })
        }),
    );

    // `&&` stops at false, `||` at true, so the failing call never runs.
    assert_eq!(
        eval("false && boom()", &context).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(eval("true || boom()", &context).unwrap(), Value::Bool(true));
    assert!(eval("true && boom()", &context).is_err());
}

// ---- objects, indexes and methods ----

#[test]
fn member_and_index_access_read_injected_objects() {
    let mut context = Context::new();
    context.set(
        "step1",
        Value::map([
            ("id", Value::from("step1")),
            ("retries", Value::from(2)),
            ("nested", Value::map([("deep", Value::from(true))])),
            ("items", Value::list([10, 20])),
        ]),
    );

    assert_eq!(eval("step1.id", &context).unwrap(), Value::from("step1"));
    assert_eq!(eval("step1['retries']", &context).unwrap(), Value::from(2));
    assert_eq!(
        eval("step1.nested.deep", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(eval("step1.items[1]", &context).unwrap(), Value::from(20));
    assert_eq!(
        eval("step1.items[1] + step1.retries", &context).unwrap(),
        Value::from(22)
    );
}

#[test]
fn missing_members_and_indexes_are_errors() {
    let mut context = Context::new();
    context.set("step1", Value::map([("id", "step1")]));
    context.set("items", Value::list([1, 2]));

    assert_eq!(
        eval("step1.retries", &context),
        Err(Error::Missing {
            name: "retries".to_string()
        })
    );
    assert_eq!(
        eval("items[2]", &context),
        Err(Error::OutOfRange { index: 2, len: 2 })
    );
    assert_eq!(
        eval("items[-1]", &context),
        Err(Error::OutOfRange { index: -1, len: 2 })
    );
    assert_eq!(
        eval("items['a']", &context),
        Err(Error::Type {
            operation: "[]",
            got: "list"
        })
    );
    assert_eq!(
        eval("step1[0]", &context),
        Err(Error::Type {
            operation: "[]",
            got: "map"
        })
    );
    assert_eq!(
        eval("step1.id.upper", &context),
        Err(Error::Type {
            operation: ".",
            got: "string"
        })
    );
}

#[test]
fn injected_functions_are_called_as_functions_and_as_methods() {
    let mut context = Context::new();
    context
        .set(
            "add",
            Value::function(|args| match args {
                [Value::Int(a), Value::Int(b)] => Ok(Value::Int(a + b)),
                _ => Err(Error::Type {
                    operation: "add",
                    got: "other",
                }),
            }),
        )
        .set(
            "length",
            Value::function(|args| match args {
                // A method receives its receiver as the first argument.
                [Value::Str(text)] => Ok(Value::Int(text.chars().count() as i64)),
                _ => Err(Error::Type {
                    operation: "length",
                    got: "other",
                }),
            }),
        )
        .set("name", "workflow");

    assert_eq!(eval("add(1, 2)", &context).unwrap(), Value::from(3));
    assert_eq!(eval("add(1, 2) * 2", &context).unwrap(), Value::from(6));
    assert_eq!(eval("name.length()", &context).unwrap(), Value::from(8));
    assert_eq!(eval("length(name)", &context).unwrap(), Value::from(8));
    assert_eq!(eval("length('abc') + 1", &context).unwrap(), Value::from(4));
}

#[test]
fn an_object_carries_its_own_methods() {
    let mut context = Context::new();
    context.set(
        "step1",
        Value::map([
            ("value", Value::from(7)),
            (
                "doubled",
                Value::function(|args| match args {
                    // The receiver is the object the function was reached
                    // through, so `self.value` is readable here.
                    [Value::Map(map)] => match map.get("value") {
                        Some(Value::Int(value)) => Ok(Value::Int(value * 2)),
                        _ => Err(Error::Missing {
                            name: "value".to_string(),
                        }),
                    },
                    _ => Err(Error::Type {
                        operation: "doubled",
                        got: "other",
                    }),
                }),
            ),
        ]),
    );

    assert_eq!(eval("step1.doubled()", &context).unwrap(), Value::from(14));
    assert_eq!(
        eval("step1.doubled() + step1.value", &context).unwrap(),
        Value::from(21)
    );

    // The object's own method wins over a context method of the same name.
    context.set("doubled", Value::function(|_| Ok(Value::from(-1))));
    assert_eq!(eval("step1.doubled()", &context).unwrap(), Value::from(14));
}

#[test]
fn unknown_names_and_methods_are_errors() {
    assert_eq!(
        value("missing"),
        Err(Error::Unknown {
            name: "missing".to_string()
        })
    );
    assert_eq!(
        value("missing()"),
        Err(Error::Unknown {
            name: "missing".to_string()
        })
    );
    assert_eq!(
        value("'text'.upper()"),
        Err(Error::UnknownMethod {
            name: "upper".to_string()
        })
    );

    let mut context = Context::new();
    context.set("count", 3);
    assert_eq!(
        eval("count()", &context),
        Err(Error::Type {
            operation: "call",
            got: "int"
        })
    );
}

// ---- built-in methods ----

#[test]
fn string_methods() {
    let mut context = Context::new();
    context.set("name", "workflow-engine");

    assert_eq!(eval("name.length()", &context).unwrap(), Value::from(15));
    assert_eq!(
        eval("name.startsWith('work')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("name.startsWith('flow')", &context).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        eval("name.endsWith('engine')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("name.endsWith('work')", &context).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        eval("name.indexOf('flow')", &context).unwrap(),
        Value::from(4)
    );
    assert_eq!(
        eval("name.indexOf('nope')", &context).unwrap(),
        Value::from(-1)
    );

    // A literal receiver carries the same methods, and both `length` and
    // `indexOf` count characters, not bytes.
    assert_eq!(eval("'héllo'.length()", &context).unwrap(), Value::from(5));
    assert_eq!(
        eval("'héllo'.indexOf('llo')", &context).unwrap(),
        Value::from(2)
    );
}

#[test]
fn collection_methods() {
    let mut context = Context::new();
    context
        .set("items", Value::list([1, 2, 3]))
        .set("names", Value::list(["a", "b"]))
        .set("obj", Value::map([("k", 1), ("j", 2)]));

    assert_eq!(
        eval("items.contains(2)", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("items.contains(9)", &context).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        eval("names.contains('b')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(eval("items.length()", &context).unwrap(), Value::from(3));
    assert_eq!(eval("obj.length()", &context).unwrap(), Value::from(2));

    // `contains` on a string is a substring test.
    assert_eq!(
        eval("'abcdef'.contains('cd')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("'abcdef'.contains('ce')", &context).unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn methods_report_a_wrong_receiver_argument_or_arity() {
    let mut context = Context::new();
    context.set("count", 3).set("text", "abc");

    let function = |name: &str, message: &str| Error::Function {
        name: name.to_string(),
        message: message.to_string(),
    };

    assert_eq!(
        eval("count.length()", &context),
        Err(function("length", "expects a string, list or map, got int"))
    );
    assert_eq!(
        eval("text.startsWith(1)", &context),
        Err(function(
            "startsWith",
            "argument 1 must be a string, got int"
        ))
    );
    assert_eq!(
        eval("text.startsWith()", &context),
        Err(function("startsWith", "takes 1 argument(s), got 0"))
    );
    assert_eq!(
        eval("'a'.length(1)", &context),
        Err(function("length", "takes 0 argument(s), got 1"))
    );
    assert_eq!(
        eval("count.contains(1)", &context),
        Err(function("contains", "expects a string or list, got int"))
    );
    // A name that is not a built-in is still an unknown method.
    assert_eq!(
        eval("text.upper()", &context),
        Err(Error::UnknownMethod {
            name: "upper".to_string()
        })
    );
}

/// The lookup order is the object's own method, then the context's, then the
/// built-in one — an injected `length` is the host's to define.
#[test]
fn an_injected_method_wins_over_a_builtin_one() {
    let mut context = Context::new();
    context
        .set("text", "abc")
        .set("length", Value::function(|_| Ok(Value::from(-1))))
        .set(
            "obj",
            Value::map([
                ("length", Value::function(|_| Ok(Value::from(-2)))),
                ("k", Value::from(1)),
            ]),
        );

    assert_eq!(eval("text.length()", &context).unwrap(), Value::from(-1));
    assert_eq!(eval("obj.length()", &context).unwrap(), Value::from(-2));
}

#[cfg(feature = "regex")]
#[test]
fn is_match_matches_a_regular_expression() {
    let mut context = Context::new();
    context.set("code", "ERR-404");

    assert_eq!(
        eval("code.is_match('^ERR-[0-9]+$')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("code.is_match('^OK')", &context).unwrap(),
        Value::Bool(false)
    );

    let err = eval("code.is_match('(')", &context).unwrap_err();
    assert!(err.to_string().contains("regular expression"), "{err}");
}

#[cfg(not(feature = "regex"))]
#[test]
fn is_match_is_unknown_without_the_regex_feature() {
    assert_eq!(
        eval("'a'.is_match('a')", &Context::new()),
        Err(Error::UnknownMethod {
            name: "is_match".to_string()
        })
    );
}

// ---- nested expressions ----

#[test]
fn nested_member_and_index_paths() {
    let mut context = Context::new();
    context.set(
        "order",
        Value::map([
            ("id", Value::from("o-1")),
            (
                "customer",
                Value::map([
                    ("name", Value::from("Ada")),
                    ("tags", Value::list([Value::from("vip"), Value::from("eu")])),
                ]),
            ),
            (
                "lines",
                Value::list([
                    Value::map([("sku", Value::from("A1")), ("qty", Value::from(2))]),
                    Value::map([("sku", Value::from("B2")), ("qty", Value::from(5))]),
                ]),
            ),
        ]),
    );

    assert_eq!(
        eval("order.customer.tags[0]", &context).unwrap(),
        Value::from("vip")
    );
    assert_eq!(
        eval("order.lines[1].qty * order.lines[0].qty", &context).unwrap(),
        Value::from(10)
    );
    assert_eq!(
        eval("order.customer.name.startsWith('A')", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("order['customer']['tags'][1] == 'eu'", &context).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("order.lines[0].sku.indexOf('1')", &context).unwrap(),
        Value::from(1)
    );
}

#[test]
fn nested_calls_fold_into_one_value() {
    let mut context = Context::new();
    context
        .set("count", 2)
        .set(
            "step1",
            Value::map([("value", Value::from(7)), ("list", Value::list([3, 4]))]),
        )
        .set(
            "twice",
            Value::function(|args| match args {
                [Value::Int(value)] => Ok(Value::Int(value * 2)),
                _ => Err(Error::Function {
                    name: "twice".to_string(),
                    message: "expects an int".to_string(),
                }),
            }),
        )
        .set(
            "sum",
            Value::function(|args| {
                let total: i64 = args.iter().filter_map(Value::as_int).sum();

                Ok(Value::Int(total))
            }),
        );

    assert_eq!(
        eval("twice(twice(count))", &context).unwrap(),
        Value::from(8)
    );
    assert_eq!(
        eval("sum(step1.list[0], step1.list[1], count)", &context).unwrap(),
        Value::from(9)
    );
    assert_eq!(
        eval("twice(sum(step1.list[0], step1.value)) + count", &context).unwrap(),
        Value::from(22)
    );
}

/// The shape a step condition has: several levels of member access, a method,
/// and the operators around them.
#[test]
fn nested_predicates() {
    let mut context = Context::new();
    context
        .set("count", 3)
        .set("status", "ok")
        .set("secrets", Value::map([("TOKEN", Value::from("sk-abc"))]))
        .set(
            "step1",
            Value::map([
                ("value", Value::from(7)),
                ("items", Value::list(["a", "b"])),
            ]),
        );

    assert_eq!(
        eval(
            "status == 'ok' && (count > 2 || step1.items.contains('z'))",
            &context
        )
        .unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval(
            "!(step1.value < 5) && (secrets.TOKEN.length() >= 6 || count == 0)",
            &context
        )
        .unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        eval("count + step1.items.length() * 2 == 7", &context).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn deep_grouping_still_evaluates_to_the_right_value() {
    let mut context = Context::new();
    context.set("a", 1).set("b", 2).set("c", 3).set("d", 4);

    // ((1 + 2) * (3 + 4)) - ((1 * 2) + (3 * 4)) = 21 - 14 = 7, and 7 % 7 = 0.
    assert_eq!(
        eval(
            "((((a + b) * (c + d)) - ((a * b) + (c * d))) % 7) + a",
            &context
        )
        .unwrap(),
        Value::from(1)
    );
    assert_eq!(
        eval("(((((((((1 + 1)))))))))  * (a + 1)", &context).unwrap(),
        Value::from(4)
    );
}

// ---- the context ----

#[test]
fn context_binds_values_of_every_kind() {
    let mut context = Context::new();
    context
        .set("i", 3)
        .set("f", 1.5)
        .set("b", true)
        .set("s", "text")
        .set("n", ())
        .set("list", Value::list([1, 2]))
        .set("obj", Value::map([("k", "v")]));

    assert_eq!(eval("i + 1", &context).unwrap(), Value::from(4));
    assert_eq!(eval("f * 2", &context).unwrap(), Value::from(3.0));
    assert_eq!(eval("!b", &context).unwrap(), Value::Bool(false));
    assert_eq!(eval("s", &context).unwrap(), Value::from("text"));
    assert!(eval("n", &context).unwrap().is_null());
    assert_eq!(eval("list[0]", &context).unwrap(), Value::from(1));
    assert_eq!(eval("obj.k", &context).unwrap(), Value::from("v"));
    assert_eq!(eval("obj['k']", &context).unwrap(), Value::from("v"));
}

#[test]
fn context_is_mutable_and_inspectable() {
    let mut context = Context::new();
    assert!(context.is_empty());
    assert_eq!(context.len(), 0);

    context.set("a", 1);
    assert!(context.contains("a"));
    assert_eq!(context.len(), 1);
    assert_eq!(context.names().collect::<Vec<_>>(), ["a"]);

    // Rebinding replaces, and removing leaves the name unknown to an
    // expression that still refers to it.
    context.set("a", 2);
    assert_eq!(eval("a", &context).unwrap(), Value::from(2));

    assert_eq!(context.remove("a"), Some(Value::from(2)));
    assert_eq!(
        eval("a", &context),
        Err(Error::Unknown {
            name: "a".to_string()
        })
    );

    context.set("b", 1);
    context.clear();
    assert!(context.is_empty());
}

#[test]
fn context_collects_from_pairs() {
    let context: Context = [("a", Value::from(1)), ("b", Value::from("x"))]
        .into_iter()
        .collect();

    assert_eq!(eval("a", &context).unwrap(), Value::from(1));
    assert_eq!(eval("b", &context).unwrap(), Value::from("x"));
}

// ---- the compiled expression ----

#[test]
fn one_expression_evaluates_against_many_contexts() {
    let expr = Expr::compile("count > 2 && status == 'ok'").unwrap();
    assert_eq!(expr.references(), ["count", "status"]);

    let mut first = Context::new();
    first.set("count", 3).set("status", "ok");
    let mut second = Context::new();
    second.set("count", 1).set("status", "ok");

    assert_eq!(expr.eval(&first).unwrap(), Value::Bool(true));
    assert_eq!(expr.eval(&second).unwrap(), Value::Bool(false));
}

#[test]
fn references_name_what_the_expression_reads() {
    let expr = Expr::compile("a + step1.value > b && check(name)").unwrap();

    assert_eq!(expr.references(), ["a", "b", "check", "name", "step1"]);
    assert!(Expr::compile("1 + 2").unwrap().references().is_empty());
}

#[test]
fn an_expression_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Expr>();
    assert_send_sync::<Value>();
    assert_send_sync::<Context>();

    let expr = Expr::compile("count * 2").unwrap();

    std::thread::scope(|scope| {
        for count in 0..4 {
            let expr = &expr;
            scope.spawn(move || {
                let mut context = Context::new();
                context.set("count", count);
                assert_eq!(expr.eval(&context).unwrap(), Value::from(count * 2));
            });
        }
    });
}

// ---- what the surface rejects ----

#[test]
fn parse_errors_carry_the_position() {
    let err = Expr::compile("1 + ").unwrap_err();
    assert_eq!(err.position(), Some(4));
    assert!(err.to_string().contains("byte 4"));

    assert_eq!(Expr::compile("1 2").unwrap_err().position(), Some(2));
    assert_eq!(Expr::compile("(1").unwrap_err().position(), Some(2));
    assert_eq!(Expr::compile("a.").unwrap_err().position(), Some(2));
    assert_eq!(
        Expr::compile("'unterminated").unwrap_err().position(),
        Some(0)
    );
    assert_eq!(Expr::compile("1 = 1").unwrap_err().position(), Some(2));
    assert_eq!(Expr::compile("!").unwrap_err().position(), Some(1));
}

#[test]
fn the_absent_forms_say_so() {
    for (source, expected) in [
        ("a ? b : c", "conditional"),
        ("{'a': 1}", "object literals"),
        ("[1, 2]", "list literals"),
    ] {
        let message = Expr::compile(source).unwrap_err().to_string();
        assert!(message.contains(expected), "{source}: {message}");
    }

    let chained = Expr::compile("1 < 2 < 3").unwrap_err();
    assert!(chained.to_string().contains("do not chain"), "{chained}");
}

#[test]
fn a_deeply_nested_source_is_rejected_not_overflowed() {
    let source = format!("{}1{}", "(".repeat(200), ")".repeat(200));
    let err = Expr::compile(&source).unwrap_err();
    assert!(err.to_string().contains("nests too deeply"), "{err}");

    // The limit is well above any expression a workflow writes.
    let normal = format!("{}1{}", "(".repeat(20), ")".repeat(20));
    assert_eq!(
        Expr::compile(&normal)
            .unwrap()
            .eval(&Context::new())
            .unwrap(),
        Value::from(1)
    );
}

#[test]
fn an_int_literal_out_of_range_is_a_parse_error() {
    let err = Expr::compile("9223372036854775808").unwrap_err();
    assert!(err.to_string().contains("out of range"), "{err}");
}

// ---- json ----

#[cfg(feature = "json")]
#[test]
fn json_values_round_trip() {
    let json = serde_json::json!({
        "count": 3,
        "ratio": 0.5,
        "name": "workflow",
        "ok": true,
        "missing": null,
        "items": [1, "two", null],
        "nested": { "deep": false },
    });

    let mut context = Context::new();
    context.set("data", Value::from(json.clone()));

    assert_eq!(eval("data.count + 1", &context).unwrap(), Value::from(4));
    assert_eq!(eval("data.ratio * 2", &context).unwrap(), Value::from(1.0));
    assert_eq!(eval("data.items[1]", &context).unwrap(), Value::from("two"));
    assert_eq!(
        eval("data.nested.deep", &context).unwrap(),
        Value::Bool(false)
    );
    assert!(eval("data.missing", &context).unwrap().is_null());

    let value = eval("data", &context).unwrap();
    assert_eq!(value.to_json().unwrap(), json);
}

#[cfg(feature = "json")]
#[test]
fn json_carries_a_function_as_an_error() {
    let value = Value::function(|_| Ok(Value::Null));
    let err = value.to_json().unwrap_err();
    assert!(err.to_string().contains("JSON"), "{err}");

    let err = Value::from(f64::NAN).to_json().unwrap_err();
    assert!(err.to_string().contains("NaN"), "{err}");
}
