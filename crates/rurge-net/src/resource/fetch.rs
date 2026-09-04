//! One conditional fetch of a URL resource.

use super::cache::Meta;
use crate::http::{HttpClient, RequestOpts};
use bytes::Bytes;
use http::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use http::{HeaderMap, HeaderName, HeaderValue};
use std::time::Duration;
use url::Url;

pub enum Fetched {
    New {
        data: Bytes,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    NotModified,
}

pub async fn fetch(
    client: &HttpClient,
    url: &Url,
    meta: Option<&Meta>,
    timeout: Duration,
    max_size: u64,
) -> Result<Fetched, String> {
    let mut opts = RequestOpts {
        timeout,
        max_body: max_size,
        ..RequestOpts::default()
    };
    if let Some(m) = meta {
        if let Some(v) = m
            .etag
            .as_deref()
            .and_then(|e| HeaderValue::from_str(e).ok())
        {
            opts.headers.push((IF_NONE_MATCH, v));
        }
        if let Some(v) = m
            .last_modified
            .as_deref()
            .and_then(|e| HeaderValue::from_str(e).ok())
        {
            opts.headers.push((IF_MODIFIED_SINCE, v));
        }
    }
    let resp = client.get(url, &opts).await.map_err(|e| e.to_string())?;
    match resp.status.as_u16() {
        304 => Ok(Fetched::NotModified),
        200 => Ok(Fetched::New {
            data: resp.body,
            etag: header(&resp.headers, &ETAG),
            last_modified: header(&resp.headers, &LAST_MODIFIED),
        }),
        s => Err(format!("http status {s}")),
    }
}

fn header(h: &HeaderMap, name: &HeaderName) -> Option<String> {
    h.get(name)?.to_str().ok().map(str::to_string)
}
