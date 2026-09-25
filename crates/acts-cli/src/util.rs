use acts_channel::{
    ActionResult,
    model::{Expr, Filter, OrderBy, PageData, Query},
};
use chrono::prelude::*;
use serde_json::json;

pub const CLAP_STYLING: clap::builder::styling::Styles = clap::builder::styling::Styles::styled()
    .header(clap_cargo::style::HEADER)
    .usage(clap_cargo::style::USAGE)
    .literal(clap_cargo::style::LITERAL)
    .placeholder(clap_cargo::style::PLACEHOLDER)
    .error(clap_cargo::style::ERROR)
    .valid(clap_cargo::style::VALID)
    .invalid(clap_cargo::style::INVALID);

pub fn local_time(millis: i64) -> String {
    if millis == 0 {
        return "(nil)".to_string();
    }
    match Local.timestamp_millis_opt(millis) {
        chrono::LocalResult::Single(dt) => format!("{}", dt.format("%Y-%m-%d %H:%M:%S")),
        _ => "".to_string(),
    }
}

pub fn size(bits: i32) -> String {
    let mut ret = String::new();
    if bits < 1024 {
        ret.push_str(&format!("{}b", bits));
    } else {
        let kb = bits / 1024;
        if kb < 1024 {
            ret.push_str(&format!("{}kb", kb));
        } else {
            let m = kb / 1024;
            if m < 1024 {
                ret.push_str(&format!("{}m", m));
            }
        }
    }

    ret
}

pub fn print_pager<T>(out: &mut String, data: &PageData<T>) {
    out.push_str(&format!(
        "total {}, page {} of {} ",
        data.count, data.page_num, data.page_count
    ));
}

pub fn print_cost<T>(out: &mut String, resp: &ActionResult<PageData<T>>) {
    let cost = resp.end_time - resp.start_time;
    out.push_str(&format!("(elapsed {cost}ms)"));
}

/// Render a response value for the REPL. The value came over the wire, so a
/// value the writer cannot render is reported instead of panicking the REPL.
pub fn to_json<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    serde_json::to_string_pretty(value)
        .map_err(|err| anyhow::anyhow!("failed to render the response as json: {err}"))
}

/// Render a response value as yaml — the counterpart of [`to_json`].
pub fn to_yaml<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    serde_yaml::to_string(value)
        .map_err(|err| anyhow::anyhow!("failed to render the response as yaml: {err}"))
}

pub fn parse_sort(s: &str) -> Result<OrderBy, anyhow::Error> {
    Ok(s.parse()?)
}

/// Parse one `TARGET=GLOB[,GLOB]` snapshot grant for `auth user set`:
/// `secrets=$subject` owns the caller's own `secrets` scope, `profile=*`
/// every scope of `profile`, `a=u1,b=*` two of them.
pub fn parse_snapshot(s: &str) -> Result<(String, Vec<String>), anyhow::Error> {
    let (target, modes) = s
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("'{s}' is not TARGET=GLOB[,GLOB]"))?;
    let target = target.trim();
    if target.is_empty() {
        return Err(anyhow::anyhow!("a snapshot grant needs a target name"));
    }
    let modes: Vec<String> = modes
        .split(',')
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .map(str::to_string)
        .collect();
    if modes.is_empty() {
        return Err(anyhow::anyhow!(
            "snapshot grant '{s}' names no scope pattern"
        ));
    }
    Ok((target.to_string(), modes))
}

/// Read a password from the terminal, without echo when the platform allows
/// it. The prompt and the read are one line of stderr so a password never
/// ends up in the transcript.
pub fn prompt_password() -> anyhow::Result<String> {
    use std::io::Write;
    write!(std::io::stderr(), "password: ")
        .map_err(|err| anyhow::anyhow!("failed to write the prompt: {err}"))?;
    std::io::stderr().flush().ok();
    let mut buffer = String::new();
    std::io::stdin()
        .read_line(&mut buffer)
        .map_err(|err| anyhow::anyhow!("failed to read the password: {err}"))?;
    let password = buffer.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        return Err(anyhow::anyhow!("no password given"));
    }
    Ok(password)
}

pub fn parse_key_value(s: &str) -> Result<Expr, anyhow::Error> {
    let pos = s
        .find(['=', '~', '>', '<'])
        .ok_or_else(|| anyhow::anyhow!("invalid KEY=value: no `=` or `~` found in `{s}`"))?;

    let key = s[..pos].as_ref();
    let value = s[pos + 1..].as_ref();
    let op: &str = s[pos..=pos].as_ref();
    let expr = match op {
        "=" => Expr::eq(key, value),
        "~" => Expr::matches(key, value),
        ">" => Expr::gt(key, value),
        "<" => Expr::lt(key, value),
        _ => {
            return Err(anyhow::anyhow!(
                "invalid OP '{op}', it should be one of '=', '~', '>' and '<'"
            ));
        }
    };

    Ok(expr)
}

pub fn parse_options(s: &str) -> Result<(String, String), anyhow::Error> {
    let pos = s
        .find(['='])
        .ok_or_else(|| anyhow::anyhow!("invalid KEY=value: no `=` found in `{s}`"))?;

    let key = s[..pos].to_string();
    let value = s[pos + 1..].to_string();
    Ok((key, value))
}

/// Parse one `KEY=value` option whose value may be written as JSON or as a
/// bare word (see [`parse_json`]).
pub fn parse_key_json<T>(s: &str) -> anyhow::Result<(T, serde_json::Value)>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| anyhow::anyhow!("invalid KEY=value: no `=` found in `{s}`"))?;

    Ok((s[..pos].parse()?, parse_json(&s[pos + 1..])?))
}

/// Parse a value the user typed: `1` is a number, `null` is null, `[2, 3]` an
/// array and `{"a": 1}` an object, while anything else — `abc`, `abc123`,
/// `hello world` — is the string it looks like.
///
/// An object or array the writer rejects is a typo in the user's own json, so
/// it is reported rather than quietly passed on as text.
pub fn parse_json(s: &str) -> anyhow::Result<serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(value) => Ok(value),
        // the writer's own message says *where* the text stopped being json,
        // which is the half a typo needs; clap renders what it is handed, so
        // the detail travels in the message rather than behind it
        Err(err) if s.trim_start().starts_with(['{', '[']) => {
            Err(anyhow::anyhow!("invalid json `{s}`: {err}"))
        }
        Err(_) => Ok(json!(s)),
    }
}

pub fn to_query(
    offset: &Option<u32>,
    count: &Option<u32>,
    query_by: &Vec<Expr>,
    order_by: &Vec<OrderBy>,
) -> Query {
    let mut query = Query::new();
    if !query_by.is_empty() {
        let mut filter = Filter::and();
        for expr in query_by {
            filter = filter.expr(expr.clone());
        }
        query = query.filter(filter);
    }

    if !order_by.is_empty() {
        for ob in order_by {
            query = query.order(&ob.field, ob.order.clone());
        }
    }

    if let Some(offset) = offset {
        query = query.offset(*offset as usize);
    };
    if let Some(count) = count {
        query = query.limit(*count as usize);
    };

    query
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_value_parses_every_operator() {
        use acts_channel::model::ExprOp;

        assert_eq!(parse_key_value("name=approve").unwrap().op, ExprOp::EQ);
        assert_eq!(parse_key_value("name~app").unwrap().op, ExprOp::Match);
        assert_eq!(parse_key_value("count>2").unwrap().op, ExprOp::GT);
        assert_eq!(parse_key_value("count<2").unwrap().op, ExprOp::LT);
    }

    /// A filter without an operator is the user's typo: the error names the
    /// input instead of panicking on the missing separator.
    #[test]
    fn key_value_without_an_operator_is_rejected() {
        let err = parse_key_value("approve").unwrap_err();
        assert!(
            err.to_string().contains("approve"),
            "the error must quote the input: {err}"
        );
    }

    #[test]
    fn options_split_on_the_first_equals() {
        assert_eq!(
            parse_options("tag=a=b").unwrap(),
            ("tag".to_string(), "a=b".to_string())
        );
        assert!(parse_options("tag").is_err());
    }

    /// A bare word is the string it looks like — digits and all — while json
    /// is parsed, and a malformed object is reported as the typo it is.
    #[test]
    fn json_values_quote_and_parse() {
        assert_eq!(parse_json("abc").unwrap(), json!("abc"));
        assert_eq!(parse_json("abc123").unwrap(), json!("abc123"));
        assert_eq!(parse_json("hello world").unwrap(), json!("hello world"));
        assert_eq!(parse_json("1").unwrap(), json!(1));
        assert_eq!(parse_json("-2.5").unwrap(), json!(-2.5));
        assert_eq!(parse_json("null").unwrap(), json!(null));
        assert_eq!(parse_json("[2, 3]").unwrap(), json!([2, 3]));
        assert_eq!(parse_json(r#"{"a": 1}"#).unwrap(), json!({"a": 1}));

        let err = parse_json(r#"{"a": 1"#).unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid json"),
            "a malformed object must be reported: {err:#}"
        );
    }

    #[test]
    fn key_json_parses_the_key_and_the_value() {
        let (key, value) = parse_key_json::<String>("a=abc123").unwrap();
        assert_eq!(key, "a");
        assert_eq!(value, json!("abc123"));

        let (key, value) = parse_key_json::<String>(r#"d={"value": 100}"#).unwrap();
        assert_eq!(key, "d");
        assert_eq!(value, json!({"value": 100}));

        let (key, value) = parse_key_json::<String>("e=null").unwrap();
        assert_eq!(key, "e");
        assert_eq!(value, json!(null));

        assert!(parse_key_json::<String>("novalue").is_err());
    }

    #[test]
    fn sort_parses_the_direction() {
        use acts_channel::model::Sort;

        let order = parse_sort("create_time!").unwrap();
        assert_eq!(order.field, "create_time");
        assert_eq!(order.order, Sort::Desc);

        let order = parse_sort("create_time").unwrap();
        assert_eq!(order.order, Sort::Asc);
    }

    #[test]
    fn query_carries_the_filters_and_the_window() {
        let plain = to_query(&None, &None, &vec![], &vec![]);
        let query = to_query(
            &Some(10),
            &Some(5),
            &vec![parse_key_value("name=approve").unwrap()],
            &vec![parse_sort("create_time!").unwrap()],
        );
        assert_eq!(query.offset, 10);
        assert_eq!(query.limit, 5);
        assert_eq!(query.order_by.len(), 1);
        assert!(query.filter.is_some());

        // no filters and no window: the query keeps the unset defaults
        assert_eq!(plain.offset, 0);
        assert_eq!(plain.limit, 100_000);
        assert!(plain.filter.is_none());
        assert!(plain.order_by.is_empty());
    }

    #[test]
    fn size_renders_bytes_kilobytes_and_megabytes() {
        assert_eq!(size(512), "512b");
        assert_eq!(size(2048), "2kb");
        assert_eq!(size(3 * 1024 * 1024), "3m");
    }

    #[test]
    fn local_time_marks_the_unset_stamp() {
        assert_eq!(local_time(0), "(nil)");
        assert!(local_time(1_700_000_000_000).starts_with("2023-"));
    }
}
