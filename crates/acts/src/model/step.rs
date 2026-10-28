use crate::JoinRule;
use crate::ValidationRule;
use crate::Variant;
use crate::{ModelBase, Vars, model::Branch, model::OwnerExpr};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};

/// The declared owner of one human decision act: the expression the act
/// resolves at start into its persisted `__assignment` task state. Only a
/// human decision leaf (`ActRunAs::Irq`) may declare one; the kernel refuses
/// an `assign` on a container or a `Func`/`Msg` leaf when the act starts.
///
/// `owner` is an [`OwnerExpr`]. The legacy string form (a user id or a
/// `${{ … }}` expression) deserializes unchanged; the object forms name a
/// user/role/group, a resource's owners, a context variable, a relation, or a
/// set combination of those. The resolved set must contain exactly one
/// participant — a human act has one owner. Multi-approver steps use the
/// `acts.core.owner` package with `block`/`parallel`/`sequence` instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assign {
    /// The owner expression. An empty resolution fails the act; so does a
    /// resolution with more than one participant.
    #[serde(default)]
    pub owner: Box<OwnerExpr>,
    /// The capability provider key that resolves this assignment.
    ///
    /// A model may declare it explicitly; the publish path fills the only
    /// registered provider when omitted, so the stored model carries the
    /// frozen selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
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
    /// Whether this step or any nested step declares `assign.owner`.
    ///
    /// The engine checks this when publishing a model: `assign.owner` is a
    /// participant primitive, and without a participant resolver the model
    /// must be refused with a message naming the plugin rather than accepted
    /// and failing later.
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

    /// Declare the act's owner expression as the legacy string form (see
    /// [`Assign`]): a literal user id or a `${{ … }}` expression.
    pub fn with_assign(mut self, owner: &str) -> Self {
        self.assign = Some(Assign {
            owner: Box::new(OwnerExpr::Value(owner.to_string())),
            provider: None,
        });
        self
    }

    /// Declare the act's owner as a structured [`OwnerExpr`] (a resource's
    /// owners, a context variable, a relation, or a set combination).
    pub fn with_assign_expr(mut self, owner: OwnerExpr) -> Self {
        self.assign = Some(Assign {
            owner: Box::new(owner),
            provider: None,
        });
        self
    }

    /// Choose the capability provider that resolves this step's `assign.owner`.
    ///
    /// A no-op when the step has no assignment. The publish path validates the
    /// key against the installed providers and freezes it into the model.
    pub fn with_assign_provider(mut self, provider: impl Into<String>) -> Self {
        if let Some(assign) = &mut self.assign {
            assign.provider = Some(provider.into());
        }
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
