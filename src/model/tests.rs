use serde_json::json;

use crate::{ActValue, Candidate, Operation};

#[test]
fn model_candidate_user() {
    let cand = Candidate::User("u1".to_string());

    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "user", "id": "u1" })
    )
}

#[test]
fn model_candidate_role() {
    let cand = Candidate::Role("r1".to_string());
    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "role", "id": "r1" })
    )
}

#[test]
fn model_candidate_org() {
    let cand = Candidate::Unit("u1".to_string());
    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "unit", "id": "u1" })
    );

    let cand = Candidate::Dept("d1".to_string());
    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "dept", "id": "d1" })
    );

    let cand = Candidate::Relation {
        id: "u1".to_string(),
        rel: "d.owner".to_string(),
    };
    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ "type": "rel", "id": "u1", "rel": "d.owner" })
    );
}

#[test]
fn model_candidate_group() {
    let cand_unit = Candidate::Unit("u1".to_string());
    let cand_dept = Candidate::Dept("d1".to_string());
    let cand_rel = Candidate::Relation {
        id: "u1".to_string(),
        rel: "d.owner".to_string(),
    };
    let cand_role = Candidate::Role("r1".to_string());
    let cand_user = Candidate::User("u1".to_string());
    let cand = Candidate::Group {
        op: Operation::Intersect,
        items: vec![
            cand_unit.clone(),
            cand_dept.clone(),
            cand_rel.clone(),
            cand_role.clone(),
            cand_user.clone(),
        ],
    };

    assert_eq!(
        Into::<ActValue>::into(cand),
        json!({ 
            "type": "group",
            "op": Operation::Intersect.to_string(), 
            "items": json!([
                Into::<ActValue>::into(cand_unit),
                Into::<ActValue>::into(cand_dept),
                Into::<ActValue>::into(cand_rel),
                Into::<ActValue>::into(cand_role),
                Into::<ActValue>::into(cand_user),
        ]) })
    );
}



#[test]
fn model_candidate_nest() {
    let cand1 = Candidate::Group {
        op: Operation::Union,
        items: vec![
             Candidate::User("u1".to_string()),
             Candidate::User("u2".to_string())
        ],
    };

    let cand2 = Candidate::Group {
        op: Operation::Difference,
        items: vec![
             Candidate::Role("r1".to_string()),
             Candidate::User("u3".to_string())
        ],
    };

    let cand = Candidate::Group {
        op: Operation::Intersect,
        items: vec![
            cand1.clone(),
            cand2.clone(),
        ],
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

    let text = json!({ "type": "user", "id": "u1" }).to_string();
    let cand = Candidate::parse(&text);
    assert!(cand.is_ok());

    if let Candidate::User(id) = cand.unwrap() {
        assert_eq!(id, "u1");
    } else {
        assert!(false);
    }
    
}

#[test]
fn model_candidate_parse_str_as_user() {

    let text = "u1";
    let cand = Candidate::parse(text);
    assert!(cand.is_ok());
    
    if let Candidate::User(id) = cand.unwrap() {
        assert_eq!(id, "u1");
    } else {
        assert!(false);
    }
}