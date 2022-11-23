use crate::TaskState;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, PartialEq, Clone)]
pub enum Tag {
    Workflow,
    Job,
    Branch,
    Step,
    Act,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Task {
    pub id: String,
    pub tag: Tag,
    pub pid: String,
    pub tid: String,
    pub state: TaskState,
    pub start_time: i64,
    pub end_time: i64,
    pub user: String,
}

impl<'a> From<Tag> for &'a str {
    fn from(tag: Tag) -> Self {
        let s = match tag {
            Tag::Workflow => "w",
            Tag::Job => "j",
            Tag::Branch => "b",
            Tag::Step => "s",
            Tag::Act => "a",
        };

        s
    }
}

impl From<&str> for Tag {
    fn from(str: &str) -> Self {
        let s = match str {
            "w" => Tag::Workflow,
            "j" => Tag::Job,
            "b" => Tag::Branch,
            "s" => Tag::Step,
            "a" => Tag::Act,
            _ => panic!("not found tag: {}", str),
        };

        s
    }
}

impl Task {
    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }
    pub fn set_start_time(&mut self, time: i64) {
        self.start_time = time;
    }
    pub fn set_end_time(&mut self, time: i64) {
        self.end_time = time;
    }
    pub fn set_user(&mut self, user: &str) {
        self.user = user.to_string();
    }
}
