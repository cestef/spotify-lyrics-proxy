use std::collections::HashMap;

use anyhow::{ensure, Result};
use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use lazy_static::lazy_static;
use listenfd::ListenFd;
use paris::{error, info, log, warn};
use rand::seq::SliceRandom;
use serde_json::Value;
use tokio::{net::TcpListener, sync::Mutex};

mod constants;

#[derive(serde::Deserialize)]
struct Config {
    port: Option<u16>,
    api_keys: Option<Vec<String>>,
    cookies: Vec<String>,
}

lazy_static! {
    static ref CLIENT: Mutex<SpotifyClient> = Mutex::new(SpotifyClient::new());
    static ref CONFIG: Config = toml::from_str(
        &std::fs::read_to_string("config.toml").expect("Failed to read config.toml")
    )
    .expect("Failed to parse config.toml");
}

#[tokio::main]
async fn main() -> Result<()> {
    ensure!(
        CONFIG.cookies.len() > 0,
        "You must provide at least one sp_dc cookie"
    );

    if CONFIG.api_keys.is_none() || CONFIG.api_keys.as_ref().unwrap().len() == 0 {
        warn!("No API key provided, this means anyone can use your API");
    }

    // build our application with a route
    let app = Router::new()
        .route("/", get(root))
        .route("/lyrics/:track_id", get(lyrics));

    let mut listenfd = ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0).unwrap() {
        // if we are given a tcp listener on listen fd 0, we use that one
        Some(listener) => TcpListener::from_std(listener).unwrap(),
        // otherwise fall back to local listening
        None => TcpListener::bind(format!("127.0.0.1:{}", CONFIG.port.unwrap_or(3000)))
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
    if let Some(api_keys) = &CONFIG.api_keys {
        let authorization = headers
            .get("authorization")
            .ok_or_else(|| anyhow::anyhow!("Authorization header not found"))?;

        let authorization = authorization.to_str()?;

        let authorization = authorization
            .strip_prefix("Bearer ")
            .ok_or_else(|| anyhow::anyhow!("Authorization header not found"))?;

        log!("Authorization: {}", authorization);

        if !api_keys.contains(&authorization.to_string()) {
            return Err(anyhow::anyhow!("Invalid API key").into());
        }
    }

    let lyrics = CLIENT.lock().await.get_lyrics(&track_id).await?;

    Ok(Json(lyrics))
}

#[derive(Debug)]
struct SpotifyClient {
    access_tokens: HashMap<String, AccessToken>,
    user_agent: String,
}

#[derive(Debug)]
struct AccessToken {
    token: String,
    expires_at: u64,
}

impl SpotifyClient {
    fn new() -> Self {
        Self {
            access_tokens: HashMap::new(),
            user_agent: constants::USER_AGENT.to_string(),
        }
    }

    async fn get_access_token(&mut self, cookie: String) -> Result<(), anyhow::Error> {
        let client = reqwest::Client::new();

        let response = client
            .get(constants::TOKEN_URL)
            .header("App-platform", "WebPlayer")
            .header("Cookie", format!("sp_dc={}", cookie))
            .header("User-Agent", &self.user_agent)
            .header("Content-Type", "text/html")
            .send()
            .await?;

        // log!("Response: {:?}", response);

        let parsed = serde_json::from_str::<Value>(&response.text().await?)?;

        // log!("Parsed: {:?}", parsed);

        self.access_tokens.insert(
            cookie,
            AccessToken {
                token: parsed
                    .get("accessToken")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string(),
                expires_at: parsed
                    .get("accessTokenExpirationTimestampMs")
                    .unwrap()
                    .as_u64()
                    .unwrap(),
            },
        );

        Ok(())
    }

    async fn get_lyrics(&mut self, track_id: &str) -> Result<Value, anyhow::Error> {
        let cookie = CONFIG
            .cookies
            .choose(&mut rand::thread_rng())
            .ok_or_else(|| anyhow::anyhow!("No cookies provided"))?;

        let access_token = self.access_tokens.get(cookie);

        let access_token = match access_token {
            Some(access_token) => {
                match access_token.expires_at > chrono::Utc::now().timestamp_millis() as u64 {
                    true => access_token,
                    false => {
                        self.get_access_token(cookie.to_string()).await?;
                        self.access_tokens.get(cookie).unwrap()
                    }
                }
            }
            None => {
                self.get_access_token(cookie.to_string()).await?;
                self.access_tokens.get(cookie).unwrap()
            }
        };

        let client = reqwest::Client::new();

        // log!("Access Token: {}", self.access_token.as_ref().unwrap());

        let response = client
            .get(format!(
                "{}{}?format=json&market=from_token",
                constants::LYRICS_URL,
                track_id,
            ))
            .header("App-platform", "WebPlayer")
            .header("Authorization", format!("Bearer {}", access_token.token))
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
