mod block;
mod call;
mod cmd;
mod irq;
mod pack;

use super::TaskLifeCycle;
use crate::{
    scheduler::Context,
    utils::{self, consts},
    Act, ActError, ActFn, ActTask, Result, TaskState, Vars,
};
use async_trait::async_trait;
use std::{cell::RefCell, rc::Rc};
use tracing::debug;

#[async_trait]
impl ActTask for Act {
    fn init(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        for s in self.catches.iter() {
            task.add_hook_catch(TaskLifeCycle::ErrorCatch, s);
        }

        if !self.timeout.is_empty() {
            for s in &self.timeout {
                task.add_hook_timeout(TaskLifeCycle::Timeout, s);
            }
        }

        // run setup
        if !self.setup.is_empty() {
            for act in &self.setup {
                act.exec(ctx)?;
            }
        }

        let func: ActFn = self.into();
        match func {
            ActFn::Irq(irq) => irq.init(ctx),
            ActFn::Call(u) => u.init(ctx),
            ActFn::Block(b) => b.init(ctx),
            ActFn::Pack(p) => p.init(ctx),
            _ => Ok(()),
        }
    }

    fn run(&self, ctx: &Context) -> Result<()> {
        let func: ActFn = self.into();
        match func {
            ActFn::Irq(req) => req.run(ctx),
            ActFn::Call(u) => u.run(ctx),
            ActFn::Block(b) => b.run(ctx),
            ActFn::Pack(p) => p.run(ctx),
            _ => Ok(()),
        }
    }

    fn next(&self, ctx: &Context) -> Result<bool> {
        let func: ActFn = self.into();
        match func {
            ActFn::Irq(req) => req.next(ctx),
            ActFn::Call(u) => u.next(ctx),
            ActFn::Block(b) => b.next(ctx),
            ActFn::Pack(p) => p.next(ctx),
            _ => Ok(false),
        }
    }

    fn review(&self, ctx: &Context) -> Result<bool> {
        let func: ActFn = self.into();
        match func {
            ActFn::Irq(req) => req.review(ctx),
            ActFn::Call(u) => u.review(ctx),
            ActFn::Block(b) => b.review(ctx),
            ActFn::Pack(p) => p.review(ctx),
            _ => Ok(true),
        }
    }
}

impl Act {
    pub fn exec(&self, ctx: &Context) -> Result<()> {
        let task = ctx.task();
        debug!("act.exec task={}", task.id);
        let act_fn = self.into();
        match act_fn {
            ActFn::Set(vars) => {
                let inputs = utils::fill_inputs(&vars, ctx);
                task.update_data(&inputs);
            }
            ActFn::Expose(vars) => {
                let outputs = utils::fill_outputs(&vars, ctx);
                // expose the vars to outputs
                task.set_data_with(move |data| data.set(consts::ACT_OUTPUTS, &outputs));
            }
            ActFn::Irq(_) => {
                let mut req = self.clone();
                if let Some(v) = ctx.get_var::<u32>(consts::ACT_INDEX) {
                    req.inputs.set(consts::ACT_INDEX, v);
                }

                if let Some(v) = ctx.get_var::<String>(consts::ACT_VALUE) {
                    req.inputs.set(consts::ACT_VALUE, v);
                }
                if req.key.is_empty() {
                    return Err(ActError::Action(format!(
                        "not found 'key' in act({})",
                        self.id
                    )));
                }
                ctx.append_act(&req)?;
            }
            ActFn::Msg(_) => {
                let mut msg = self.clone();
                if let Some(v) = ctx.get_var::<u32>(consts::ACT_INDEX) {
                    msg.inputs.set(consts::ACT_INDEX, v);
                }

                if let Some(v) = ctx.get_var::<String>(consts::ACT_VALUE) {
                    msg.inputs.set(consts::ACT_VALUE, v);
                }
                if task.state().is_none() {
                    task.add_hook_stmts(TaskLifeCycle::Created, &msg);
                } else {
                    if msg.key.is_empty() {
                        return Err(ActError::Action(format!(
                            "not found 'key' in act({})",
                            self.id
                        )));
                    }
                    ctx.emit_message(&msg)?;
                }
            }
            ActFn::Cmd(cmd) => {
                if task.state().is_none() {
                    task.add_hook_stmts(TaskLifeCycle::Created, &cmd.into());
                } else if let Err(err) = cmd.run(ctx) {
                    task.set_state(TaskState::Error);
                    return Err(err);
                }
            }
            ActFn::Block(_) => {
                ctx.append_act(self)?;
            }
            ActFn::Pack(_) => {
                ctx.append_act(self)?;
            }
            ActFn::If(cond) => {
                let result = ctx.eval(&cond.on)?;
                if result {
                    for s in &cond.then {
                        s.exec(ctx)?;
                    }
                } else {
                    for s in &cond.r#else {
                        s.exec(ctx)?;
                    }
                }
            }
            ActFn::Each(each) => {
                let cans = each.parse(ctx, &each.r#in)?;
                for (index, value) in cans.iter().enumerate() {
                    ctx.set_var(consts::ACT_INDEX, index);
                    ctx.set_var(consts::ACT_VALUE, value);
                    for s in &each.then {
                        s.exec(ctx)?;
                    }
                }
            }
            ActFn::Chain(chain) => {
                let cans = chain.parse(ctx, &chain.r#in)?;
                let stmts = &chain.then;
                let mut items = cans.iter().enumerate();
                if let Some((index, value)) = items.next() {
                    let head = Rc::new(RefCell::new(Act::default()));

                    head.borrow_mut().id = utils::shortid();
                    head.borrow_mut().act = "block".to_string();
                    head.borrow_mut().then = stmts.clone();
                    head.borrow_mut().inputs = Vars::new()
                        .with(consts::ACT_INDEX, index)
                        .with(consts::ACT_VALUE, value);

                    let mut pre = head.clone();
                    for (index, value) in items {
                        let p = Rc::new(RefCell::new(Act::default()));
                        p.borrow_mut().id = utils::shortid();
                        p.borrow_mut().act = "block".to_string();
                        p.borrow_mut().then = stmts.clone();
                        p.borrow_mut().inputs = Vars::new()
                            .with(consts::ACT_INDEX, index)
                            .with(consts::ACT_VALUE, value);

                        pre.borrow_mut().next = Some(Box::new((*p).clone().into_inner()));
                        pre = p;
                    }

                    let act = head.take();
                    act.exec(ctx)?;
                }
            }
            ActFn::Call(_) => {
                ctx.append_act(self)?;
            }
            ActFn::OnCreated(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_stmts(TaskLifeCycle::Created, &s);
                }
            }
            ActFn::OnCompleted(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_stmts(TaskLifeCycle::Completed, &s);
                }
            }
            ActFn::OnBeforeUpdate(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_stmts(TaskLifeCycle::BeforeUpdate, &s);
                }
            }
            ActFn::OnUpdated(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_stmts(TaskLifeCycle::Updated, &s);
                }
            }
            ActFn::OnStep(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_stmts(TaskLifeCycle::Step, &s);
                }
            }
            ActFn::OnErrorCatch(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_catch(TaskLifeCycle::ErrorCatch, &s);
                }
            }
            ActFn::OnTimeout(stmts) => {
                let task = ctx.task();
                for s in stmts {
                    task.add_hook_timeout(TaskLifeCycle::Timeout, &s);
                }
            }
            ActFn::None => {
                // ignore
                return Err(ActError::Action(format!(
                    "cannot recognize the act({}) as a valid act function",
                    self.id
                )));
            }
        }
        Ok(())
    }
}
