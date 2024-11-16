use acts::{Executor, Message, Result, Vars};
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct User<'a> {
    pub id: &'a str,
    #[allow(dead_code)]
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct Client<'a> {
    users: HashMap<String, Vec<User<'a>>>,
}

impl<'a> Client<'a> {
    pub fn new() -> Self {
        Client {
            users: HashMap::new(),
        }
    }

    pub fn init(&mut self) {
        self.add_user(
            User {
                id: "1",
                name: "John",
            },
            "pm",
        );
        self.add_user(
            User {
                id: "2",
                name: "Rose",
            },
            "pm",
        );
        self.add_user(
            User {
                id: "3",
                name: "Tom",
            },
            "gm",
        );
    }
    pub fn add_user(&mut self, user: User<'a>, role: &str) {
        self.users
            .entry(role.to_string())
            .and_modify(|v| v.push(user.clone()))
            .or_insert(vec![user.clone()]);
    }

    pub fn process(&self, executor: &Executor, message: &Message) -> Result<()> {
        if message.is_key("init") && message.is_state("created") {
            // init the workflow
            println!("do init work");
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            executor
                .act()
                .complete(&message.pid, &message.tid, &options)?;
            println!(
                "action state: id={} key={} inputs={}",
                &message.tid, &message.key, &message.inputs,
            );
        } else if message.is_key("pm_act") && message.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            executor
                .act()
                .complete(&message.pid, &message.tid, &options)?;
            println!(
                "action state: id={} key={} inputs={}",
                &message.tid, &message.key, &message.inputs,
            );
        } else if message.is_key("gm_act") && message.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u2"));

            executor
                .act()
                .complete(&message.pid, &message.tid, &options)?;
            println!(
                "action state: id={} key={} inputs={}",
                &message.tid, &message.key, &message.inputs,
            );
        } else if message.is_key("pm") && message.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".into(), "admin".into());
            options.insert(
                "pm".into(),
                json!(self.role(&message.inputs.get::<String>("role_id").unwrap())),
            );
            executor
                .act()
                .complete(&message.pid, &message.tid, &options)?;
            println!(
                "action state: id={} key={} inputs={}",
                &message.tid, &message.key, &message.inputs,
            );
        } else if message.is_key("gm") && message.is_state("created") {
            let mut options = Vars::new();

            options.insert("uid".into(), "admin".into());
            options.insert(
                "gm".into(),
                json!(self.role(&message.inputs.get::<String>("role_id").unwrap())),
            );

            executor
                .act()
                .complete(&message.pid, &message.tid, &options)?;
            println!("action state: id={} key={}", &message.tid, &message.key,);
        } else if message.is_type("msg") {
            println!(
                "msg: id={} key={} inputs={}",
                message.tid, message.key, message.inputs
            );
        }

        Ok(())
    }
}

impl Client<'_> {
    fn role(&self, name: &str) -> Vec<String> {
        let mut ret = vec![];
        if let Some(users) = self.users.get(name) {
            let iter = users.iter().map(|u| u.id.to_string());
            ret.extend(iter);
        }
        ret
    }
}
