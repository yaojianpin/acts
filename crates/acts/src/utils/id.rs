use nanoid::nanoid;

use crate::utils::consts::KEY_SEP;

const ALPHABETS: [char; 62] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', 'A', 'B',
    'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U',
    'V', 'W', 'X', 'Y', 'Z',
];

#[allow(unused)]
pub fn longid() -> String {
    nanoid!(21, &ALPHABETS)
}

pub fn shortid() -> String {
    nanoid!(8, &ALPHABETS)
}

/// A deterministic id for a derived object (a message, a delivery): the same
/// domain tag and parts always produce the same value, and a different tag or
/// any different part produces a different domain-separated value. Used where
/// a retried dispatch must converge on the object the previous attempt
/// created instead of minting a second one.
pub(crate) fn stable_id(tag: &str, parts: &[&str]) -> String {
    let mut bytes = Vec::new();
    for part in parts {
        bytes.extend_from_slice(part.as_bytes());
        // A separator outside every id grammar makes ("a","bc") and
        // ("ab","c") different inputs even before hashing.
        bytes.push(0);
    }
    format!("{tag}{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes))
}

#[derive(Debug)]
pub struct Id<'a> {
    pid: &'a str,
    tid: &'a str,
}

impl<'a> Id<'a> {
    pub fn new(pid: &'a str, tid: &'a str) -> Self {
        Self { pid, tid }
    }

    #[allow(unused)]
    pub fn from(id: &'a str) -> Self {
        let parts: Vec<&str> = id.split(KEY_SEP).collect();
        let pid = parts.first().unwrap();
        let mut tid = "";
        if parts.len() > 1 {
            tid = parts.get(1).unwrap();
        }

        Self { pid, tid }
    }

    pub fn id(&self) -> String {
        let mut id = String::from(self.pid);
        if !self.tid.is_empty() {
            id.push_str(self.tid);
        }

        id
    }

    #[allow(unused)]
    pub fn pid(&self) -> &str {
        self.pid
    }

    #[allow(unused)]
    pub fn tid(&self) -> &str {
        self.tid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_new_and_accessors() {
        let id = Id::new("pid", "tid");
        assert_eq!(id.pid(), "pid");
        assert_eq!(id.tid(), "tid");
        assert_eq!(id.id(), "pidtid");
    }

    #[test]
    fn id_from_splits_the_key_separator() {
        let raw = format!("pid{}tid", KEY_SEP);
        let id = Id::from(&raw);
        assert_eq!(id.pid(), "pid");
        assert_eq!(id.tid(), "tid");
        assert_eq!(id.id(), "pidtid");

        // no separator: the whole value is the process id and tid is empty
        let id = Id::from("onlypid");
        assert_eq!(id.pid(), "onlypid");
        assert_eq!(id.tid(), "");
        assert_eq!(id.id(), "onlypid");
    }

    #[test]
    fn generated_ids_have_the_expected_length() {
        assert_eq!(shortid().len(), 8);
        assert_eq!(longid().len(), 21);
        assert_ne!(shortid(), shortid());
    }

    #[test]
    fn stable_ids_are_deterministic_and_domain_separated() {
        let first = stable_id("msg_", &["p1", "k1", "topic"]);
        assert_eq!(first, stable_id("msg_", &["p1", "k1", "topic"]));
        assert_ne!(first, stable_id("del_", &["p1", "k1", "topic"]));
        // Tuple boundaries are part of the input: ("a","bc") is not ("ab","c").
        assert_ne!(stable_id("x_", &["a", "bc"]), stable_id("x_", &["ab", "c"]));
        assert!(first.starts_with("msg_"));
        assert_eq!(first.len(), "msg_".len() + 32);
    }
}
