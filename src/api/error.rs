use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::ApiResponse;

/// Custom error type for API handlers
#[derive(Debug)]
pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound(msg) => {
                tracing::warn!(target: "api::error", status = 404, error = %msg, "Not found error");
                (StatusCode::NOT_FOUND, msg)
            }
            AppError::BadRequest(msg) => {
                tracing::warn!(target: "api::error", status = 400, error = %msg, "Bad request error");
                (StatusCode::BAD_REQUEST, msg)
            }
            AppError::Internal(err) => {
                tracing::error!(target: "api::error", status = 500, error = ?err, "Internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".to_string(),
                )
            }
        };

        let body = Json(ApiResponse::<()>::error(message));
        (status, body).into_response()
    }
}
