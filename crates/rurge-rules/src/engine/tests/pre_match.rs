//! The entry that decides a session before its connection exists
//! (`RuleEngine::pre_match`), and what the evaluation of a session keeps of it.

use super::*;
use rurge_config::{ListenerKind, Transport};

/// A loaded database that knows the addresses of 8.0.0.0/8 (the US, AS 15169)
/// and says `Unknown` of all others.
struct FakeGeo;

impl GeoLookup for FakeGeo {
    fn country(&self, ip: IpAddr) -> GeoResult<[u8; 2]> {
        match ip {
            IpAddr::V4(v) if v.octets()[0] == 8 => GeoResult::Known(*b"US"),
            _ => GeoResult::Unknown,
        }
    }
    fn asn(&self, ip: IpAddr) -> GeoResult<u32> {
        match ip {
            IpAddr::V4(v) if v.octets()[0] == 8 => GeoResult::Known(15169),
            _ => GeoResult::Unknown,
        }
    }
}

/// The engine of `text`, on `FakeGeo`.
fn engine_with_geo(text: &str) -> RuleEngine {
    let cfg = load(text);
    let sets = InlineSets::from_config(&cfg);
    RuleEngine::build(&cfg, &sets, Arc::new(FakeGeo)).unwrap()
}

/// A profile whose only rule but the FINAL is `rule`, rejecting and flagged
/// `pre-matching`.
fn flagged(rule: &str) -> String {
    format!("[Proxy]\nP = direct\n[Rule]\n{rule},REJECT,pre-matching\nFINAL,DIRECT\n")
}

/// The same for a rule that refers to the set `S`, which has the lines `lines`.
fn flagged_with_set(rule: &str, lines: &str) -> String {
    format!("{}[Ruleset S]\n{lines}\n", flagged(rule))
}

/// The same for the rule that takes the lines `lines` from the set `S`.
fn flagged_set(lines: &str) -> String {
    flagged_with_set("RULE-SET,S", lines)
}

/// The DNS query of the app for `name`: no connection yet, so no port and no
/// address, and the query is a UDP packet of its own.
fn dns_query(name: &str) -> SessionInfo {
    SessionInfo {
        src: "198.18.0.1:53000".parse().unwrap(),
        in_port: 0,
        listener: ListenerKind::Tun,
        transport: Transport::Udp,
        ..SessionInfo::udp(HostName::Domain(name.into()), 0)
    }
}

/// The first packet of a TCP connection of the app to `host:port`, a name in
/// its lowercase form or an address.
fn syn(host: &str, port: u16) -> SessionInfo {
    SessionInfo {
        src: "198.18.0.1:53001".parse().unwrap(),
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(HostName::parse(host), port)
    }
}

fn at_dns(e: &RuleEngine, name: &str) -> Option<PreMatch> {
    e.pre_match(&dns_query(name), PreMatchLayer::Dns)
}

fn at_syn(e: &RuleEngine, host: &str, port: u16) -> Option<PreMatch> {
    e.pre_match(&syn(host, port), PreMatchLayer::Syn)
}

/// A rule that wants the ports, the addresses, the transport or the HTTP
/// request of the connection is not a hit at the DNS layer, whatever the
/// query happens to carry: port 0, its own UDP, the source port of the app.
/// The same rule is a hit at the SYN layer when the connection fits it.
#[test]
fn the_dns_layer_needs_the_connection_for_ports_addresses_and_http() {
    // the rule and a connection that fits it
    let rules = [
        ("DEST-PORT,0-65535", syn("a.test", 443)),
        ("SRC-PORT,53000-53999", syn("a.test", 443)),
        ("IP-CIDR,192.0.2.0/24", syn("192.0.2.7", 443)),
        ("IP-CIDR6,2001:db8::/32", syn("[2001:db8::1]", 443)),
        ("GEOIP,US", syn("8.8.8.8", 443)),
        ("GEOIP,UNKNOWN", syn("1.1.1.1", 443)),
        ("IP-ASN,15169", syn("8.8.8.8", 443)),
        ("IP-ASN,UNKNOWN", syn("1.1.1.1", 443)),
    ];
    for (rule, connection) in rules {
        let e = engine_with_geo(&flagged(rule));
        assert_eq!(at_dns(&e, "a.test"), None, "{rule} at the DNS layer");
        let hit = e.pre_match(&connection, PreMatchLayer::Syn);
        assert_eq!(hit.map(|m| m.rule), Some(0), "{rule} at the SYN layer");
        // a NOT over it is not a hit before the connection either, and the
        // rule saying `no-resolve` does not make the address known
        for not in [
            format!("NOT,(({rule}))"),
            format!("NOT,(({rule},no-resolve))"),
        ] {
            let e = engine_with_geo(&flagged(&not));
            assert_eq!(at_dns(&e, "a.test"), None, "{not} at the DNS layer");
        }
    }
    // lines a set holds (none of these may be written as a rule of its own
    // with `pre-matching`) and whether the SYN of a TCP connection to
    // a.test:443 hits them: the first packet says nothing of HTTP
    let lines = [
        ("IN-PORT,0", true),
        ("PROTOCOL,UDP", false),
        ("PROTOCOL,TCP", true),
        ("USER-AGENT,*", false),
        ("URL-REGEX,^http", false),
    ];
    for (line, hits_at_syn) in lines {
        let e = engine_with_geo(&flagged_set(line));
        assert_eq!(at_dns(&e, "a.test"), None, "{line} at the DNS layer");
        assert_eq!(
            at_syn(&e, "a.test", 443).is_some(),
            hits_at_syn,
            "{line} at the SYN layer"
        );
        // a NOT over the line is no hit at the DNS layer, and at the SYN layer
        // it is one exactly when the line is not
        let e = engine_with_geo(&flagged_with_set("NOT,((RULE-SET,S))", line));
        assert_eq!(at_dns(&e, "a.test"), None, "NOT over {line} at DNS");
        assert_eq!(
            at_syn(&e, "a.test", 443).is_some(),
            !hits_at_syn,
            "NOT over {line} at SYN"
        );
    }
}

/// `AND,((DOMAIN,a.test),(DEST-PORT,443))` says nothing of the DNS query for
/// a.test: the port is not known yet. The connection that follows is
/// refused when it is to port 443.
#[test]
fn a_port_condition_keeps_a_rule_from_the_dns_layer() {
    // the second range holds for the port 0 of a query
    for (condition, hit_port, miss_port) in [("443", 443, 80), ("0-1023", 80, 8080)] {
        let rule = format!("AND,((DOMAIN,a.test),(DEST-PORT,{condition}))");
        let e = engine_with_geo(&flagged(&rule));
        assert_eq!(at_dns(&e, "a.test"), None, "{rule}");
        assert!(
            at_syn(&e, "a.test", hit_port).is_some(),
            "{rule}:{hit_port}"
        );
        assert_eq!(at_syn(&e, "a.test", miss_port), None, "{rule}:{miss_port}");
        // and the port alone does not make another name a hit
        assert_eq!(at_syn(&e, "b.test", hit_port), None, "{rule}:b.test");
    }
}

/// Not knowing is not a reason to refuse: a `NOT` over a condition of the
/// connection (a port, an address, even one that says `no-resolve`) is no
/// hit at the DNS layer, whatever the name.
#[test]
fn not_never_turns_not_knowing_into_a_hit() {
    for rule in [
        "NOT,((DEST-PORT,443))",
        "NOT,((IP-CIDR,10.0.0.0/8))",
        "NOT,((IP-CIDR,10.0.0.0/8,no-resolve))",
        "NOT,((OR,((DOMAIN,a.test),(DEST-PORT,443))))",
    ] {
        let e = engine_with_geo(&flagged(rule));
        for name in ["a.test", "b.test", "nas", "10.example"] {
            assert_eq!(at_dns(&e, name), None, "{rule}: {name}");
        }
    }
    // the port decides it for a.test; what it does for another name is the
    // next test's
    let e = engine_with_geo(&flagged("NOT,((AND,((DOMAIN,a.test),(DEST-PORT,443))))"));
    assert_eq!(at_dns(&e, "a.test"), None);
}

/// A name that makes an `AND` false makes it false on every port, and the
/// `NOT` over it is a hit; the port decides only for the name that holds the
/// other half. The loop over the halves goes on past one it cannot decide.
#[test]
fn not_over_an_and_the_name_makes_false_is_a_hit() {
    for rule in [
        "NOT,((AND,((DOMAIN,a.test),(DEST-PORT,443))))",
        "NOT,((AND,((DEST-PORT,443),(DOMAIN,a.test))))",
    ] {
        let e = engine_with_geo(&flagged(rule));
        assert!(at_dns(&e, "b.test").is_some(), "{rule}: b.test at DNS");
        assert_eq!(at_dns(&e, "a.test"), None, "{rule}: a.test at DNS");
        for port in [80, 443] {
            assert!(
                at_syn(&e, "b.test", port).is_some(),
                "{rule}: b.test:{port}"
            );
        }
        assert!(at_syn(&e, "a.test", 80).is_some(), "{rule}: a.test:80");
        assert_eq!(at_syn(&e, "a.test", 443), None, "{rule}: a.test:443");
    }
}

/// A `NOT` over a set is a hit when the set can say the name is not in it:
/// all its lines are about names. A set with a line that needs the connection
/// (an address, a port) does not say so, unless another line matches.
#[test]
fn not_over_a_set_is_a_hit_only_when_the_set_decides() {
    const SETS: &str = "\
[Ruleset Names]
DOMAIN-SUFFIX,b.test
[Ruleset Addresses]
DOMAIN-SUFFIX,b.test
IP-CIDR,10.0.0.0/8
[Ruleset Numbers]
DOMAIN-SUFFIX,b.test
IP-ASN,15169
[Ruleset Ports]
DOMAIN-SUFFIX,b.test
DEST-PORT,443
";
    let profile = |set: &str| {
        format!(
            "[Proxy]\nP = direct\n[Rule]\nNOT,((RULE-SET,{set})),REJECT,pre-matching\n\
FINAL,DIRECT\n{SETS}"
        )
    };
    // the set holds names only: c.test is not in it, b.test is
    let e = engine_with_geo(&profile("Names"));
    assert!(at_dns(&e, "c.test").is_some());
    assert_eq!(at_dns(&e, "b.test"), None);
    // the set has a line about the connection: c.test may be in it, and
    // b.test is
    for set in ["Addresses", "Numbers", "Ports"] {
        let e = engine_with_geo(&profile(set));
        assert_eq!(at_dns(&e, "c.test"), None, "{set}: c.test");
        assert_eq!(at_dns(&e, "b.test"), None, "{set}: b.test");
    }
    // the connection decides what the name did not
    let e = engine_with_geo(&profile("Ports"));
    assert_eq!(at_syn(&e, "c.test", 443), None);
    assert!(at_syn(&e, "c.test", 80).is_some());
}

/// An `OR` that the name decides is a hit before the port is known: the other
/// half cannot make it false. An `OR` the name does not decide waits for the
/// connection.
#[test]
fn an_or_decided_by_its_name_is_a_hit_at_the_dns_layer() {
    let cases = [
        ("OR,((DOMAIN,a.test),(DEST-PORT,443))", "a.test", true),
        ("OR,((DOMAIN,a.test),(DEST-PORT,443))", "b.test", false),
        // the other order: the loop goes on past the half it cannot decide
        ("OR,((DEST-PORT,443),(DOMAIN,a.test))", "a.test", true),
        ("OR,((DEST-PORT,443),(DOMAIN,a.test))", "b.test", false),
        // a port that holds for the 0 of a query
        ("OR,((DOMAIN,a.test),(DEST-PORT,0-1023))", "a.test", true),
        ("OR,((DOMAIN,a.test),(DEST-PORT,0-1023))", "b.test", false),
        (
            "AND,((DOMAIN,a.test),(NOT,((DOMAIN,b.test))))",
            "a.test",
            true,
        ),
    ];
    for (rule, name, hit) in cases {
        let e = engine_with_geo(&flagged(rule));
        assert_eq!(at_dns(&e, name).is_some(), hit, "{rule}: {name}");
    }
    // the connection decides the second kind
    let e = engine_with_geo(&flagged("OR,((DOMAIN,a.test),(DEST-PORT,443))"));
    assert!(at_syn(&e, "b.test", 443).is_some());
    assert_eq!(at_syn(&e, "b.test", 80), None);
}

/// The address the query comes from and the network the machine is on are
/// known to the DNS layer, and rules about them decide there.
#[test]
fn src_ip_and_subnet_decide_at_the_dns_layer() {
    let e = engine_with_geo(&flagged("SRC-IP,198.18.0.1/32"));
    assert!(at_dns(&e, "a.test").is_some());
    let e = engine_with_geo(&flagged("SRC-IP,198.18.0.2/32"));
    assert_eq!(at_dns(&e, "a.test"), None);
    // together with a port they wait for the connection
    let e = engine_with_geo(&flagged("AND,((SRC-IP,198.18.0.1/32),(DEST-PORT,0-1023))"));
    assert_eq!(at_dns(&e, "a.test"), None);
    assert!(at_syn(&e, "a.test", 443).is_some());
    assert_eq!(at_syn(&e, "a.test", 8080), None);

    let cell = Arc::new(NetworkCell::new(NetworkFacts::default()));
    let e = engine_with_geo(&flagged("SUBNET,TYPE:WIFI")).with_network(cell.clone());
    assert_eq!(at_dns(&e, "a.test"), None);
    cell.store(at_home());
    assert!(at_dns(&e, "a.test").is_some());
    cell.store(at_a_desk_with_wifi_up());
    assert_eq!(at_dns(&e, "a.test"), None);
}

/// The kinds that look at the name alone decide at the DNS layer; so does
/// `HOSTNAME-TYPE`. A rule that waits for the process is not a hit.
#[test]
fn name_rules_and_the_host_name_type_decide_at_the_dns_layer() {
    let cases = [
        ("DOMAIN,a.test", "a.test", true),
        ("DOMAIN,a.test", "b.test", false),
        ("DOMAIN-SUFFIX,b.test", "www.b.test", true),
        ("DOMAIN-KEYWORD,key", "a-key.test", true),
        ("DOMAIN-WILDCARD,w?.test", "w1.test", true),
        ("DOMAIN-WILDCARD,w?.test", "ww1.test", false),
    ];
    for (rule, name, hit) in cases {
        let e = engine_with_geo(&flagged(rule));
        assert_eq!(at_dns(&e, name).is_some(), hit, "{rule}: {name}");
    }
    let e = engine_with_geo(&flagged_set("HOSTNAME-TYPE,SIMPLE"));
    assert!(at_dns(&e, "nas").is_some());
    assert_eq!(at_dns(&e, "a.test"), None);
    let e = engine_with_geo(&flagged_set("PROCESS-NAME,x"));
    assert_eq!(at_dns(&e, "a.test"), None);
}

/// Hands every `DOMAIN-SET` rule the same set of names.
struct DomainSetOf(SetHandle);

impl SetLookup for DomainSetOf {
    fn lookup(&self, _: &ResourceRef, kind: SetKind) -> SetRef {
        assert_eq!(kind, SetKind::DomainSet);
        Arc::new(self.0.clone())
    }
}

/// A name a set holds is a hit at the DNS layer whatever else the set holds;
/// a name it does not hold is one only if the set has no line about the
/// connection. The hit names the line. A `DOMAIN-SET` is names only and
/// decides as it does at the SYN layer.
#[test]
fn set_lines_that_need_the_connection_keep_the_set_from_the_dns_layer() {
    let line = |m: Option<PreMatch>| m.and_then(|m| m.sub_rule).map(|h| (h.set, h.entry));
    let hit = |set: &str, entry: &str| Some((set.to_string(), entry.to_string()));
    // a port line that holds for the 0 of a query, and for 80 and 443
    let e = engine_with_geo(&flagged_set("DOMAIN-SUFFIX,b.test\nDEST-PORT,<1024"));
    assert_eq!(line(at_dns(&e, "b.test")), hit("S", "DOMAIN-SUFFIX,b.test"));
    assert_eq!(at_dns(&e, "c.test"), None);
    // the connection decides what the name does not
    assert_eq!(line(at_syn(&e, "c.test", 443)), hit("S", "DEST-PORT,<1024"));
    assert_eq!(at_syn(&e, "c.test", 8080), None);
    // the same with the port line of the manual's kind
    let e = engine_with_geo(&flagged_set("DOMAIN-SUFFIX,b.test\nDEST-PORT,443"));
    assert_eq!(line(at_dns(&e, "b.test")), hit("S", "DOMAIN-SUFFIX,b.test"));
    assert_eq!(at_dns(&e, "c.test"), None);
    // a line that matches wins over a line before it that waits for the
    // connection
    let e = engine_with_geo(&flagged_set("DEST-PORT,<1024\nDOMAIN-KEYWORD,key"));
    assert_eq!(
        line(at_dns(&e, "a-key.test")),
        hit("S", "DOMAIN-KEYWORD,key")
    );
    assert_eq!(at_dns(&e, "c.test"), None);
    // a set of names only has nothing to wait for: a name it does not hold is
    // no hit, and stays none at the SYN layer
    let e = engine_with_geo(&flagged_set("DOMAIN-SUFFIX,b.test"));
    assert!(at_dns(&e, "b.test").is_some());
    assert_eq!(at_dns(&e, "c.test"), None);
    assert_eq!(at_syn(&e, "c.test", 443), None);

    // a domain set
    let cfg = load(&flagged("DOMAIN-SET,names.list"));
    let names = HashSet::new();
    let ctx = ParseCtx {
        inline_rulesets: &names,
        base_dir: Path::new("."),
        reach: Reach::Anywhere,
    };
    let parsed = parse_set(SetKind::DomainSet, "b.test\n.d.test\n", &ctx);
    let set = SetHandle::new(CompiledSet::compile(
        "names",
        SetKind::DomainSet,
        &parsed,
        &NoNested,
        1,
    ));
    let e = RuleEngine::build(&cfg, &DomainSetOf(set), Arc::new(NoGeo)).unwrap();
    assert_eq!(line(at_dns(&e, "b.test")), hit("names", "b.test"));
    assert_eq!(line(at_dns(&e, "x.d.test")), hit("names", ".d.test"));
    assert_eq!(at_dns(&e, "c.test"), None);
    assert_eq!(at_dns(&e, "b.test.example"), None);
}

/// A hit is counted once per call that returns it, at either layer; a miss
/// counts nothing, and what a rule that missed found in a set is not handed
/// to the next.
#[test]
fn pre_match_counts_a_hit_and_names_the_sub_rule() {
    let e = engine_with_geo(
        "[Proxy]\nP = direct\n[Rule]\n\
AND,((RULE-SET,Listed),(DEST-PORT,1)),REJECT,pre-matching\n\
RULE-SET,Listed,REJECT-DROP,pre-matching\n\
DOMAIN,plain.example,REJECT,pre-matching\nFINAL,DIRECT\n\
[Ruleset Listed]\nDOMAIN,listed.example\n",
    );
    let hits = || -> Vec<u64> { e.rules().iter().map(|r| r.hits()).collect() };
    assert_eq!(hits(), [0, 0, 0, 0]);

    // the first rule has the set's line but not the port: it is a miss, and the
    // second rule's hit is its own
    let m = at_syn(&e, "listed.example", 443).expect("the second rule");
    assert_eq!(
        (m.rule, m.policy.name().as_str()),
        (1, "REJECT-DROP"),
        "{m:?}"
    );
    assert_eq!(
        m.sub_rule,
        Some(SubRuleHit {
            set: "Listed".into(),
            entry: "DOMAIN,listed.example".into()
        })
    );
    assert_eq!(hits(), [0, 1, 0, 0]);
    // the same name asked at the DNS layer: the port of the first rule is not
    // known, so it is a miss as well
    let m = at_dns(&e, "listed.example").expect("the second rule");
    assert_eq!(m.rule, 1);
    assert_eq!(hits(), [0, 2, 0, 0]);

    // a rule without a set names no line
    let m = at_dns(&e, "plain.example").expect("the third rule");
    assert_eq!((m.rule, m.policy.name().as_str()), (2, "REJECT"));
    assert_eq!(m.sub_rule, None);
    assert_eq!(hits(), [0, 2, 1, 0]);

    // misses count nothing, at either layer
    assert_eq!(at_dns(&e, "other.example"), None);
    assert_eq!(at_syn(&e, "other.example", 443), None);
    assert_eq!(hits(), [0, 2, 1, 0]);

    // the first rule matches when the port does
    let m = at_syn(&e, "listed.example", 1).expect("the first rule");
    assert_eq!(m.rule, 0);
    assert_eq!(m.sub_rule.map(|h| h.set).as_deref(), Some("Listed"));
    assert_eq!(hits(), [1, 2, 1, 0]);
}

/// `evaluate` has no use for a verdict that says "the connection is needed":
/// every session it is given has one. The decisions of rules that look at
/// ports, addresses, `NOT` and sets, flagged `pre-matching` or not, are the
/// ones they were before the pre-matching entry knew a verdict of that kind,
/// and no step of a trace says it.
#[tokio::test]
async fn the_rule_engine_never_sees_needs_connection() {
    const ORDER: &str = "\
[Proxy]
P = direct
[Rule]
DEST-PORT,8080,REJECT
IP-CIDR,10.0.0.0/8,P,no-resolve
RULE-SET,Mixed,P
NOT,((DEST-PORT,22)),REJECT
FINAL,DIRECT
[Ruleset Mixed]
DOMAIN-SUFFIX,b.test
DEST-PORT,9090
";
    const FLAGGED: &str = "\
[Proxy]
P = direct
[Rule]
RULE-SET,Mixed,REJECT,pre-matching
NOT,((DEST-PORT,443)),REJECT,pre-matching
AND,((DOMAIN,a.test),(DEST-PORT,443)),REJECT,pre-matching
FINAL,P
[Ruleset Mixed]
DOMAIN-SUFFIX,b.test
DEST-PORT,9090
";
    let cases = [
        // written order: a port, an address, a set (its domain line, its
        // port line), a NOT over a port, and the FINAL
        (ORDER, "c.test", 8080, (Reason::Rule, Some(0), "REJECT")),
        (ORDER, "10.1.1.1", 443, (Reason::Rule, Some(1), "P")),
        (ORDER, "b.test", 443, (Reason::Rule, Some(2), "P")),
        (ORDER, "c.test", 9090, (Reason::Rule, Some(2), "P")),
        (ORDER, "c.test", 443, (Reason::Rule, Some(3), "REJECT")),
        (ORDER, "c.test", 22, (Reason::Final, Some(4), "DIRECT")),
        // flagged: the set by its port line, a NOT over a port, an AND with
        // a port; and a session none of them takes
        (
            FLAGGED,
            "c.test",
            9090,
            (Reason::PreMatching, Some(0), "REJECT"),
        ),
        (
            FLAGGED,
            "c.test",
            80,
            (Reason::PreMatching, Some(1), "REJECT"),
        ),
        (
            FLAGGED,
            "a.test",
            443,
            (Reason::PreMatching, Some(2), "REJECT"),
        ),
        (FLAGGED, "c.test", 443, (Reason::Final, Some(3), "P")),
    ];
    for (profile, host, port, (reason, matched, policy)) in cases {
        let e = engine(profile);
        let s = SessionInfo::tcp(HostName::parse(host), port);
        let (d, trace) = e.evaluate_traced(&s, OutboundMode::Rule, &Panicking).await;
        assert_eq!(
            (d.reason, d.matched, d.policy().map(named).unwrap()),
            (reason, matched, policy.to_string()),
            "{host}:{port}"
        );
        assert!(
            trace.iter().all(|t| t.verdict != "needs-connection"),
            "{host}:{port}: {:?}",
            steps(&trace)
        );
    }
}
