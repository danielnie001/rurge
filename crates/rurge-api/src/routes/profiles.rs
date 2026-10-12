//! `GET /v1/profiles/current`, `POST /v1/profiles/reload|check` (M4 D3).

use crate::App;
use crate::error::{ApiError, ApiResult, query_params};
use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use rurge_config::Severity;
use rurge_engine::check_profile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Deserialize)]
pub struct CurrentQuery {
    pub sensitive: Option<u8>,
}

pub async fn current(
    State(app): State<App>,
    q: Result<Query<CurrentQuery>, QueryRejection>,
) -> ApiResult<impl IntoResponse> {
    let q = query_params(q)?;
    let sensitive = q.sensitive.unwrap_or(0) != 0;
    let text = app
        .engine
        .config_text(sensitive)
        .await
        .map_err(|e| ApiError::internal(format!("cannot read the profile: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub listeners_rebound: bool,
}

pub async fn reload(State(app): State<App>) -> Json<ReloadJson> {
    let report = app.control.reload().await;
    Json(ReloadJson {
        ok: report.ok,
        errors: report.errors,
        warnings: report.warnings,
        listeners_rebound: report.listeners_rebound,
    })
}

#[derive(Serialize)]
pub struct CheckJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub diagnostics: Vec<Value>,
    /// What the manual's answer has: `null` for a profile with no error in it,
    /// otherwise the first error and how many more there are. The wording is
    /// rurge's own; the manual only says that an error is returned.
    pub error: Option<String>,
}

/// What the check says of a body it cannot take a profile name from. Fixed:
/// nothing the caller sent comes back in it.
const BAD_BODY: &str = "the body must be a JSON object; \"name\", when given, must be a string";

/// What it says of a name that is not the running profile's.
const NO_SUCH_PROFILE: &str = "profile not found";

/// The `name` of a check request, if it has one, or why the body has none to
/// read. The body is read here and not by an extractor that answers for
/// itself: the text such an extractor gives for a body of the wrong type
/// quotes what was sent.
fn requested_name(body: Result<Bytes, BytesRejection>) -> ApiResult<Option<String>> {
    let bad = || ApiError::bad_request(BAD_BODY);
    let body = body.map_err(|_| bad())?;
    // No body is no name, and neither is a body of nothing but whitespace.
    if body
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
    {
        return Ok(None);
    }
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(mut fields)) => match fields.remove("name") {
            None => Ok(None),
            Some(Value::String(name)) => Ok(Some(name)),
            Some(_) => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// Whether `name` names the profile at `path`: the name of its file, or that
/// without its last extension, compared exactly. A file name that is not
/// Unicode is named by nothing.
fn names(path: &Path, name: &str) -> bool {
    let file = path.file_name().and_then(|file| file.to_str());
    let stem = path.file_stem().and_then(|stem| stem.to_str());
    [file, stem].contains(&Some(name))
}

/// Re-validates the profile on disk with the daemon's load options; never
/// touches the running config. The dry build runs too, so a policy that
/// cannot be built is reported here exactly as a reload would report it, and
/// so does an offline assembly from the cached subscriptions.
///
/// The request may carry the manual's `{"name": "Profile2"}`. Surge picks one
/// of the profiles it keeps in a directory with it; rurge has the one that
/// runs, the file `-c` names, until it manages a directory of them. So a name
/// is either that profile's (its file's name, or that without its last
/// extension) or refused with a 404, and nothing is loaded for it. No body, a
/// body of nothing but whitespace and an object without a `name` check the
/// profile that runs; a body that is no JSON object, or has a `name` that is no
/// string, is a 400. Both refusals are given before the profile is loaded, in
/// fixed texts. The answer's `error` is `null` for a profile with no error in
/// it, otherwise the first error and how many more there are.
pub async fn check(
    State(app): State<App>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Json<CheckJson>> {
    let path = app.engine.profile_path();
    if requested_name(body)?.is_some_and(|name| !names(&path, &name)) {
        return Err(ApiError::not_found(NO_SUCH_PROFILE));
    }
    let opts = app.load_options.clone();
    let data_dir = app.engine.data_dir();
    let loaded = tokio::task::spawn_blocking(move || check_profile(&path, &opts, &data_dir))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::internal(format!("cannot load the profile: {e}")))?;
    let diagnostics = loaded.diagnostics.sorted();
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    let error = diagnostics
        .iter()
        .find(|d| d.severity == Severity::Error)
        .map(|first| match errors - 1 {
            0 => first.headline().to_string(),
            more => format!("{} (and {more} more)", first.headline()),
        });
    Ok(Json(CheckJson {
        ok: errors == 0,
        errors,
        warnings,
        diagnostics: diagnostics
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
            .collect(),
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn the_file_name_and_the_file_name_without_its_last_extension_name_the_profile() {
        let path = Path::new("profiles/config.conf");
        assert!(names(path, "config.conf"));
        assert!(names(path, "config"));
    }

    /// Nothing else does: no case folding, no path, no other form of the name.
    #[test]
    fn no_other_name_names_it() {
        let path = Path::new("profiles/config.conf");
        for name in [
            "",
            "Config",
            "CONFIG.CONF",
            "config.",
            "config.conf ",
            " config",
            "conf",
            ".conf",
            "config.conf.bak",
            "profiles/config.conf",
            "profiles\\config.conf",
            "./config.conf",
            "../config",
            "config/",
        ] {
            assert!(!names(path, name), "{name:?}");
        }
    }

    #[test]
    fn only_the_last_extension_comes_off() {
        let path = Path::new("work.backup.conf");
        assert!(names(path, "work.backup.conf"));
        assert!(names(path, "work.backup"));
        assert!(!names(path, "work"));
    }

    #[test]
    fn a_file_without_an_extension_and_a_dot_file_have_one_name() {
        assert!(names(Path::new("surge"), "surge"));
        assert!(!names(Path::new("surge"), ""));
        assert!(names(Path::new(".surge"), ".surge"));
        assert!(!names(Path::new(".surge"), "surge"));
        assert!(!names(Path::new(".surge"), ""));
    }

    #[cfg(unix)]
    fn not_unicode() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![0xff, b'.', b'c', b'o', b'n', b'f'])
    }

    #[cfg(windows)]
    fn not_unicode() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0xd800, 0x2e, 0x63, 0x6f, 0x6e, 0x66])
    }

    /// A name is compared as the text it is; the file name that is not text is
    /// not made into some by replacing what is wrong with it.
    #[test]
    fn a_file_name_that_is_not_unicode_is_named_by_nothing() {
        let path = Path::new("profiles").join(not_unicode());
        for name in ["", ".conf", "conf", "\u{fffd}.conf", "\u{fffd}", "profiles"] {
            assert!(!names(&path, name), "{name:?}");
        }
    }
}
