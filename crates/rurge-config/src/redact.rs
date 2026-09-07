//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.

const SECRET_KEYS: [&str; 3] = ["password", "ca-passphrase", "ca-p12"];
const KEY_AT_KEYS: [&str; 4] = [
    "http-api",
    "external-controller-access",
    "http-listen",
    "socks5-listen",
];

pub fn redact_profile(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

fn redact_line(line: &str) -> String {
    let Some((key, value)) = line.split_once('=') else {
        return line.to_string();
    };
    let k = key.trim().to_ascii_lowercase();
    if SECRET_KEYS.contains(&k.as_str()) {
        return format!("{key}= ***");
    }
    if k == "wifi-access-http-auth" {
        let v = value.trim_start();
        let lead = &value[..value.len() - v.len()];
        return match v.split_once(':') {
            Some((user, _)) => format!("{key}={lead}{user}:***"),
            None => format!("{key}={lead}***"),
        };
    }
    if KEY_AT_KEYS.contains(&k.as_str()) {
        let items: Vec<String> = value
            .split(',')
            .map(|item| {
                let t = item.trim_start();
                let lead = &item[..item.len() - t.len()];
                match t.split_once('@') {
                    Some((_, rest)) => format!("{lead}***@{rest}"),
                    None => item.to_string(),
                }
            })
            .collect();
        return format!("{key}={}", items.join(","));
    }
    // policy lines: `name = type, host, port, password=..., psk=...`
    let mut redacted = value.to_string();
    for param in ["password", "psk", "private-key"] {
        redacted = redact_param(&redacted, param);
    }
    format!("{key}={redacted}")
}

/// Replaces the value of every `<param> = <value>` occurrence (up to the next
/// comma or end of line) with `***`, keeping the surrounding spacing.
fn redact_param(value: &str, param: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut rest = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(param) {
        let start = search + pos;
        // must be at a token boundary followed by optional spaces and '='
        let before_ok = start == 0 || matches!(lower.as_bytes()[start - 1], b',' | b' ' | b'\t');
        let after = &value[start + param.len()..];
        let eq = after.trim_start().strip_prefix('=');
        match (before_ok, eq) {
            (true, Some(tail)) => {
                let tail_start = value.len() - tail.len();
                let tail_lead = &tail[..tail.len() - tail.trim_start().len()];
                let val_len = tail
                    .trim_start()
                    .find(',')
                    .unwrap_or(tail.trim_start().len());
                out.push_str(&value[rest..tail_start]);
                out.push_str(tail_lead);
                out.push_str("***");
                rest = tail_start + tail_lead.len() + val_len;
                search = rest;
            }
            _ => search = start + param.len(),
        }
    }
    out.push_str(&value[rest..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_but_keeps_structure() {
        let text = "[General]\nhttp-api = s3cret@127.0.0.1:6171\nhttp-listen = pw@127.0.0.1:6152, 127.0.0.1:6153\nwifi-access-http-auth = alice:hunter2\n[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x1, udp-relay=true\nTJ = trojan, h, 443, password = y2\n[MITM]\nca-passphrase = abc\nca-p12 = MIIK...\n[Rule]\nFINAL,DIRECT\n";
        let out = redact_profile(text);
        assert!(out.contains("http-api = ***@127.0.0.1:6171"), "{out}");
        assert!(
            out.contains("http-listen = ***@127.0.0.1:6152, 127.0.0.1:6153"),
            "{out}"
        );
        assert!(out.contains("wifi-access-http-auth = alice:***"), "{out}");
        assert!(out.contains("password=***, udp-relay=true"), "{out}");
        assert!(out.contains("password = ***"), "{out}");
        assert!(
            out.contains("ca-passphrase = ***") && out.contains("ca-p12 = ***"),
            "{out}"
        );
        assert!(
            !out.contains("s3cret")
                && !out.contains("hunter2")
                && !out.contains("x1")
                && !out.contains("y2")
                && !out.contains("MIIK")
        );
        assert_eq!(out.lines().count(), text.lines().count());
        assert!(out.contains("FINAL,DIRECT"));
    }
}
