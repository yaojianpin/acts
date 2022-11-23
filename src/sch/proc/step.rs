use crate::{
    debug,
    model::{Act, Step},
    sch::{proc::matcher::Matcher, ActId, Context, Task, TaskState},
    ActError, ActTask,
};
use async_trait::async_trait;
use core::clone::Clone;

impl_act_state!(Step);
impl_act_time!(Step);
impl_act_acts!(Step);
impl_act_id!(Step);

impl Step {
    fn prepare_sub(&self, ctx: &Context) {
        if self.acts().len() == 0 {
            if let Some(sub) = &self.subject {
                let matcher = Matcher::capture(&sub.matcher);
                self.set_matcher(&matcher);

                match matcher {
                    Matcher::Empty | Matcher::Error => {
                        self.set_state(&TaskState::Fail(
                            ActError::SubjectError(
                                "matcher should be one of one, any, ord, ord(rule), some(rule)"
                                    .to_string(),
                            )
                            .into(),
                        ));
                    }
                    Matcher::One => {
                        let acts = self.prepare_users(ctx, &sub.users, None);

                        if acts.len() != 1 {
                            self.set_state(&TaskState::Fail(
                                ActError::SubjectError(
                                    "matcher: the users is more then one".to_string(),
                                )
                                .into(),
                            ));
                            return;
                        }
                        if let Some(act) = acts.get(0) {
                            act.set_state(&TaskState::WaitingEvent);
                            self.push_act(act);

                            let task = Task::Act(act.id(), act.clone());
                            ctx.proc.tree.push_act(&task, &act.step_task_id);

                            ctx.send_message(&act.owner, &task);
                        }
                    }
                    Matcher::Any | Matcher::All | Matcher::Some(_) => {
                        let users = self.prepare_users(ctx, &sub.users, None);

                        for act in &users {
                            act.set_state(&TaskState::WaitingEvent);
                            self.push_act(act);

                            let task = Task::Act(act.id(), act.clone());
                            ctx.proc.tree.push_act(&task, &act.step_task_id);

                            ctx.send_message(&act.owner, &task);
                        }
                    }
                    Matcher::Ord(ord) => {
                        let acts = self.prepare_users(ctx, &sub.users, ord);
                        if let Some(act) = acts.get(0) {
                            act.set_state(&TaskState::WaitingEvent);
                            self.set_ord(0);
                            self.push_act(&act);
                            let task = Task::Act(act.id(), act.clone());
                            ctx.proc.tree.push_act(&task, &act.step_task_id);

                            ctx.send_message(&act.owner, &task);
                        }
                    }
                }
            }
        }
    }
    fn prepare_users(&self, ctx: &Context, users: &str, ord: Option<String>) -> Vec<Act> {
        let mut acts = Vec::new();

        if users.is_empty() {
            self.set_state(&TaskState::Fail(
                ActError::SubjectError("users is empty".to_string()).into(),
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
                            self.set_state(&TaskState::Fail(err.into()));
                            return acts;
                        }
                    }
                }

                if let Some(task) = ctx.task() {
                    for user in users.iter() {
                        acts.push(Act::new(&task.tid(), user));
                    }
                    self.set_candidates(&acts);
                }
            }
            Err(err) => self.set_state(&TaskState::Fail(err.into())),
        }

        acts
    }

    fn process_run(&self, ctx: &Context) {
        if let Some(script) = &self.run {
            let ret = ctx.run(script);
            if let Some(err) = ret.err() {
                self.set_state(&TaskState::Fail(err.into()));
            }
        }
    }

    fn process_action(&self, ctx: &Context) {
        if let Some(act) = &self.action {
            act(ctx.vm());
        }
    }

    fn check_event(&self, ctx: &Context) -> bool {
        match &self.accept {
            Some(m) => {
                if m.is_sequence() {
                    let seq = m.as_sequence().unwrap();
                    return seq.iter().all(|evt| {
                        let key = evt.as_str().unwrap();
                        ctx.user_data().action == key
                    });
                }

                true
            }
            None => true,
        }
    }
    pub(in crate::sch) fn check_pass(&self, ctx: &Context) -> bool {
        let acts = self.acts();
        if acts.len() > 0 {
            let matcher = self.matcher();
            match matcher.is_pass(self, &ctx) {
                Ok(ret) => {
                    if !ret {
                        return false;
                    }

                    return self.check_event(ctx);
                }
                Err(err) => {
                    self.set_state(&TaskState::Fail(err.into()));
                    true
                }
            }
        } else {
            return self.check_event(ctx);
        }
    }
}

#[async_trait]
impl ActTask for Step {
    fn prepare(&self, ctx: &Context) {
        self.prepare_sub(ctx);
    }

    fn run(&self, ctx: &Context) {
        if let Some(expr) = &self.r#if {
            match ctx.eval(expr) {
                Ok(cond) => {
                    if cond {
                        self.process_action(ctx);
                        self.process_run(ctx);
                    } else {
                        self.set_state(&TaskState::Skip);
                    }
                }
                Err(err) => self.set_state(&TaskState::Fail(err.into())),
            }
        } else {
            self.process_action(ctx);
            self.process_run(ctx);
        }
    }
}
