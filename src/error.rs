use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Every fallible handler returns this. It maps cleanly onto a JSON error body
/// so the frontend can always `await res.json()` and find `.error`.
#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        AppError {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        AppError::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        AppError::new(StatusCode::NOT_FOUND, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

pub type AppResult<T> = Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            eprintln!("error: {}", self.message);
        }
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::not_found("not found"),
            other => AppError::internal(format!("database error: {other}")),
        }
    }
}

impl From<r2d2::Error> for AppError {
    fn from(e: r2d2::Error) -> Self {
        AppError::internal(format!("database pool error: {e}"))
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::internal(format!("io error: {e}"))
    }
}
