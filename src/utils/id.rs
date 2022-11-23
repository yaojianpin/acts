use nanoid::nanoid;

const ALPHABETS: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B',
    'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z',
];

const ID_SEP: &str = ":";

pub fn longid() -> String {
    nanoid!(21, &ALPHABETS)
}

pub fn shortid() -> String {
    nanoid!(8, &ALPHABETS)
}

#[derive(Debug)]
pub struct Id<'a> {
    pid: &'a str,
    tid: &'a str,
}

impl<'a> Id<'a> {
    pub fn new(pid: &'a str, tid: &'a str) -> Self {
        Self { pid: pid, tid: tid }
    }

    pub fn from(id: &'a str) -> Self {
        let parts: Vec<&str> = id.split(ID_SEP).collect();
        let pid = parts.get(0).unwrap();
        let mut tid = "";
        if parts.len() > 1 {
            tid = parts.get(1).unwrap();
        }

        Self { pid, tid }
    }

    pub fn id(&self) -> String {
        let mut id = String::from(self.pid);
        if !self.tid.is_empty() {
            id.push_str(ID_SEP);
            id.push_str(&self.tid);
        }

        id
    }

    pub fn pid(&self) -> &'a str {
        self.pid
    }

    pub fn tid(&self) -> &'a str {
        self.tid
    }
}
