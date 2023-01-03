use std::collections::HashMap;
use yao::{OrgAdapter, RoleAdapter, RuleAdapter};

#[derive(Debug, Clone)]
pub struct User<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

#[derive(Debug, Clone)]
pub struct Adapter<'a> {
    users: HashMap<String, Vec<User<'a>>>,
}

impl<'a> Adapter<'a> {
    pub fn new() -> Self {
        Adapter {
            users: HashMap::new(),
        }
    }
    pub fn add_user(&mut self, user: User<'a>, role: &str) {
        self.users
            .entry(role.to_string())
            .and_modify(|v| v.push(user.clone()))
            .or_insert(vec![user.clone()]);
    }
}

impl<'a> RuleAdapter for Adapter<'a> {
    fn ord(&self, _name: &str, _acts: &Vec<String>) -> yao::ActResult<Vec<String>> {
        todo!()
    }

    fn some(&self, _name: &str, _step: &yao::Step, _ctx: &yao::Context) -> yao::ActResult<bool> {
        todo!()
    }
}

impl<'a> OrgAdapter for Adapter<'a> {
    fn dept(&self, _name: &str) -> Vec<String> {
        todo!()
    }

    fn unit(&self, _name: &str) -> Vec<String> {
        todo!()
    }

    fn relate(&self, _t: &str, _id: &str, _r: &str) -> Vec<String> {
        todo!()
    }
}

impl<'a> RoleAdapter for Adapter<'a> {
    fn role(&self, name: &str) -> Vec<String> {
        let default = Vec::new();
        let users = self.users.get(name).unwrap_or(&default);

        users.iter().map(|u| u.id.to_string()).collect()
    }
}
