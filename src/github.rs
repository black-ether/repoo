use std::{
    env,
    process::{Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{
    StatusCode,
    blocking::Client,
    header::{HeaderMap, HeaderValue},
};
use serde::de::DeserializeOwned;

pub struct Github {
    client: Client,
    verbose: bool,
    pause_until: Mutex<Instant>,
}

impl Github {
    pub fn new(verbose: bool) -> Result<Self> {
        let token = env::var("GITHUB_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty())
            .or_else(|| {
                let result = Command::new("gh")
                    .args(["auth", "token"])
                    .env("GH_PROMPT_DISABLED", "1")
                    .stdin(Stdio::null())
                    .output()
                    .ok()?;
                result
                    .status
                    .success()
                    .then(|| String::from_utf8(result.stdout).ok())
                    .flatten()
                    .filter(|token| !token.trim().is_empty())
            });
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static("2022-11-28"),
        );
        if let Some(token) = token {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
                .map_err(|_| anyhow::anyhow!("GitHub token contains invalid header characters"))?;
            value.set_sensitive(true);
            headers.insert("authorization", value);
        } else {
            eprintln!("warning: GitHub requests are unauthenticated; API quota is lower");
        }
        let client = Client::builder()
            .user_agent(concat!("repoo/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            verbose,
            pause_until: Mutex::new(Instant::now()),
        })
    }

    pub fn get<T: DeserializeOwned>(&self, route: &[&str], query: &[(&str, &str)]) -> Result<T> {
        let mut url = reqwest::Url::parse("https://api.github.com")?;
        url.path_segments_mut()
            .expect("GitHub URL supports path segments")
            .extend(route);
        let mut last_message = String::new();
        for attempt in 0..=2 {
            loop {
                let delay = self
                    .pause_until
                    .lock()
                    .unwrap()
                    .saturating_duration_since(Instant::now());
                if delay.is_zero() {
                    break;
                }
                ensure!(
                    delay <= Duration::from_secs(60),
                    "GitHub rate limited; retry in {} seconds",
                    delay.as_secs()
                );
                thread::sleep(delay);
            }
            let response = self
                .client
                .get(url.clone())
                .query(query)
                .send()
                .context("GitHub request failed")?;
            let status = response.status();
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
            };
            let remaining = header("x-ratelimit-remaining");
            let reset = header("x-ratelimit-reset");
            if self.verbose {
                eprintln!(
                    "GitHub {}: {} remaining={}, reset={}",
                    url.path(),
                    status,
                    remaining.unwrap_or("unknown"),
                    reset.unwrap_or("unknown")
                );
            }
            let retry_after = header("retry-after").and_then(|seconds| seconds.parse::<u64>().ok());
            let exhausted = remaining == Some("0");
            let reset_delay = reset
                .and_then(|timestamp| timestamp.parse::<u64>().ok())
                .map(|timestamp| {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    timestamp.saturating_sub(now).saturating_add(1)
                });
            if status.is_success() {
                if exhausted {
                    self.pause(reset_delay.unwrap_or(60))?;
                }
                return response.json().context("could not decode GitHub response");
            }
            let message = response
                .json::<serde_json::Value>()
                .ok()
                .and_then(|body| body["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            last_message = format!("GitHub {status} for {}: {message}", url.path());
            let limited = status == StatusCode::TOO_MANY_REQUESTS
                || (status == StatusCode::FORBIDDEN
                    && (exhausted
                        || retry_after.is_some()
                        || message.to_ascii_lowercase().contains("rate limit")));
            if !limited {
                bail!("{last_message}");
            }
            let delay = retry_after
                .or(if exhausted { reset_delay } else { None })
                .unwrap_or(60 * (1_u64 << attempt))
                .max(1);
            self.pause(delay)?;
            if delay > 60 {
                bail!("{last_message}; retry in {delay} seconds");
            }
        }
        bail!("{last_message}")
    }

    fn pause(&self, seconds: u64) -> Result<()> {
        let until = Instant::now()
            .checked_add(Duration::from_secs(seconds))
            .context("GitHub returned an invalid rate limit delay")?;
        let mut pause = self.pause_until.lock().unwrap();
        *pause = (*pause).max(until);
        Ok(())
    }
}
