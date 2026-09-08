use acts_channel::{
    ActionResult,
    model::{Expr, Filter, OrderBy, PageData, Query},
};
use chrono::prelude::*;
use serde_json::json;
use std::error::Error;

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

pub fn parse_sort(s: &str) -> Result<OrderBy, anyhow::Error> {
    Ok(s.parse()?)
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

pub fn parse_key_json<T>(
    s: &str,
) -> Result<(T, serde_json::Value), Box<dyn Error + Send + Sync + 'static>>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;

    let mut v = s[pos + 1..].to_string();
    let re_not_str =
        regex::Regex::new(r#"([+-]?\d+(\.\d+)?([Ee][+-]?\d+)?)|(\{.*\})|(\[.*\])|null"#).unwrap();
    if !re_not_str.is_match(&v) {
        v = format!(r#""{v}""#);
    }
    Ok((s[..pos].parse()?, serde_json::from_str(&v)?))
}

pub fn parse_json(s: &str) -> Result<serde_json::Value, Box<dyn Error + Send + Sync + 'static>> {
    let re_not_str =
        regex::Regex::new(r#"([+-]?\d+(\.\d+)?([Ee][+-]?\d+)?)|(\{.*\})|(\[.*\])|null"#).unwrap();
    if !re_not_str.is_match(s) {
        return Ok(json!(s));
    }
    Ok(serde_json::from_str(s)?)
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
