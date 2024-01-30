use std::time::Duration;

use anyhow::{ensure, Result};
use axum::{
    error_handling::HandleErrorLayer,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    BoxError, Json, Router,
};
use dotenv::dotenv;
use lazy_static::lazy_static;
use listenfd::ListenFd;
use paris::{error, info, log};
use serde_json::Value;
use tokio::{net::TcpListener, sync::Mutex};
use tower::{buffer::BufferLayer, limit::RateLimitLayer, ServiceBuilder};

mod constants;

lazy_static! {
    static ref CLIENT: Mutex<SpotifyClient> = Mutex::new(SpotifyClient::new());
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv()?;

    // Check if SP_DC is set
    ensure!(
        std::env::var("SP_DC").is_ok(),
        "SP_DC is not set. Please set it to your sp_dc cookie value."
    );

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .route("/lyrics/:track_id", get(lyrics))
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(|err: BoxError| async move {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Unhandled error: {}", err),
                    )
                }))
                .layer(BufferLayer::new(1024))
                .layer(RateLimitLayer::new(5, Duration::from_secs(1))),
        );

    let mut listenfd = ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0).unwrap() {
        // if we are given a tcp listener on listen fd 0, we use that one
        Some(listener) => TcpListener::from_std(listener).unwrap(),
        // otherwise fall back to local listening
        None => TcpListener::bind(format!(
            "127.0.0.1:{}",
            std::env::var("PORT").unwrap_or("3000".into())
        ))
        .await
        .unwrap(),
    };

    info!("Listening on <b>{}</>", listener.local_addr().unwrap());
    axum::serve(listener, app).await?;

    Ok(())
}

async fn root() -> String {
    format!("{} v{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

async fn lyrics(headers: HeaderMap, Path(track_id): Path<String>) -> Result<Json<Value>, AppError> {
    let authorization = headers
        .get("authorization")
        .ok_or_else(|| anyhow::anyhow!("Authorization header not found"))?;

    let authorization = authorization.to_str()?;

    let authorization = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow::anyhow!("Authorization header not found"))?;

    log!("Authorization: {}", authorization);

    if authorization != std::env::var("API_KEY")? {
        return Err(anyhow::anyhow!("Invalid API key").into());
    }

    let lyrics = CLIENT.lock().await.get_lyrics(&track_id).await?;

    Ok(Json(lyrics))
}

#[derive(Debug)]
struct SpotifyClient {
    access_token: Option<String>,
    expires_at: Option<u64>,
    user_agent: String,
}

impl SpotifyClient {
    fn new() -> Self {
        Self {
            access_token: None,
            expires_at: None,
            user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/101.0.0.0 Safari/537.36".to_string(),
        }
    }

    async fn get_access_token(&mut self) -> Result<(), anyhow::Error> {
        let client = reqwest::Client::new();

        let response = client
            .get(constants::TOKEN_URL)
            .header("App-platform", "WebPlayer")
            .header("Cookie", format!("sp_dc={}", std::env::var("SP_DC")?))
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "text/html")
            .send()
            .await?;

        // log!("Response: {:?}", response);

        let parsed = serde_json::from_str::<Value>(&response.text().await?)?;

        log!("Parsed: {:?}", parsed);
        // let parsed = response.json::<Value>().await?;

        self.access_token = Some(
            parsed
                .get("accessToken")
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
        );
        self.expires_at = Some(
            parsed
                .get("accessTokenExpirationTimestampMs")
                .unwrap()
                .as_u64()
                .unwrap(),
        );

        Ok(())
    }

    async fn get_lyrics(&mut self, track_id: &str) -> Result<Value, anyhow::Error> {
        if self.access_token.is_none()
            || self.expires_at.is_none()
            || self.expires_at.unwrap() < (chrono::Utc::now().timestamp_millis() as u64)
        {
            log!("Refreshing access token");
            self.get_access_token().await?;
            info!("Access token refreshed");
        }

        let client = reqwest::Client::new();

        log!("Access Token: {}", self.access_token.as_ref().unwrap());

        let response = client
            .get(format!(
                "{}{}?format=json&market=from_token",
                constants::LYRICS_URL,
                track_id,
            ))
            .header("App-platform", "WebPlayer")
            .header(
                "Authorization",
                format!("Bearer {}", self.access_token.as_ref().unwrap()),
            )
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "text/html")
            .send()
            .await?;

        match response.status().as_u16() {
            200 => {
                let parsed = response.json::<Value>().await?;
                Ok(parsed.get("lyrics").unwrap().clone())
            }

            _ => {
                error!("Response: {:?}", response);
                Err(anyhow::anyhow!("Something went wrong"))
            }
        }
    }
}

// Make our own error that wraps `anyhow::Error`.
struct AppError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

// This enables using `?` on functions that return `Result<_, anyhow::Error>` to turn them into
// `Result<_, AppError>`. That way you don't need to do that manually.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
