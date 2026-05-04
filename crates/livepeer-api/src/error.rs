use axum::{http::StatusCode, response::IntoResponse, Json};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

/// Standard error envelope per SPEC §14.4.
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub context: Option<serde_json::Value>,
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, code: "not_found", message: message.into(), context: None }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "bad_request", message: message.into(), context: None }
    }
    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: err.to_string(),
            context: None,
        }
    }
    pub fn with_context(mut self, ctx: serde_json::Value) -> Self {
        self.context = Some(ctx);
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "context": self.context,
            }
        });
        (self.status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::not_found("not found"),
            other => Self::internal(other),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Canonical JSON error envelope returned by failing API endpoints.")]
pub struct ErrorEnvelope {
    /// Standard top-level error wrapper returned by every failing API route.
    pub error: ErrorBody,
}

#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "Structured contents of the API error response.")]
pub struct ErrorBody {
    /// Stable machine-readable error class.
    pub code: String,
    /// Human-readable error message intended for operators and API consumers.
    pub message: String,
    /// Optional structured debugging context. Present only when the server has extra details.
    pub context: Option<serde_json::Value>,
}
