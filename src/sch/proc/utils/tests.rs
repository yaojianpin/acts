use crate::{sch::proc::utils::matcher::Matcher, Subject};

#[tokio::test]
async fn sch_sub_parser() {
    let text = r#"
    matcher: any
    cands: |
        ["a"]"#;

    let sub = serde_yaml::from_str::<Subject>(text).unwrap();
    assert_eq!(sub.matcher, "any");
    assert_eq!(sub.cands, r#"["a"]"#);
}

#[tokio::test]
async fn sch_sub_matcher() {
    let matcher = Matcher::parse("any").unwrap();
    assert_eq!(matcher, Matcher::Any);

    let matcher = Matcher::parse("all").unwrap();
    assert_eq!(matcher, Matcher::All);

    let matcher = Matcher::parse("one").unwrap();
    assert_eq!(matcher, Matcher::One);

    let matcher = Matcher::parse("ord").unwrap();
    assert_eq!(matcher, Matcher::Ord(None));

    let matcher = Matcher::parse("ord(a)").unwrap();
    assert_eq!(matcher, Matcher::Ord(Some("a".to_string())));

    let matcher = Matcher::parse("some(a)").unwrap();
    assert_eq!(matcher, Matcher::Some("a".to_string()));

    let matcher = Matcher::parse("not_exist").unwrap();
    assert_eq!(matcher, Matcher::Error);
}
