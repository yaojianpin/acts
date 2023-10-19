use super::Rule;
use crate::{
    event::ActionState, sch::ActTask, utils::consts, ActError, ActFor, Candidate, Context, Result,
    Vars,
};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};

impl ActTask for ActFor {
    fn init(&self, ctx: &Context) -> Result<()> {
        // disable emit message because it will generate the sub acts
        ctx.task.set_emit_disabled(true);
        let (_, cand) = self.parse(ctx, &self.r#in)?;
        let mut cands = Vec::new();
        if !cand.calculable(&mut cands) {
            // generate group init action
            let mut cands_init = HashMap::new();
            for v in cands {
                cands_init.insert(v.id()?, json!(null));
            }

            let mut inputs = Vars::new();
            inputs.insert(consts::FOR_ACT_KEY_USERS.to_string(), json!(cands_init));

            let mut outputs = Vars::new();
            outputs.insert(consts::FOR_ACT_KEY_USERS.to_string(), json!(null));
            ctx.sched_act(&self.init_key(), &self.tag(ctx), &inputs, &outputs)?;
        } else {
            let users = cand
                .users(&HashMap::new())?
                .into_iter()
                .collect::<Vec<_>>()
                .into();
            ctx.set_var(consts::FOR_ACT_KEY_CANDS, users);

            // generate the cands acts
            let rule = Rule::parse(&self.by)?;
            rule.next(ctx, self)?;
        }
        Ok(())
    }

    fn run(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let rule = Rule::parse(&self.by)?;
        if rule.check(ctx, self)? {
            ctx.task.set_action_state(ActionState::Completed);
        } else {
            return rule.next(ctx, self);
        }

        Ok(false)
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let rule = Rule::parse(&self.by)?;
        if rule.check(ctx, self)? {
            ctx.task.set_action_state(ActionState::Completed);
            return Ok(true);
        }

        let is_next = rule.next(ctx, self)?;
        return Ok(!is_next);
    }
}

impl ActFor {
    pub fn parse(&self, ctx: &Context, scr: &str) -> Result<(Rule, Candidate)> {
        if scr.is_empty() {
            return Err(ActError::Runtime("act.for's in is empty".to_string()));
        }
        let rule = Rule::parse(&self.by)?;
        let result = ctx.eval_with::<rhai::Dynamic>(scr)?;
        let cand = Candidate::parse(&result.to_string())?;
        Ok((rule, cand))
    }

    pub fn tag(&self, ctx: &Context) -> String {
        let mut tag = ctx.task.node.tag();
        if tag.is_empty() {
            tag = consts::FOR_ACT_TAG.to_string();
        }

        tag
    }

    pub fn init_key(&self) -> String {
        self.alias
            .init
            .clone()
            .unwrap_or(consts::FOR_ACT_CTOR.to_string())
    }

    pub fn each_key(&self) -> String {
        self.alias
            .each
            .clone()
            .unwrap_or(consts::FOR_ACT_EACH.to_string())
    }

    pub fn calc(
        &self,
        ctx: &Context,
        values: &HashMap<String, Vec<String>>,
    ) -> Result<BTreeSet<String>> {
        let (_, cand) = self.parse(ctx, &self.r#in)?;
        let users = cand.users(values)?;
        Ok(users)
    }
}
