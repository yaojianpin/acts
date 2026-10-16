use super::*;

#[test]
fn commit_digest_covers_graph_topology_and_model_identity() {
    let mut left = repository().load("p1").unwrap().unwrap();
    let mut right = left.clone();
    let task = id();
    right.tasks.get_mut(&task).unwrap().model_node = NodeId::parse("different-model").unwrap();
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
    left.version.graph = 9;
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
}

#[test]
fn commit_digest_is_unambiguous_for_separator_like_user_data() {
    let mut left = repository().load("p1").unwrap().unwrap();
    let mut right = left.clone();
    left.variables.insert((id(), "a:b".into()), "c".into());
    right.variables.insert((id(), "a".into()), "b:c".into());
    assert_ne!(
        deterministic_digest(&left).unwrap(),
        deterministic_digest(&right).unwrap()
    );
}

#[test]
fn sha256_matches_standard_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn commit_digest_is_fixed_size_lower_hex() {
    let digest = deterministic_digest(&repository().load("p1").unwrap().unwrap()).unwrap();
    assert_eq!(digest.len(), 64);
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
}

#[test]
fn canonical_process_and_commit_digest_match_golden_fingerprint() {
    let snapshot = repository().load("p1").unwrap().unwrap();
    let bytes = canonical_process_bytes(&snapshot).unwrap();
    const GOLDEN_DIGEST: &str = "a47d4bd489ffc8d817e121a5d1743a93b612d8bdd7da8a1ee2c5e1288967028e";
    assert_eq!(bytes.len(), 76);
    assert_eq!(sha256_hex(&bytes), GOLDEN_DIGEST);
    assert_eq!(deterministic_digest(&snapshot).unwrap(), GOLDEN_DIGEST);
}
