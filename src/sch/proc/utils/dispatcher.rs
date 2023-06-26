use tracing::info;

use super::{matcher::Matcher, subject};
use crate::{
    event::EventAction,
    sch::{Act, ActKind, TaskState},
    utils::consts,
    ActError, ActResult, Candidate, Context, Step, Vars,
};
use std::sync::Arc;

pub struct Dispatcher<'a> {
    ctx: &'a Context,
}

impl<'a> Dispatcher<'a> {
    pub fn new(ctx: &'a Context) -> Self {
        Self { ctx }
    }

    pub fn process(&self, step: &Step) -> ActResult<()> {
        self.prepare(step)?;
        self.next()?;

        Ok(())
    }

    pub fn redo(&self) -> ActResult<()> {
        info!("dispatcher::redo");
        let env = self.ctx.env();
        if self.is_role_submit() {
            if let Some(uid) = self.ctx.env().get(consts::INITIATOR) {
                self.init_submit_role_step(uid.as_str().unwrap());
            }
        } else {
            let matcher = env
                .get(consts::SUBJECT_MATCHER)
                .unwrap_or("any".into())
                .into();
            let cands = env
                .get(consts::SUBJECT_CANDS)
                .ok_or(ActError::Action(format!(
                    "cannot find '{}' in task's vars",
                    consts::SUBJECT_CANDS
                )))?
                .into();

            self.init(&matcher, &cands);
        }

        self.next()?;
        Ok(())
    }

    pub fn next(&self) -> ActResult<bool> {
        info!("dispatcher::next");
        let acts = self.ctx.task.acts();
        {
            // first to dispatch the act that is not ActKind::User
            let mut acts = acts
                .iter()
                .filter(|act| act.kind != ActKind::User && !act.active());
            while let Some(next) = acts.next() {
                self.ctx.dispatch_act(next, EventAction::Create);
                return Ok(false);
            }
        }
        {
            let acts: Vec<&Arc<Act>> = acts
                .iter()
                .filter(|act| act.kind == ActKind::User)
                .collect();
            let len = acts.len();
            if len > 0 {
                let matcher: Matcher = self
                    .ctx
                    .task
                    .env
                    .get(consts::SUBJECT_MATCHER)
                    .unwrap_or("any".into()) // default to any
                    .into();
                let result = matcher.check(self.ctx)?;
                if !result {
                    // dispatch the user acts
                    match matcher {
                        Matcher::One | Matcher::Any | Matcher::All => {
                            for act in acts.iter().filter(|act| act.active() == false) {
                                self.ctx.dispatch_act(act, EventAction::Create);
                            }
                        }
                        Matcher::Some(rule) => {
                            for act in acts.iter().filter(|act| act.active() == false) {
                                self.ctx.dispatch_act(act, EventAction::Create);
                            }
                            // send the rule Some act to client after matching
                            // it must send the the rule to client on each user complete action.
                            self.ctx.task.env.set(consts::RULE_SOME, rule.into());
                            let act = self
                                .ctx
                                .task
                                .push_act(ActKind::Some, &self.ctx.env().vars());
                            self.ctx.dispatch_act(&act, EventAction::Create);
                        }
                        Matcher::Ord(_) => {
                            let ord = self
                                .ctx
                                .task
                                .env
                                .get(consts::SUBJECT_ORD_INDEX)
                                .unwrap()
                                .as_i64()
                                .unwrap() as usize;
                            if let Some(act) = acts.iter().skip(ord + 1).next() {
                                self.ctx
                                    .task
                                    .env
                                    .set(consts::SUBJECT_ORD_INDEX, (ord + 1).into());
                                self.ctx.dispatch_act(act, EventAction::Create);
                            }
                        }
                        _ => {}
                    }

                    return Ok(false);
                }
            } else {
                if let Some(cands) = self.ctx.task.env.get(consts::SUBJECT_CANDS) {
                    let cands: Candidate = cands.into();

                    if !cands.calced() {
                        // candidate action is not completed yet
                        return Ok(false);
                    }

                    // generate acts according to the candidate
                    match cands.users() {
                        Ok(users) => {
                            for uid in users {
                                let mut vars = Vars::new();
                                vars.insert(consts::ACT_OWNER.to_string(), uid.into());
                                self.ctx.task.push_act(ActKind::User, &vars);
                            }

                            // contine to dispatch the acts
                            return self.next();
                        }
                        Err(err) => {
                            self.ctx.task.set_state(TaskState::Fail(err.into()));
                        }
                    }
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    pub fn prepare(&self, step: &Step) -> ActResult<()> {
        info!("dispatcher::prepare");
        let ctx = self.ctx;
        if let Some(key) = step.action.as_deref() {
            let mut vars = Vars::new();
            vars.insert(consts::ACT_ACTION.to_string(), key.into());
            self.ctx.task.push_act(ActKind::Action, &vars);
        }

        if let Some(script) = &step.run {
            let ret = ctx.run(script);
            if let Some(err) = ret.err() {
                ctx.task.set_state(TaskState::Fail(err.into()));
            }
        }

        match &step.subject {
            Some(sub) => {
                match subject::parse(ctx, sub) {
                    Ok((ref matcher, ref cands)) => {
                        self.init(matcher, cands);
                    }
                    Err(err) => ctx.task.set_state(TaskState::Fail(err.into())),
                };
            }
            None => {
                // prepare submit node and generate an act with initiator
                if let Some(role) = ctx.env().get(consts::STEP_ROLE) {
                    let role: &str = role.as_str().unwrap_or("");
                    if role == consts::STEP_ROLE_SUBMIT {
                        if let Some(uid) = self.ctx.env().get(consts::INITIATOR) {
                            self.init_submit_role_step(uid.as_str().unwrap());
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn init(&self, matcher: &Matcher, cands: &Candidate) {
        let ctx = self.ctx;
        ctx.task
            .env
            .set(consts::SUBJECT_MATCHER, matcher.clone().into());
        ctx.task.env.set(consts::SUBJECT_CANDS, cands.into());
        ctx.task.env.set(consts::SUBJECT_ORD_INDEX, 0.into());

        if let Matcher::Ord(Some(ord)) = matcher {
            ctx.task.env.set(consts::RULE_ORD, ord.clone().into());
        }

        if !cands.calced() {
            let act = Act::new(&ctx.task, ActKind::Candidate, &ctx.env().vars());
            ctx.proc.push_act(&act);
        } else {
            match cands.users() {
                Ok(users) => {
                    for uid in users {
                        let mut vars = Vars::new();
                        vars.insert(consts::ACT_OWNER.to_string(), uid.into());
                        ctx.task.push_act(ActKind::User, &vars);
                    }
                }
                Err(err) => {
                    ctx.task.set_state(TaskState::Fail(err.into()));
                }
            }
        }
    }

    fn is_role_submit(&self) -> bool {
        if let Some(role) = self.ctx.env().get(consts::STEP_ROLE) {
            let role: &str = role.as_str().unwrap_or("");
            return role == consts::STEP_ROLE_SUBMIT;
        }

        false
    }
    fn init_submit_role_step(&self, uid: &str) {
        info!("init_submit_role_step");
        let mut vars = Vars::new();
        vars.insert(consts::ACT_OWNER.to_string(), uid.into());
        let act = self.ctx.task.push_act(ActKind::User, &vars);
        act.set_active(true);
        if let Some(auto_submit) = self.ctx.env().get(consts::AUTO_SUBMIT) {
            let auto_submit = auto_submit.as_bool().unwrap_or(false);
            if auto_submit {
                info!("auto_submit pid={} tid={} aid={}", act.pid, act.tid, act.id);
                act.set_state(TaskState::Success);
                self.ctx.dispatch_act(&act, EventAction::Complete);

                self.ctx.task.set_state(TaskState::Success);
                self.ctx
                    .dispatch_task(&self.ctx.task, EventAction::Complete);

                // remove the auto_submit flag
                self.ctx.env().remove(consts::AUTO_SUBMIT);
            }
        } else {
            act.set_state(TaskState::WaitingEvent);
            self.ctx.dispatch_act(&act, EventAction::Create);
        }
    }
}
