use super::*;

// ── Cidr ──

/// The parser accepts the deployment's spellings and rejects everything a
/// trust decision must not be made on.
#[test]
fn cidr_parse() {
    assert_eq!(
        Cidr::parse("10.0.0.0/8"),
        Some(Cidr {
            addr: "10.0.0.0".parse().unwrap(),
            prefix: 8
        })
    );
    // a bare address is a host range of its own family
    assert_eq!(
        Cidr::parse("127.0.0.1").unwrap().prefix,
        32,
        "a bare v4 address is a /32"
    );
    assert_eq!(
        Cidr::parse("::1").unwrap().prefix,
        128,
        "a bare v6 address is a /128"
    );
    assert!(Cidr::parse("2001:db8::/32").is_some());
    for bad in [
        "not an address",
        "10.0.0.0/33",
        "::/129",
        "10.0.0.0/-1",
        "10.0.0.0/x",
        "",
    ] {
        assert!(Cidr::parse(bad).is_none(), "'{bad}' must not parse");
    }
}

/// Containment matches on the prefix bits, per family: a v4 range never
/// trusts a v6 peer (or the other way around), and the /0 edge is everything.
#[test]
fn cidr_contains() {
    let private = Cidr::parse("10.0.0.0/8").unwrap();
    assert!(private.contains(&"10.1.2.3".parse().unwrap()));
    assert!(!private.contains(&"11.1.2.3".parse().unwrap()));
    assert!(!private.contains(&"fd00::1".parse().unwrap()));

    let host = Cidr::parse("127.0.0.1").unwrap();
    assert!(host.contains(&"127.0.0.1".parse().unwrap()));
    assert!(!host.contains(&"127.0.0.2".parse().unwrap()));

    let v6 = Cidr::parse("fd00::/8").unwrap();
    assert!(v6.contains(&"fd12::1".parse().unwrap()));
    assert!(!v6.contains(&"fe80::1".parse().unwrap()));
    assert!(!v6.contains(&"10.0.0.1".parse().unwrap()));

    let everything_v6 = Cidr::parse("::/0").unwrap();
    assert!(everything_v6.contains(&"2001:db8::1".parse().unwrap()));
    assert!(
        !everything_v6.contains(&"127.0.0.1".parse().unwrap()),
        "a v6 /0 still only matches v6 peers"
    );
}

// ── client_ip ──

/// A trusted proxy's `X-Forwarded-For` is honored: the client is the
/// rightmost entry that is not itself one of the trusted ranges (proxies
/// append, so the innermost proxy sits left of the client).
#[test]
fn client_ip_from_a_trusted_proxy() {
    let trusted = vec![Cidr::parse("10.0.0.0/8").unwrap()];
    let peer: IpAddr = "10.1.1.1".parse().unwrap();
    assert_eq!(
        client_ip(peer, Some("203.0.113.9, 10.0.0.2"), &trusted),
        "203.0.113.9".parse::<IpAddr>().unwrap(),
        "the rightmost non-trusted entry is the client"
    );
    // a chain of trusted proxies only: the peer (innermost proxy) is the
    // best client there is
    assert_eq!(client_ip(peer, Some("10.0.0.2, 10.0.0.3"), &trusted), peer);
}

/// An untrusted peer cannot pick its own rate-limit bucket with a header:
/// its forged `X-Forwarded-For` is ignored and the peer address is used.
#[test]
fn client_ip_ignores_an_untrusted_peer() {
    let trusted = vec![Cidr::parse("10.0.0.0/8").unwrap()];
    let peer: IpAddr = "192.168.5.5".parse().unwrap();
    assert_eq!(client_ip(peer, Some("9.9.9.9"), &trusted), peer);
    // no header at all, trusted peer or not
    assert_eq!(client_ip(peer, None, &trusted), peer);
    // an empty or unparseable chain falls back to the peer
    assert_eq!(client_ip(peer, Some("not-an-ip"), &trusted), peer);
}

// ── route_action ──

/// Every route maps onto the action name its handler dispatches, with or
/// without the nested `/api` prefix, and paths off the action surface map to
/// nothing.
#[test]
fn route_action_maps_the_whole_surface() {
    let cases = [
        ("/api/model/deploy", Some("model:deploy".to_string())),
        ("/model/deploy", Some("model:deploy".to_string())),
        ("/api/model/list", Some("model:ls".to_string())),
        ("/proc/start", Some("proc:start".to_string())),
        ("/api/act/complete", Some("act:complete".to_string())),
        ("/api/hooks/pid:trg", Some("evt:start".to_string())),
        ("/api/msg/sse", Some("msg:sub".to_string())),
        ("/api/snap/upsert", Some("snap:upsert".to_string())),
        ("/health", None),
        ("/", None),
    ];
    for (path, want) in cases {
        assert_eq!(route_action(path), want, "route_action('{path}')");
    }
}

/// The per-route body caps come from the one profile table the handlers
/// dispatch under: the deploy route's cap is the deploy profile's, the small
/// routes' the small class — and they differ, so a cap can never silently
/// widen to whatever a neighbouring route got.
#[tokio::test(flavor = "multi_thread")]
async fn body_caps_follow_the_action_profiles() {
    let engine = acts::Engine::builder().disable_acl().start().await.unwrap();
    let ingress = engine.ingress();
    let deploy = ingress
        .profile(&route_action("/model/deploy").unwrap())
        .max_raw_bytes;
    let small = ingress
        .profile(&route_action("/model/get").unwrap())
        .max_raw_bytes;
    let event = ingress
        .profile(&route_action("/hooks/x").unwrap())
        .max_raw_bytes;
    assert_eq!(
        deploy,
        8 * 1024 * 1024,
        "the deploy cap is the deploy profile"
    );
    assert_eq!(small, 64 * 1024, "the small cap is the small profile");
    assert_eq!(event, 1024 * 1024, "the hook cap is the event profile");
    assert_ne!(deploy, small);
    assert_ne!(event, small);
    engine.close().await;
}
