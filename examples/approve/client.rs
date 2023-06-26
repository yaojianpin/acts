use acts::{
    event::EventAction, ActResult, ActValue, Candidate, Executor, Message, OrgAdapter, RoleAdapter,
    Vars,
};
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
            "admin",
        );

        self.add_user(
            User {
                id: "2",
                name: "Tom",
            },
            "admin",
        );
    }
    pub fn add_user(&mut self, user: User<'a>, role: &str) {
        self.users
            .entry(role.to_string())
            .and_modify(|v| v.push(user.clone()))
            .or_insert(vec![user.clone()]);
    }

    pub fn process(&self, executor: &Executor, message: &Message) -> ActResult<()> {
        println!(
            "message kind={} event={} key={:?}, vars={:?}",
            message.kind, message.event, message.key, message.vars
        );
        if let Some(_msg) = message.as_task_message() {
            // do something for task message
        } else if let Some(_msg) = message.as_notice_message() {
            // do something for notice message
        } else if let Some(msg) = message.as_user_message() {
            if msg.event == &EventAction::Create {
                let mut vars = Vars::new();
                vars.insert("uid".to_string(), msg.uid.into());
                let state = executor.complete(&msg.pid, &msg.aid, &vars)?;
                println!("action state: aid={} cost={}ms", msg.aid, state.cost());
            }
        } else if let Some(msg) = message.as_action_message() {
            if msg.event == &EventAction::Create {
                let mut vars = Vars::new();
                vars.insert("uid".into(), "admin".into());
                vars.insert("custom".into(), 100.into());

                let state = executor.complete(msg.pid, &msg.aid, &vars)?;
                println!("action state: aid={} cost={}ms", msg.aid, state.cost());
            }
        } else if let Some(msg) = message.as_canidate_message() {
            if msg.event == &EventAction::Create {
                let mut vars = Vars::new();
                let new_cand = self.parse_cand(&msg.cands);
                vars.insert("uid".into(), "admin".into());
                vars.insert("sub_cands".into(), new_cand);
                let state = executor.complete(&msg.pid, &msg.aid, &vars)?;
                println!("action state: aid={} cost={}ms", msg.aid, state.cost());
            }
        } else if let Some(msg) = message.as_some_message() {
            if msg.event == &EventAction::Create {
                println!("run client act rule={}, vars={:#?}", msg.some, msg.vars);

                let mut vars = Vars::new();
                vars.insert("uid".into(), "admin".into());
                vars.insert("some".into(), true.into());

                let state = executor.complete(&msg.pid, &msg.aid, &vars)?;
                println!("action state: aid={} cost={}ms", msg.aid, state.cost());
            }
        }

        Ok(())
    }

    fn parse_cand(&self, data: &ActValue) -> ActValue {
        let cand: Candidate = data.into();
        let ret = cand.calc(self);
        if ret.is_err() {
            eprintln!("{}", ret.err().unwrap());
            std::process::exit(1);
        }

        ret.unwrap().into()
    }
}

impl<'a> RoleAdapter for Client<'a> {
    fn role(&self, name: &str) -> Vec<String> {
        let mut ret = vec![];

        if let Some(users) = self.users.get(name) {
            let iter = users.iter().map(|u| u.id.to_string());
            ret.extend(iter);
        }
        ret
    }
}

impl OrgAdapter for Client<'_> {
    fn dept(&self, _name: &str) -> Vec<String> {
        todo!()
    }

    fn unit(&self, _name: &str) -> Vec<String> {
        todo!()
    }

    fn relate(&self, _id_type: &str, _id: &str, _relation: &str) -> Vec<String> {
        todo!()
    }
}
