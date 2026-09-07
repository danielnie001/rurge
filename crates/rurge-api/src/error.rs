//! The unified error body (M4 design §5.3): every failure is
//! `{"error": "<message>"}` with the matching status code.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn unauthorized() -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
    pub fn banned() -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "banned")
    }
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, message)
    }
    /// Reserved for a later task (e.g. reload already in progress); unused until then.
    #[allow(dead_code)]
    pub fn conflict(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, message)
    }
    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
    pub fn not_implemented(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// Unwraps a JSON body, turning axum's rejection (bad JSON, wrong
/// content-type, missing field) into our 400 body.
pub fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    match body {
        Ok(Json(v)) => Ok(v),
        Err(e) => Err(ApiError::bad_request(e.body_text())),
    }
}
