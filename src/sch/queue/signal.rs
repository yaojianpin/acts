use crate::sch::{Message, Proc};

#[derive(Clone)]
pub enum Signal {
    Terminal,
    Proc(Proc),
    Task(String, Proc),
    Message(Message),
}
