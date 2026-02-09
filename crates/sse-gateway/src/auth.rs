//! Authentication module for SSE Gateway
//!
//! Provides a simple callback-based authentication.

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Request context passed to the auth callback
#[derive(Debug, Clone)]
pub struct AuthRequest {
    /// HTTP headers from the request
    pub headers: HeaderMap,
    /// The channel ID being requested
    pub channel_id: String,
    /// Client IP address (from X-Forwarded-For or direct connection)
    pub client_ip: Option<String>,
}

impl AuthRequest {
    /// Get a header value as string
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// Get the Bearer token from Authorization header
    pub fn bearer_token(&self) -> Option<&str> {
        self.header("authorization")
            .and_then(|auth| auth.strip_prefix("Bearer "))
    }
}

/// Auth callback result - None means allowed, Some(Response) means denied
pub type AuthResponse = Option<Response>;

/// Type alias for the async auth callback function
///
/// Return `None` to allow the connection, or `Some(Response)` to deny with custom response.
pub type AuthFn = Arc<
    dyn Fn(AuthRequest) -> Pin<Box<dyn Future<Output = AuthResponse> + Send>> + Send + Sync,
>;

/// Helper to create an auth callback from a closure
pub fn auth_fn<F, Fut>(f: F) -> AuthFn
where
    F: Fn(AuthRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = AuthResponse> + Send + 'static,
{
    Arc::new(move |req| Box::pin(f(req)))
}

/// Helper to create a simple error response
pub fn deny(status: StatusCode, message: impl Into<String>) -> Response {
    (status, message.into()).into_response()
}

/// Helper to create a JSON error response
pub fn deny_json(status: StatusCode, body: impl serde::Serialize) -> Response {
    (status, axum::Json(body)).into_response()
}
