use super::CommandRunner as Command;
use crate::{client, util};
use acts_channel::{
    Vars,
    model::{Expr, GraphNode, OrderBy, PageData, ProcInfo, ProcessGraph},
};
use clap::{Args, Subcommand};
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use serde_json::Value;

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub struct ProcArgs {
    #[command(subcommand)]
    pub command: ProcCommands,
}

#[derive(Debug, Subcommand)]
pub enum ProcCommands {
    #[command(about = "get proc by id")]
    Get {
        #[arg(help = "proc id")]
        id: String,
        #[arg(short, long, help = "format to print, the value should be one of json and tree", value_parser(["json", "tree"]))]
        fmt: Option<String>,
    },
    #[command(about = "show a proc's step graph")]
    Graph {
        #[arg(help = "proc id")]
        id: String,
        #[arg(
            short = 'v',
            long = "var",
            value_name = "NAME",
            num_args = 0..=1,
            default_missing_value = "*",
            action = clap::ArgAction::Append,
            help = "show the named step/branch vars; repeat for more than one.\na bare --var shows every declared var"
        )]
        vars: Vec<String>,
        #[arg(
            short = 'q',
            long = "query",
            value_name = "KEY[=VALUE]",
            value_parser = parse_query,
            action = clap::ArgAction::Append,
            help = "keep only steps matching every KEY[=VALUE] given.\n'uses' addresses the step's package (wildcards * ? allowed);\nany other KEY addresses its params (dots nest: a.b)"
        )]
        query: Vec<(String, Option<Value>)>,
        #[arg(
            short = 'x',
            short_alias = 'X',
            long = "exclude-query",
            visible_alias = "without-query",
            value_name = "KEY[=VALUE]",
            value_parser = parse_query,
            action = clap::ArgAction::Append,
            help = "hide steps matching any KEY[=VALUE] given.\n'uses' addresses the step's package (wildcards * ? allowed);\nany other KEY addresses its params (dots nest: a.b)"
        )]
        exclude_query: Vec<(String, Option<Value>)>,
    },
    #[command(about = "list all procs")]
    Ls {
        #[arg(short, long, help = "skip the offset number to begin count")]
        offset: Option<u32>,
        #[arg(short, long, help = "expect to load the max count")]
        count: Option<u32>,
        #[arg(short='Q', long, help = "query by keys. \nexample: -Q mid=approve", value_parser = util::parse_key_value)]
        query_by: Vec<Expr>,
        #[arg(short='O', long, help = "order by keys. '!' means order by desc. \nexample: -O mid -O start_time -O end_time!", value_parser = util::parse_sort)]
        order_by: Vec<OrderBy>,
    },
    #[command(about = "deploy a workflow model")]
    Start {
        #[arg(required = true, help = "proc id")]
        id: String,
        #[arg(short, long, help = "specify a pid for proc")]
        pid: Option<String>,
    },
}

pub async fn process(parent: &mut Command<'_>, command: &ProcCommands) -> anyhow::Result<()> {
    let ret = match command {
        ProcCommands::Get { id, fmt } => get(parent, id, fmt).await,
        ProcCommands::Graph {
            id,
            vars,
            query,
            exclude_query,
        } => graph(parent, id, vars, query, exclude_query).await,
        ProcCommands::Ls {
            offset,
            count,
            query_by,
            order_by,
        } => ls(parent, offset, count, query_by, order_by).await,
        ProcCommands::Start { id, pid } => start(parent, id, pid, &parent.vars.clone()).await,
    }?;

    parent.output(&ret);
    Ok(())
}

pub async fn start(
    parent: &mut Command<'_>,
    mid: &str,
    pid: &Option<String>,
    vars: &Vars,
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let mut options = Vars::new().extend(vars);
    if let Some(pid) = pid {
        options.set("pid", pid);
    }
    let resp = parent.start(mid, options).await?;

    let pid = client::payload("proc:start", resp.data)?;
    ret.push_str(&format!("pid={pid}"));
    // print the elapsed
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

pub async fn get(
    parent: &mut Command<'_>,
    pid: &str,
    fmt: &Option<String>,
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let mut options = Vars::new();
    options.set("pid", pid);

    if let Some(fmt) = fmt {
        options.set("fmt", fmt);
    };

    let resp = parent.send::<ProcInfo>("proc:get", options).await?;

    let proc = client::payload("proc:get", resp.data)?;
    ret.push_str(&util::to_json(&proc)?);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

/// One step predicate, shared by `--query` (include) and `--exclude-query`
/// (veto). `uses` addresses the step's package (wildcards allowed); every
/// other key addresses its params. A bare key matches on presence, a pair on
/// equality.
#[derive(Debug, Clone)]
struct QueryPredicate {
    key: String,
    value: Option<Value>,
}

impl QueryPredicate {
    fn matches(&self, node: &GraphNode) -> bool {
        if self.key == "uses" {
            let Some(uses) = node.uses.as_deref() else {
                return false;
            };
            return self
                .value
                .as_ref()
                .is_none_or(|pattern| pattern.as_str().is_some_and(|p| wildcard_match(p, uses)));
        }
        let Some(actual) = param_value(&node.params, &self.key) else {
            return false;
        };
        self.value
            .as_ref()
            .is_none_or(|expected| actual == expected)
    }
}

/// `KEY[=VALUE]`: a bare key matches on presence, a pair on equality. The
/// value is read as JSON where it parses, so `count=2` is a number.
fn parse_query(s: &str) -> anyhow::Result<(String, Option<Value>)> {
    let (key, value) = match s.split_once('=') {
        Some((key, value)) => (key, Some(util::parse_json(value)?)),
        None => (s, None),
    };
    if key.is_empty() {
        return Err(anyhow::anyhow!("a query needs a key: `{s}`"));
    }
    Ok((key.to_string(), value))
}

#[derive(Debug, Default)]
struct GraphFilter {
    /// Include: every predicate must hold.
    query: Vec<QueryPredicate>,
    /// Veto: any matching predicate hides the step.
    exclude_query: Vec<QueryPredicate>,
}

impl GraphFilter {
    fn is_active(&self) -> bool {
        !self.query.is_empty() || !self.exclude_query.is_empty()
    }

    /// Inclusions run first: the step must match every `--query` predicate,
    /// then any `--exclude-query` veto wins. A step that declares a filtered
    /// property not at all is hidden by an inclusion and untouched by an
    /// exclusion. Structural nodes (`workflow`, `branch`, `block`) have no
    /// step properties of their own; the tree decides whether they are
    /// needed for a visible descendant.
    fn matches(&self, node: &GraphNode) -> bool {
        if !self.query.is_empty() && !self.query.iter().all(|include| include.matches(node)) {
            return false;
        }
        !self
            .exclude_query
            .iter()
            .any(|exclude| exclude.matches(node))
    }
}

/// `proc:graph` prints the run's declared graph, optionally narrowed to the
/// steps the filters admit and with the requested step/branch vars appended
/// to each line. A hidden step takes its whole subtree with it.
pub async fn graph(
    parent: &mut Command<'_>,
    pid: &str,
    vars: &[String],
    query: &[(String, Option<Value>)],
    exclude_query: &[(String, Option<Value>)],
) -> anyhow::Result<String> {
    let resp = parent
        .send::<ProcessGraph>("proc:graph", Vars::new().with("pid", pid))
        .await?;
    let graph = client::payload("proc:graph", resp.data)?;

    let predicates = |filters: &[(String, Option<Value>)]| -> Vec<QueryPredicate> {
        filters
            .iter()
            .map(|(key, value)| QueryPredicate {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    };
    let selection = GraphFilter {
        query: predicates(query),
        exclude_query: predicates(exclude_query),
    };
    let mut ret = render_graph(&graph, vars, &selection);
    let cost = resp.end_time - resp.start_time;
    ret.push_str(&format!("(elapsed {cost}ms)"));

    Ok(ret)
}

fn render_graph(graph: &ProcessGraph, vars: &[String], filter: &GraphFilter) -> String {
    GraphRenderer {
        graph,
        vars,
        filter,
        output: String::new(),
    }
    .render()
}

struct GraphRenderer<'a> {
    graph: &'a ProcessGraph,
    vars: &'a [String],
    filter: &'a GraphFilter,
    output: String,
}

impl GraphRenderer<'_> {
    fn render(mut self) -> String {
        let mut ancestors = Vec::new();
        for root in self.graph.nodes.iter().filter(|node| node.parent.is_none()) {
            self.write_node(root, &mut ancestors, true, true);
        }
        self.output
    }

    fn write_node(
        &mut self,
        node: &GraphNode,
        ancestors: &mut Vec<bool>,
        is_last: bool,
        is_root: bool,
    ) {
        if !is_root {
            for ancestor_is_last in ancestors.iter() {
                self.output
                    .push_str(if *ancestor_is_last { "    " } else { "│   " });
            }
            self.output.push_str(if is_last { "└── " } else { "├── " });
        }
        let fact = |value: &Option<String>| value.clone().unwrap_or_else(|| "nil".to_string());
        self.output.push_str(&format!(
            "{} id:{} name={}  uses={} state={} next={}",
            node.kind,
            node.nid,
            node.name,
            fact(&node.uses),
            fact(&node.state),
            fact(&node.next),
        ));
        write_vars(node, self.vars, &mut self.output);
        self.output.push('\n');

        let children: Vec<&GraphNode> = visible_children(self.graph, node)
            .into_iter()
            .filter(|child| node_visible(self.graph, child, self.filter))
            .collect();
        let len = children.len();
        for (index, child) in children.iter().enumerate() {
            if !is_root {
                ancestors.push(is_last);
            }
            self.write_node(child, ancestors, index + 1 == len, false);
            if !is_root {
                ancestors.pop();
            }
        }
    }
}

/// Whether the filter keeps this node. A hidden step takes its subtree with
/// it; a structural node survives only while it still holds a visible
/// descendant.
fn node_visible(graph: &ProcessGraph, node: &GraphNode, filter: &GraphFilter) -> bool {
    if !filter.is_active() {
        return true;
    }
    if node.kind == "step" {
        return filter.matches(node);
    }
    visible_children(graph, node)
        .iter()
        .any(|child| node_visible(graph, child, filter))
}

fn write_vars(node: &GraphNode, vars: &[String], output: &mut String) {
    if vars.is_empty() {
        return;
    }
    let mut selected = serde_json::Map::new();
    for name in vars {
        if name == "*" {
            for var in &node.vars {
                selected.insert(var.name.clone(), var.value.clone());
            }
        } else if let Some(var) = node.vars.iter().find(|var| &var.name == name) {
            selected.insert(name.clone(), var.value.clone());
        }
    }
    if !selected.is_empty() {
        output.push_str(&format!(" vars={}", Value::Object(selected)));
    }
}

/// One node's direct children by execution identity: an executed node's
/// children are the executions it admitted; a declaration with no execution
/// owns the placeholders sharing its enclosing execution.
fn graph_children<'a>(graph: &'a ProcessGraph, node: &GraphNode) -> Vec<&'a GraphNode> {
    match node.tid.as_deref() {
        Some(tid) => graph
            .nodes
            .iter()
            .filter(|child| child.parent_tid.as_deref() == Some(tid))
            .collect(),
        None => graph
            .nodes
            .iter()
            .filter(|child| {
                child.tid.is_none()
                    && child.parent_tid == node.parent_tid
                    && child.parent.as_deref() == Some(node.nid.as_str())
            })
            .collect(),
    }
}

/// The children a tree line actually shows: every virtual `block` is replaced
/// by its own visible children, recursively.
fn visible_children<'a>(graph: &'a ProcessGraph, node: &GraphNode) -> Vec<&'a GraphNode> {
    fn collect<'a>(graph: &'a ProcessGraph, node: &GraphNode, visible: &mut Vec<&'a GraphNode>) {
        for child in graph_children(graph, node) {
            if child.kind == "block" {
                collect(graph, child, visible);
            } else {
                visible.push(child);
            }
        }
    }

    let mut visible = Vec::new();
    collect(graph, node, &mut visible);
    visible
}

fn param_value<'a>(params: &'a Value, key: &str) -> Option<&'a Value> {
    if key.is_empty() {
        return None;
    }
    key.split('.')
        .try_fold(params, |value, segment| value.get(segment))
}

/// `*` matches any run of characters, `?` one character; a pattern without
/// wildcards is an exact match.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains(['*', '?']) {
        return pattern == text;
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut dp = vec![false; text.len() + 1];
    dp[0] = true;
    for token in pattern {
        if token == '*' {
            for index in 1..=text.len() {
                dp[index] = dp[index] || dp[index - 1];
            }
        } else {
            for index in (1..=text.len()).rev() {
                dp[index] = dp[index - 1] && (token == '?' || token == text[index - 1]);
            }
            dp[0] = false;
        }
    }
    dp[text.len()]
}

pub async fn ls(
    parent: &mut Command<'_>,
    offset: &Option<u32>,
    count: &Option<u32>,
    query_by: &Vec<Expr>,
    order_by: &Vec<OrderBy>,
) -> anyhow::Result<String> {
    let mut ret = String::new();
    let query = util::to_query(offset, count, query_by, order_by);
    let resp = parent
        .send::<PageData<ProcInfo>>("proc:ls", Vars::new().with("query", query))
        .await?;
    let data = client::payload("proc:ls", resp.data.as_ref())?;
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["pid", "name", "model id", "state", "start time"]);
    for p in &data.rows {
        table.add_row(vec![
            p.id.to_string(),
            p.name.to_string(),
            p.mid.to_string(),
            p.state.to_string(),
            util::local_time(p.start_time),
        ]);
    }
    println!("{table}");
    util::print_pager(&mut ret, data);
    util::print_cost(&mut ret, &resp);

    Ok(ret)
}
