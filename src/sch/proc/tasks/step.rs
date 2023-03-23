use crate::{
    model::{Act, Step},
    sch::{proc::matcher::Matcher, Context, NodeData, TaskState},
    ActError, ActTask,
};
use async_trait::async_trait;
use core::clone::Clone;

impl Step {
    fn prepare_sub(&self, ctx: &Context) {
        if self.acts().len() == 0 {
            if let Some(sub) = &self.subject {
                let matcher = Matcher::capture(&sub.matcher);
                self.cands().matcher = matcher.clone();
                match matcher {
                    Matcher::Empty | Matcher::Error => {
                        ctx.task.set_state(&TaskState::Fail(
                            ActError::RuntimeError(
                                "subject matcher should be one of 'one', 'any', 'ord', 'ord(rule)', 'some(rule)'"
                                    .to_string(),
                            )
                            .into(),
                        ));
                    }
                    Matcher::One => {
                        let acts = self.prepare_cands(ctx, &sub.users, None);

                        if acts.len() != 1 {
                            ctx.task.set_state(&TaskState::Fail(
                                ActError::RuntimeError(
                                    "subject matcher one: the users is more then one".to_string(),
                                )
                                .into(),
                            ));
                            return;
                        }
                        if let Some(act) = acts.get(0) {
                            self.push_act(act);

                            let data = NodeData::Act(act.clone());
                            ctx.proc.tree.push_act(&data, &act.step_id);
                        }
                    }
                    Matcher::Any | Matcher::All | Matcher::Some(_) => {
                        let users = self.prepare_cands(ctx, &sub.users, None);
                        for act in &users {
                            self.push_act(act);

                            let data = NodeData::Act(act.clone());
                            ctx.proc.tree.push_act(&data, &act.step_id);
                        }
                    }
                    Matcher::Ord(ord) => {
                        let acts = self.prepare_cands(ctx, &sub.users, ord);
                        if let Some(act) = acts.get(0) {
                            self.cands().ord = 0;
                            self.push_act(&act);
                            let task = NodeData::Act(act.clone());
                            ctx.proc.tree.push_act(&task, &act.step_id);
                        }
                    }
                }
            }
        }
    }
    fn prepare_cands(&self, ctx: &Context, users: &str, ord: Option<String>) -> Vec<Act> {
        let mut acts = Vec::new();

        if users.is_empty() {
            ctx.task.set_state(&TaskState::Fail(
                ActError::RuntimeError("subject users is required".to_string()).into(),
            ));
            return acts;
        }

        let ret = ctx.eval_with::<rhai::Array>(users);
        match ret {
            Ok(users) => {
                let mut users: Vec<String> = users
                    .iter()
                    .map(|c| c.clone().into_string().unwrap())
                    .collect();
                if let Some(ord) = ord {
                    match ctx.proc.scher.ord(&ord, &users) {
                        Ok(data) => users = data,
                        Err(err) => {
                            ctx.task.set_state(&TaskState::Fail(err.into()));
                            return acts;
                        }
                    }
                }

                for user in users.iter() {
                    acts.push(Act::new(&ctx.task.nid(), user));
                }
                self.cands().acts = acts.clone();
            }
            Err(err) => ctx.task.set_state(&TaskState::Fail(err.into())),
        }

        acts
    }

    fn process_run(&self, ctx: &Context) {
        if let Some(script) = &self.run {
            let ret = ctx.run(script);
            if let Some(err) = ret.err() {
                ctx.task.set_state(&TaskState::Fail(err.into()));
            }
        }
    }

    fn process_action(&self, ctx: &Context) {
        if let Some(act) = &self.action {
            act(ctx.vm());
        }
    }
}

#[async_trait]
impl ActTask for Step {
    fn prepare(&self, _ctx: &Context) {}

    fn run(&self, ctx: &Context) {
        if let Some(expr) = &self.r#if {
            match ctx.eval(expr) {
                Ok(cond) => {
                    if cond {
                        self.process_action(ctx);
                        self.process_run(ctx);
                        self.prepare_sub(ctx);
                    } else {
                        ctx.task.set_state(&TaskState::Skip);
                    }
                }
                Err(err) => ctx.task.set_state(&TaskState::Fail(err.into())),
            }
        } else {
            self.process_action(ctx);
            self.process_run(ctx);
            self.prepare_sub(ctx);
        }
    }

    fn post(&self, ctx: &Context) {
        let acts = self.acts();
        if acts.len() > 0 {
            let matcher = &self.cands().matcher;
            match matcher.check(self, &ctx) {
                Ok(ret) => {
                    if ret {
                        ctx.task.set_state(&TaskState::Success);
                    }
                }
                Err(err) => {
                    ctx.task.set_state(&TaskState::Fail(err.into()));
                }
            }

            if ctx.task.state().is_completed() {
                // skip all children tasks
                for tid in &ctx.task.children() {
                    if let Some(act) = ctx.proc.task(tid) {
                        if act.state().is_waiting() {
                            act.set_state(&TaskState::Skip);
                        }
                    }
                }
            }
        } else {
            ctx.task.set_state(&TaskState::Success);
        }
    }
}
