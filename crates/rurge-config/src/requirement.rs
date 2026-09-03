use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ReqError(pub String);

/// Split a profile line into (requirement expression, content).
/// Minimal version: only the simplified `#!IOS-ONLY` / `#!MACOS-ONLY` / `#!TVOS-ONLY`
/// prefix and suffix forms. Task 5 replaces this with the full implementation.
pub fn split_line(line: &str) -> Result<(Option<String>, String), ReqError> {
    const SIMPLIFIED: [(&str, &str); 3] = [
        ("#!IOS-ONLY", "SYSTEM == 'iOS'"),
        ("#!MACOS-ONLY", "SYSTEM == 'macOS'"),
        ("#!TVOS-ONLY", "SYSTEM == 'tvOS'"),
    ];
    for (tag, expr) in SIMPLIFIED {
        if let Some(rest) = line.strip_prefix(tag) {
            return Ok((Some(expr.to_string()), rest.trim().to_string()));
        }
        if let Some(body) = line.strip_suffix(tag) {
            let body = body.trim_end();
            if body.ends_with(char::is_whitespace) || body.is_empty() {
                return Ok((Some(expr.to_string()), body.trim().to_string()));
            }
            // e.g. "foo #!IOS-ONLY" -> body "foo " (already trimmed) ; require a separating space
            return Ok((Some(expr.to_string()), body.to_string()));
        }
    }
    Ok((None, line.trim().to_string()))
}
