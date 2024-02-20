use crate::{ActValue, Candidate, Operation};
use serde_json::json;

#[test]
fn model_candidate_value() {
    let cand = Candidate::Value("u1".to_string());

    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "value", "value": "u1" })
    )
}

#[test]
fn model_candidate_group() {
    let cand_user = Candidate::Value("u1".to_string());
    let cand = Candidate::Group {
        op: Operation::Intersect,
        items: vec![
            cand_user.clone(),
        ],
    };

    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ 
            "type": "group",
            "op": Operation::Intersect.to_string(), 
            "items": json!([
                Into::<ActValue>::into(cand_user),
        ]) })
    );
}

#[test]
fn model_candidate_nest() {
    let cand1 = Candidate::Group {
        op: Operation::Union,
        items: vec![
            Candidate::Value("u1".to_string()),
            Candidate::Value("u2".to_string()),
        ],
    };

    let cand2 = Candidate::Group {
        op: Operation::Except,
        items: vec![
            Candidate::Value("r1".to_string()),
            Candidate::Value("u3".to_string()),
        ],
    };

    let cand = Candidate::Group {
        op: Operation::Intersect,
        items: vec![cand1.clone(), cand2.clone()],
    };

    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ 
            "type": "group",
            "op": Operation::Intersect.to_string(), 
            "items": json!([
                Into::<ActValue>::into(cand1),
                Into::<ActValue>::into(cand2),
        ]) })
    );
}

#[test]
fn model_candidate_parse() {
    let text = json!({ "type": "value", "value": "u1" }).to_string();
    let cand = Candidate::parse(&text);
    assert!(cand.is_ok());

    if let Candidate::Value(value) = cand.unwrap() {
        assert_eq!(value, "u1");
    } else {
        assert!(false);
    }
}

#[test]
fn model_candidate_parse_str_as_value() {
    let text = "u1";
    let cand = Candidate::parse(text);
    assert!(cand.is_ok());

    if let Candidate::Value(value) = cand.unwrap() {
        assert_eq!(value, "u1");
    } else {
        assert!(false);
    }
}
