use acts::{Executor, Message, Result, RoleAdapter, Vars};
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct User<'a> {
    pub id: &'a str,
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

            let state = executor.complete(&message.proc_id, &message.id, &options)?;
            println!("action state: id={} cost={}ms", &message.id, state.cost());
        } else if message.is_key("each") && message.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".to_string(), json!("u1"));

            let state = executor.complete(&message.proc_id, &message.id, &options)?;
            println!(
                "action state: id={} key={} cost={}ms",
                &message.id,
                &message.key,
                state.cost()
            );
        } else if message.is_key("pm") && message.is_state("created") {
            let mut options = Vars::new();
            options.insert("uid".into(), "admin".into());
            options.insert("users".into(), json!({ "role(pm)": self.role("pm") }));

            let state = executor.complete(&message.proc_id, &message.id, &options)?;
            println!(
                "action state: id={} key={} cost={}ms",
                &message.id,
                &message.key,
                state.cost()
            );
        } else if message.is_key("gm") && message.is_state("created") {
            let mut options = Vars::new();

            options.insert("uid".into(), "admin".into());
            options.insert("users".into(), json!({ "role(gm)": self.role("gm") }));

            let state = executor.complete(&message.proc_id, &message.id, &options)?;
            println!(
                "action state: id={} key={} cost={}ms",
                &message.id,
                &message.key,
                state.cost()
            );
        } else if message.is_key("final") {
            println!("do final work");
        }

        Ok(())
    }
}

impl RoleAdapter for Client<'_> {
    fn role(&self, name: &str) -> Vec<String> {
        let mut ret = vec![];
        if let Some(users) = self.users.get(name) {
            let iter = users.iter().map(|u| u.id.to_string());
            ret.extend(iter);
        }
        ret
    }
}

// you can also implement the OrgAdapter to get the org users
// impl OrgAdapter for Client<'_> {
//     fn dept(&self, _value: &str) -> Vec<String> {
//         todo!()
//     }

//     fn unit(&self, _value: &str) -> Vec<String> {
//         todo!()
//     }

//     fn relate(&self, _value: &str) -> Vec<String> {
//         todo!()
//     }
// }
