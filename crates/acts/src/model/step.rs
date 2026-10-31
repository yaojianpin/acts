use crate::JoinRule;
use crate::ValidationRule;
use crate::Variant;
use crate::{ModelBase, Vars, model::Branch, model::CandidateExpr};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

/// The declared assignment of one human decision act: the expression the act
/// resolves at start into its persisted `__assignment` task state, and — for
/// multi-approver steps — the candidate pool its `uses` package computes.
/// Only a human decision leaf (`ActRunAs::Irq`) may declare an `owner`; a
/// container step (`block`/`parallel`/`sequence`) may declare `candidates`
/// for the fan-out. The kernel refuses an `assign` on a subflow or a
/// `Func`/`Msg` leaf when the act starts.
///
/// The three assignment vocabularies are separate capabilities (see
/// `crate::scheduler::shell::caps::assign`):
///
/// - `owner` — the one responsible person (`assignee`); the act's decisions
///   must come from them.
/// - `candidates` — the list of people the container fans acts out over; each
///   generated child carries its own owner.
/// - delegation — runtime state (`delegator`/`delegatee`), moved with
///   `act:delegate`/`act:undelegate`, never declared in the model.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assign {
    /// The owner: a literal user id, a `${{ … }}` expression, or absent. The
    /// resolution must contain exactly one participant — a human act has one
    /// owner; an empty or larger resolution fails the act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The candidate pool: a `uses` package computes it from `params` at act
    /// start and the answer lands in the act's variables, where the
    /// container's fan-out (`block`/`parallel`/`sequence`) reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Box<Candidates>>,
}

/// One act's candidate-pool declaration (see [`Assign::candidates`]).
///
/// `uses` names the IRQ package that computes the pool — the core
/// [`CANDIDATES_PACKAGE`] by default, or any package answering the same
/// contract (`{ participants, ids, count }`). `params` is the package's
/// input: a [`CandidateExpr`] the engine computes locally when every atom
/// resolves in the kernel, and otherwise sends to the package with its
/// local context atoms already resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Candidates {
    /// The computing package's params.
    #[serde(default)]
    pub params: CandidateExpr,
    /// The package that computes the pool. Defaults to
    /// [`CANDIDATES_PACKAGE`]; a model names its own extension here.
    #[serde(default = "default_candidates_uses")]
    pub uses: String,
}

impl Default for Candidates {
    fn default() -> Self {
        Self {
            params: CandidateExpr::default(),
            uses: default_candidates_uses(),
        }
    }
}

/// The core package that computes a candidate pool — `assign.candidates`'s
/// default `uses`. A model may name its own computing package instead; the
/// kernel only defines the request (the prepared `params`) and the answer
/// contract (`{ participants, ids, count }`).
pub const CANDIDATES_PACKAGE: &str = "acts.core.candidates";

fn default_candidates_uses() -> String {
    CANDIDATES_PACKAGE.to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub desc: String,

    #[serde(default)]
    pub id: String,

    /// define the step vars
    #[serde(default)]
    pub vars: Vec<Variant>,

    #[serde(default)]
    pub r#if: Option<String>,

    /// while condition: re-execute this step while the condition holds
    #[serde(default)]
    pub r#while: Option<String>,

    #[serde(default)]
    pub branches: Vec<Branch>,

    #[serde(default)]
    pub next: Option<String>,

    /// The join rule of this step's parallel container: how many of the
    /// branches (or the `acts.core.parallel`/`acts.core.block{ mode: parallel
    /// }` acts it fans out) must complete before the step continues. Absent
    /// (and `all`) means every branch must complete, the engine's original
    /// shape.
    #[serde(default)]
    pub join: Option<JoinRule>,

    // to use a package, such as 'acts.transform.set'
    #[serde(default)]
    pub uses: Option<String>,

    // package params
    #[serde(default)]
    pub params: JsonValue,

    /// The human act's owner declaration. Only a human decision leaf may
    /// carry one; see [`Assign`].
    #[serde(default)]
    pub assign: Option<Assign>,

    #[serde(default)]
    pub catches: Vec<Step>,

    #[serde(default)]
    pub timeouts: Vec<Step>,

    /// extra options to send to client
    #[serde(default)]
    pub options: Vars,

    /// Custom input rules: conditions over the step's scope and its resolved
    /// params, decided when the act starts. The first condition that holds
    /// runs its action (`error` by default — the step fails with
    /// `validation_failed` and the rule's message before the act ever reaches
    /// a client).
    #[serde(default)]
    pub rules: Vec<ValidationRule>,

    /// metadata to store some extra value for UI styles
    /// don't send to client
    #[serde(default)]
    pub metadata: Vars,

    /// variables to expose
    #[serde(default)]
    pub exposes: Vec<Variant>,
}

impl ModelBase for Step {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Step {
    /// Whether this step or any nested step declares an assignment (an
    /// `owner` or a `candidates` pool).
    pub fn has_assignment(&self) -> bool {
        self.assign.is_some()
            || self
                .branches
                .iter()
                .any(|branch| branch.steps.iter().any(Step::has_assignment))
            || self.catches.iter().any(Step::has_assignment)
            || self.timeouts.iter().any(Step::has_assignment)
    }
}

impl Step {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn with_id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    pub fn with_desc(mut self, desc: &str) -> Self {
        self.desc = desc.to_string();
        self
    }

    pub fn with_uses(mut self, uses: &str, params: Vars) -> Self {
        self.uses = Some(uses.to_string());
        self.params = params.into();
        self
    }

    pub fn with_uses_code(mut self, uses: &str, code: &str) -> Self {
        self.uses = Some(uses.to_string());
        self.params = code.into();
        self
    }

    pub fn with_next(mut self, next: &str) -> Self {
        self.next = Some(next.to_string());
        self
    }

    pub fn with_if(mut self, r#if: &str) -> Self {
        self.r#if = Some(r#if.to_string());
        self
    }

    pub fn with_while(mut self, r#while: &str) -> Self {
        self.r#while = Some(r#while.to_string());
        self
    }

    /// Set the step's join rule (see [`Step::join`]).
    pub fn with_join(mut self, rule: JoinRule) -> Self {
        self.join = Some(rule);
        self
    }

    /// Add one input validation (see [`Step::rules`]).
    pub fn with_validation(mut self, rule: ValidationRule) -> Self {
        self.rules.push(rule);
        self
    }

    pub fn with_var<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.vars.push(Variant::create(name, json!(value)));
        self
    }

    pub fn vars(&self) -> Vars {
        let mut ret = Vars::new();
        self.vars.iter().for_each(|var| {
            ret.set(&var.name, var.value.clone());
        });

        ret
    }

    pub fn with_option<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.options.set(name, value);
        self
    }

    /// Declare the act's owner (see [`Assign`]): a literal user id or a
    /// `${{ … }}` expression.
    pub fn with_assign(mut self, owner: &str) -> Self {
        self.assign = Some(Assign {
            owner: Some(owner.to_string()),
            candidates: None,
        });
        self
    }

    /// Declare the act's candidate pool, computed by the core
    /// [`CANDIDATES_PACKAGE`] from the expression (see [`Candidates`]).
    pub fn with_candidates(self, params: CandidateExpr) -> Self {
        self.with_candidates_package(CANDIDATES_PACKAGE, params)
    }

    /// Declare the act's candidate pool with an explicit computing package:
    /// `uses` names it, `params` is its input expression (see [`Candidates`]).
    pub fn with_candidates_package(mut self, uses: &str, params: CandidateExpr) -> Self {
        self.assign = Some(Assign {
            owner: self.assign.and_then(|assign| assign.owner),
            candidates: Some(Box::new(Candidates {
                params,
                uses: uses.to_string(),
            })),
        });
        self
    }

    pub fn with_expose(mut self, var: Variant) -> Self {
        self.exposes.push(var);
        self
    }

    pub fn exposes(&self) -> Vars {
        let mut vars = Vars::new();
        self.exposes.iter().for_each(|var| {
            vars.set(&var.name, var.value.clone());
        });
        vars
    }

    pub fn with_branch(mut self, build: fn(Branch) -> Branch) -> Self {
        let branch = Branch::default();
        self.branches.push(build(branch));
        self
    }

    pub fn with_catch(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.catches.push(build(step));
        self
    }

    pub fn with_timeout(mut self, build: fn(Step) -> Step) -> Self {
        let step = Step::default();
        self.timeouts.push(build(step));
        self
    }

    pub fn with_metadata<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize + Clone,
    {
        self.metadata.set(name, value);
        self
    }
}
