use super::Rule;

#[tokio::test]
async fn sch_group_rule_parse_any() {
    let rule = Rule::parse("any").unwrap();
    assert_eq!(rule, Rule::Any);
}

#[tokio::test]
async fn sch_group_rule_parse_all() {
    let rule = Rule::parse("all").unwrap();
    assert_eq!(rule, Rule::All);
}

#[tokio::test]
async fn sch_group_rule_parse_ord() {
    let rule = Rule::parse("ord").unwrap();
    assert_eq!(rule, Rule::Ord(None));
}

#[tokio::test]
async fn sch_group_rule_parse_ord_key() {
    let rule = Rule::parse("ord(a)").unwrap();
    assert_eq!(rule, Rule::Ord(Some("a".to_string())));
}

#[tokio::test]
async fn sch_group_rule_parse_some() {
    let rule = Rule::parse("some(a)").unwrap();
    assert_eq!(rule, Rule::Some("a".to_string()));
}

#[tokio::test]
async fn sch_group_rule_parse_error() {
    let rule = Rule::parse("not_exist");
    assert_eq!(rule.is_err(), true);
}
