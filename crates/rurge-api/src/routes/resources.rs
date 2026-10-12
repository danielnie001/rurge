//! `GET /v1/external_resources` and `POST /v1/external_resources/update`: the
//! files the profile and the generation in use download or read, and a way to
//! have them updated now.

use crate::App;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::rejection::BytesRejection;
use rurge_engine::{ExternalResource, UnusedLines};
use serde::Serialize;
use serde_json::{Map, Value};
use std::time::Duration;

/// The longest an update request waits for the downloads it started. A key that
/// is not answered by then says `timed out`.
const UPDATE_LIMIT: Duration = Duration::from_secs(60);

/// What the endpoint says of a body it cannot read an update from. Fixed:
/// nothing the caller sent comes back in it.
const BAD_BODY: &str = "the body must be a JSON object with a string \"key\"";

/// What it says of a key that names no resource that is listed.
const NO_SUCH_KEY: &str = "no external resource has this key";

/// One resource as the manual shows it: `updatedAt` is not there for a local
/// resource, `error` only when there is one to give (`ExternalResource::error`),
/// and rurge's own `suspended` only when it is true and `skipped`, `truncated`
/// and `unknownParams` on a rule set and a domain set only.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceJson {
    path: String,
    #[serde(rename = "type")]
    kind: &'static str,
    key: String,
    local: bool,
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<u64>,
    updating: bool,
    /// The module that refers to the resource; rurge has no modules yet, so
    /// every resource is the profile's and this is `null`.
    from_module: (),
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    suspended: bool,
    #[serde(flatten)]
    unused: Option<UnusedJson>,
}

/// What the last load of the file of a set could not use. A set that has
/// lines of that kind has loaded, so `error` has nothing to say of them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnusedJson {
    skipped: usize,
    truncated: usize,
    unknown_params: usize,
}

impl From<UnusedLines> for UnusedJson {
    fn from(unused: UnusedLines) -> UnusedJson {
        UnusedJson {
            skipped: unused.skipped,
            truncated: unused.truncated,
            unknown_params: unused.unknown_params,
        }
    }
}

impl From<ExternalResource> for ResourceJson {
    fn from(resource: ExternalResource) -> ResourceJson {
        ResourceJson {
            path: resource.path,
            kind: resource.kind,
            key: resource.key,
            local: resource.local,
            ready: resource.ready,
            updated_at: resource.updated_at,
            updating: resource.updating,
            from_module: (),
            error: resource.error,
            suspended: resource.suspended,
            unused: resource.unused.map(UnusedJson::from),
        }
    }
}

#[derive(Serialize)]
pub struct ListJson {
    defines: Vec<ResourceJson>,
}

pub async fn list(State(app): State<App>) -> Json<ListJson> {
    let defines = app
        .engine
        .external_resources(app.sources.as_deref())
        .into_iter()
        .map(ResourceJson::from)
        .collect();
    Json(ListJson { defines })
}

/// The `key` of an update request, or why there is none. The body is read
/// here and not by an extractor that answers for itself: the text such an
/// extractor gives for a body of the wrong type quotes what was sent.
fn requested_key(body: Result<Bytes, BytesRejection>) -> ApiResult<String> {
    let bad = || ApiError::bad_request(BAD_BODY);
    let body = body.map_err(|_| bad())?;
    match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(mut fields)) => match fields.remove("key") {
            Some(Value::String(key)) => Ok(key),
            _ => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// `{"key": "<key>"}` updates the resource that key names, `{"key": "all"}`
/// every one that is downloaded. The answer maps each key that was updated to
/// `"success"` or to why it was not; it is an object, and the order the engine
/// answers in (that of the listing) is not kept.
pub async fn update(
    State(app): State<App>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Json<Value>> {
    let key = requested_key(body)?;
    let wanted = (key != "all").then_some(key.as_str());
    let answered = app
        .engine
        .update_external_resources(app.sources.as_ref(), wanted, UPDATE_LIMIT)
        .await
        .ok_or_else(|| ApiError::bad_request(NO_SUCH_KEY))?;
    let mut out = Map::new();
    for (key, result) in answered {
        let said = match result {
            Ok(()) => "success".to_string(),
            Err(error) => error,
        };
        out.insert(key, Value::String(said));
    }
    Ok(Json(Value::Object(out)))
}
