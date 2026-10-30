//! `GET /v1/mitm/ca`: the certificate authority this generation signs the
//! decrypted sessions with (`phase 4 M2 design 12.2`), as a DER file a client
//! can be asked to trust. `404` when the generation has none rurge can sign
//! with, so that a client learns the difference between "no CA" and an empty
//! answer.

use crate::App;
use crate::error::{ApiError, ApiResult};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

pub async fn ca(State(app): State<App>) -> ApiResult<Response> {
    let der = app
        .engine
        .mitm_ca_der()
        .ok_or_else(|| ApiError::not_found("no usable MITM CA"))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-x509-ca-cert"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"rurge-ca.cer\"",
            ),
        ],
        der,
    )
        .into_response())
}
