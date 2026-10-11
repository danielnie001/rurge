//! What a set of a million entries costs in memory. A measurement, not a gate:
//! the three tests are ignored. Each builds one set the way a set file is
//! loaded (text, parsed lines, compiled set), asks the set a few hundred
//! thousand questions, and prints the counts. The number to read is the peak
//! working set of the process, taken from the operating system while the test
//! runs (`PeakWorkingSetSize` on Windows, `VmHWM` in `/proc/<pid>/status` on
//! Linux), so every test is run alone, in release:
//!
//! ```text
//! cargo test --release --test memory -- --ignored --exact <name> --nocapture
//! ```
//!
//! `nothing_is_built_is_the_baseline` is the same process with no set in it.

use rurge_config::HostName;
use rurge_config::rule::{ParseCtx, ResourceRef};
use rurge_config::session::SessionInfo;
use rurge_rules::matcher::{EvalCtx, NoGeo, SetLookup, SetRef, Verdict};
use rurge_rules::set::{CompiledSet, SetHandle};
use rurge_rules::set_format::{MAX_ENTRIES, SetKind, parse_set};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// How many entries are asked about once the set is built.
const ASKED: usize = 200_000;
const TLDS: [&str; 5] = ["com", "net", "org", "io", "cn"];

/// Entry `i`'s name, three labels like the names in a real list. No two are
/// alike: the first label is a one-to-one function of `i`.
fn name(i: usize) -> String {
    let i = u32::try_from(i).expect("an entry number");
    let a = i.wrapping_mul(0x9E37_79B1);
    let b = i.wrapping_mul(0x85EB_CA6B) ^ 0xC2B2_AE35;
    format!("h{a:x}.d{b:x}.{}", TLDS[i as usize % TLDS.len()])
}

/// The `k`-th /24 network. No two are alike and they are spread over the whole
/// address space, which is the worst case for a prefix tree.
fn network(k: usize) -> [u8; 3] {
    let k = u32::try_from(k).expect("a network number");
    let x = k.wrapping_mul(0x9E37_79B1) & 0x00FF_FFFF;
    [(x >> 16) as u8, (x >> 8) as u8, x as u8]
}

/// A kind of set, and how to write entry `i` of it and how to ask for it.
struct Subject {
    kind: SetKind,
    /// The line of entry `i` in the set's file.
    line: fn(usize) -> String,
    /// The host of a session that entry `i` answers.
    target: fn(usize) -> String,
}

/// Half suffix entries (`.name`), half exact ones.
const DOMAIN_SET: Subject = Subject {
    kind: SetKind::DomainSet,
    line: |i| {
        if i % 2 == 0 {
            format!(".{}", name(i))
        } else {
            name(i)
        }
    },
    target: |i| {
        if i % 2 == 0 {
            format!("www.{}", name(i))
        } else {
            name(i)
        }
    },
};

/// Three in five `DOMAIN-SUFFIX`, one in five `DOMAIN`, one in five `IP-CIDR`
/// (a /24 each): all three indexes a set has are filled.
const RULE_SET: Subject = Subject {
    kind: SetKind::RuleSet,
    line: |i| match i % 5 {
        0..=2 => format!("DOMAIN-SUFFIX,{}", name(i)),
        3 => format!("DOMAIN,{}", name(i)),
        _ => {
            let [a, b, c] = network(i / 5);
            format!("IP-CIDR,{a}.{b}.{c}.0/24")
        }
    },
    target: |i| match i % 5 {
        0..=2 => format!("www.{}", name(i)),
        3 => name(i),
        _ => {
            let [a, b, c] = network(i / 5);
            format!("{a}.{b}.{c}.7")
        }
    },
};

/// No set in these sets refers to another.
struct NoNesting;

impl SetLookup for NoNesting {
    fn lookup(&self, _: &ResourceRef, kind: SetKind) -> SetRef {
        Arc::new(SetHandle::new(CompiledSet::empty("nested", kind)))
    }
}

fn build_and_ask(subject: &Subject) {
    // the file's bytes stay: the resource that holds them keeps them while
    // they are the current version
    let mut text = String::with_capacity(MAX_ENTRIES * 48);
    for i in 0..MAX_ENTRIES {
        text.push_str(&(subject.line)(i));
        text.push('\n');
    }
    let inline = HashSet::new();
    let ctx = ParseCtx {
        inline_rulesets: &inline,
        base_dir: Path::new("."),
    };
    let parsed = parse_set(subject.kind, &text, &ctx);
    assert_eq!(
        (parsed.lines.len(), parsed.skipped_total, parsed.truncated),
        (MAX_ENTRIES, 0, 0),
        "every generated line is an entry"
    );
    let parsed_lines = parsed.lines.len();
    let set = CompiledSet::compile("a million entries", subject.kind, &parsed, &NoNesting, 1);
    drop(parsed);
    assert_eq!(set.entry_count(), MAX_ENTRIES);

    // The set is asked in earnest, so that it is alive and busy when the
    // process is last read: every answer must be the entry that was asked for.
    let geo = NoGeo;
    for n in 0..ASKED {
        let i = n * 7919 % MAX_ENTRIES;
        let session = SessionInfo::tcp(HostName::parse(&(subject.target)(i)), 443);
        let answer = set.eval(&session, &mut EvalCtx::new(&geo), true, false);
        assert_eq!(answer.verdict, Verdict::Match, "entry {i}");
        assert_eq!(
            answer.entry.as_deref(),
            Some((subject.line)(i).as_str()),
            "entry {i}"
        );
    }
    let unlisted = SessionInfo::tcp(HostName::parse("nothing.listed.invalid"), 443);
    let answer = set.eval(&unlisted, &mut EvalCtx::new(&geo), true, false);
    assert_eq!(answer.verdict, Verdict::NoMatch);
    println!(
        "{}: {parsed_lines} entries parsed, {} compiled, {ASKED} asked and answered, {} bytes of text",
        subject.kind.keyword(),
        set.entry_count(),
        text.len()
    );
}

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn a_domain_set_of_a_million_entries() {
    build_and_ask(&DOMAIN_SET);
}

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn a_rule_set_of_a_million_entries() {
    build_and_ask(&RULE_SET);
}

#[test]
#[ignore = "a measurement, not a gate: run on its own, in release"]
fn nothing_is_built_is_the_baseline() {
    println!("no set: 0 entries");
}
