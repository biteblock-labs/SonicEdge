use reqwest::Client;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, warn};

const TELEGRAM_QUEUE_CAPACITY: usize = 128;
const TELEGRAM_MESSAGE_LIMIT: usize = 4096;
const TELEGRAM_SEND_TIMEOUT_SECS: u64 = 5;

#[derive(Clone)]
pub struct TelegramNotifier {
    sender: mpsc::Sender<String>,
}

impl TelegramNotifier {
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .ok()
            .and_then(normalize_env);
        let chat_id = std::env::var("TELEGRAM_CHAT_ID")
            .ok()
            .and_then(normalize_env);

        match (token, chat_id) {
            (Some(token), Some(chat_id)) => Some(Self::new(token, chat_id)),
            (None, None) => None,
            _ => {
                warn!("telegram notifier disabled: TELEGRAM_BOT_TOKEN or TELEGRAM_CHAT_ID missing");
                None
            }
        }
    }

    pub fn new(token: String, chat_id: String) -> Self {
        let (sender, receiver) = mpsc::channel(TELEGRAM_QUEUE_CAPACITY);
        tokio::spawn(run_worker(token, chat_id, receiver));
        Self { sender }
    }

    pub fn notify(&self, message: String) {
        if message.is_empty() {
            return;
        }
        let message = truncate_message(message);
        match self.sender.try_send(message) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                debug!("telegram notifier queue full; dropping message");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("telegram notifier queue closed; dropping message");
            }
        }
    }
}

#[derive(Serialize)]
struct TelegramPayload<'a> {
    chat_id: &'a str,
    text: &'a str,
    disable_web_page_preview: bool,
}

async fn run_worker(token: String, chat_id: String, mut receiver: mpsc::Receiver<String>) {
    let client = Client::builder()
        .timeout(Duration::from_secs(TELEGRAM_SEND_TIMEOUT_SECS))
        .build()
        .unwrap_or_else(|err| {
            warn!(
                ?err,
                "telegram notifier client build failed; using default client"
            );
            Client::new()
        });
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");

    while let Some(message) = receiver.recv().await {
        let payload = TelegramPayload {
            chat_id: &chat_id,
            text: &message,
            disable_web_page_preview: true,
        };
        match client.post(&url).json(&payload).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    warn!(status = %response.status(), "telegram send failed");
                }
            }
            Err(err) => {
                warn!(?err, "telegram send failed");
            }
        }
    }
}

fn normalize_env(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn truncate_message(mut message: String) -> String {
    if message.len() <= TELEGRAM_MESSAGE_LIMIT {
        return message;
    }
    if TELEGRAM_MESSAGE_LIMIT <= 3 {
        message.truncate(TELEGRAM_MESSAGE_LIMIT);
        return message;
    }
    message.truncate(TELEGRAM_MESSAGE_LIMIT - 3);
    message.push_str("...");
    message
}
