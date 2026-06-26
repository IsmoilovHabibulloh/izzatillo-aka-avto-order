use anyhow::{Context, Result, anyhow, bail};
use grammers_client::{Client, SignInError};
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use grammers_tl_types as tl;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::models::{AdResult, TelegramAuthResponse, TelegramSettings};

pub struct TelegramService {
    session_path: PathBuf,
    active: Mutex<Option<ActiveClient>>,
    pending: Mutex<Option<PendingLogin>>,
}

struct ActiveClient {
    client: Client,
    runner: JoinHandle<()>,
}

enum PendingStep {
    Code(grammers_client::client::LoginToken),
    Password(grammers_client::client::PasswordToken),
}

struct PendingLogin {
    client: Client,
    runner: JoinHandle<()>,
    step: PendingStep,
}

impl TelegramService {
    pub fn new(session_path: impl AsRef<Path>) -> Self {
        Self {
            session_path: session_path.as_ref().to_path_buf(),
            active: Mutex::new(None),
            pending: Mutex::new(None),
        }
    }

    pub async fn is_connected(&self) -> bool {
        let active = self.active.lock().await;
        if let Some(active) = active.as_ref() {
            active.client.is_authorized().await.unwrap_or(false)
        } else {
            false
        }
    }

    pub async fn waiting_for(&self) -> Option<String> {
        let pending = self.pending.lock().await;
        pending.as_ref().map(|pending| match pending.step {
            PendingStep::Code(_) => "code".to_string(),
            PendingStep::Password(_) => "password".to_string(),
        })
    }

    pub async fn disconnect(&self) {
        if let Some(active) = self.active.lock().await.take() {
            active.runner.abort();
        }
        if let Some(pending) = self.pending.lock().await.take() {
            pending.runner.abort();
        }
    }

    pub async fn request_code(
        &self,
        api_id: i32,
        api_hash: String,
        phone: String,
    ) -> Result<TelegramAuthResponse> {
        let (client, runner) = self.connect(api_id).await?;

        if client.is_authorized().await? {
            self.set_active(client, runner).await;
            return Ok(TelegramAuthResponse {
                connected: true,
                waiting_for: None,
                message: "Userbot allaqachon ulangan".to_string(),
            });
        }

        let token = client
            .request_login_code(&phone, &api_hash)
            .await
            .context("Telegram login kodi so'rovida xatolik")?;

        if let Some(old) = self.pending.lock().await.replace(PendingLogin {
            client,
            runner,
            step: PendingStep::Code(token),
        }) {
            old.runner.abort();
        }

        Ok(TelegramAuthResponse {
            connected: false,
            waiting_for: Some("code".to_string()),
            message: "Kod yuborildi".to_string(),
        })
    }

    pub async fn verify_code(
        &self,
        code: Option<String>,
        password: Option<String>,
    ) -> Result<TelegramAuthResponse> {
        let pending = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(|| anyhow!("Avval login kodini so'rang"))?;

        match pending.step {
            PendingStep::Code(token) => {
                let code = code
                    .as_deref()
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .ok_or_else(|| anyhow!("Telegram kodi kerak"))?;

                match pending.client.sign_in(&token, code).await {
                    Ok(_) => {
                        self.set_active(pending.client, pending.runner).await;
                        Ok(TelegramAuthResponse {
                            connected: true,
                            waiting_for: None,
                            message: "Userbot ulandi".to_string(),
                        })
                    }
                    Err(SignInError::PasswordRequired(token)) => {
                        let hint = token.hint().map(|x| format!(" ({x})")).unwrap_or_default();
                        self.restore_pending(PendingLogin {
                            step: PendingStep::Password(token),
                            ..pending
                        })
                        .await;
                        Ok(TelegramAuthResponse {
                            connected: false,
                            waiting_for: Some("password".to_string()),
                            message: format!("2FA parol kerak{hint}"),
                        })
                    }
                    Err(err) => Err(anyhow!(err).context("Telegram kodini tasdiqlab bo'lmadi")),
                }
            }
            PendingStep::Password(token) => {
                let password = password
                    .as_deref()
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .ok_or_else(|| anyhow!("2FA parol kerak"))?;

                match pending.client.check_password(token, password).await {
                    Ok(_) => {
                        self.set_active(pending.client, pending.runner).await;
                        Ok(TelegramAuthResponse {
                            connected: true,
                            waiting_for: None,
                            message: "Userbot ulandi".to_string(),
                        })
                    }
                    Err(SignInError::InvalidPassword(token)) => {
                        self.restore_pending(PendingLogin {
                            step: PendingStep::Password(token),
                            ..pending
                        })
                        .await;
                        Err(anyhow!("2FA parol noto'g'ri"))
                    }
                    Err(err) => Err(anyhow!(err).context("2FA parolni tasdiqlab bo'lmadi")),
                }
            }
        }
    }

    pub async fn ensure_client(&self, settings: &TelegramSettings) -> Result<Client> {
        {
            let active = self.active.lock().await;
            if let Some(active) = active.as_ref() {
                if active.client.is_authorized().await.unwrap_or(false) {
                    return Ok(active.client.clone());
                }
            }
        }

        let api_id = settings
            .api_id
            .ok_or_else(|| anyhow!("Telegram API ID kiritilmagan"))?;
        let (client, runner) = self.connect(api_id).await?;
        if !client.is_authorized().await? {
            runner.abort();
            bail!("Userbot ulanmagan. Admin paneldan Telegram login qiling");
        }

        self.set_active(client.clone(), runner).await;
        Ok(client)
    }

    /// Berilgan `query` (key) bo'yicha Telegram'da GLOBAL sponsored qidiruv qiladi
    /// (`contacts.getSponsoredPeers`). Natija — qidiruvga mos sponsored kanallar.
    /// Har bir topilgan kanal `AdResult` ko'rinishida qaytariladi: target_channel —
    /// topilgan kanal, matched_keywords — qidirilgan key.
    pub async fn get_sponsored_peers(&self, client: &Client, query: &str) -> Result<Vec<AdResult>> {
        let query_trimmed = query.trim();
        if query_trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let response = client
            .invoke(&tl::functions::contacts::GetSponsoredPeers {
                q: query_trimmed.to_string(),
            })
            .await
            .with_context(|| format!("Telegram sponsored qidiruv xatosi: {query_trimmed}"))?;

        let data = match response {
            tl::enums::contacts::SponsoredPeers::Peers(data) => data,
            tl::enums::contacts::SponsoredPeers::Empty => return Ok(Vec::new()),
        };

        let query_lc = query_trimmed.to_lowercase();
        let mut out = Vec::new();

        for peer in data.peers {
            let tl::enums::SponsoredPeer::Peer(peer) = peer;
            let Some((username, title)) = resolve_peer(&peer.peer, &data.chats, &data.users) else {
                // username yo'q (link yasab bo'lmaydi) — o'tkazib yuboramiz.
                continue;
            };

            let username_lc = username.to_lowercase();
            let url = format!("https://t.me/{username}");
            let random_id_hex = to_hex(&peer.random_id);
            // Barqaror fingerprint: bir xil key bir xil kanalni topsa, natija takrorlanmaydi.
            let fingerprint = format!("{query_lc}:{username_lc}");

            out.push(AdResult {
                id: uuid::Uuid::new_v4().to_string(),
                fingerprint,
                channel: username_lc.clone(),
                channel_title: title.clone(),
                target_channel: Some(username_lc),
                matched_keywords: vec![query_trimmed.to_string()],
                title: title.unwrap_or_default(),
                message: peer.additional_info.clone().unwrap_or_default(),
                url,
                button_text: String::new(),
                sponsor_info: peer.sponsor_info,
                additional_info: peer.additional_info,
                recommended: false,
                random_id_hex,
                found_at: chrono::Utc::now(),
            });
        }

        Ok(out)
    }

    async fn connect(&self, api_id: i32) -> Result<(Client, JoinHandle<()>)> {
        if let Some(parent) = self.session_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let session_path = self
            .session_path
            .to_str()
            .ok_or_else(|| anyhow!("Session path UTF-8 emas"))?;
        let session = Arc::new(SqliteSession::open(session_path).await?);
        let SenderPool { runner, handle, .. } = SenderPool::new(Arc::clone(&session), api_id);
        let client = Client::new(handle);
        let runner = tokio::spawn(runner.run());
        Ok((client, runner))
    }

    async fn set_active(&self, client: Client, runner: JoinHandle<()>) {
        if let Some(old) = self
            .active
            .lock()
            .await
            .replace(ActiveClient { client, runner })
        {
            old.runner.abort();
        }
    }

    async fn restore_pending(&self, pending: PendingLogin) {
        if let Some(old) = self.pending.lock().await.replace(pending) {
            old.runner.abort();
        }
    }
}

/// Sponsored peer (Peer) ni topilgan chats/users ro'yxati orqali (username, title)
/// ga aylantiradi. Username topilmasa None — chunki order linkini yasab bo'lmaydi.
fn resolve_peer(
    peer: &tl::enums::Peer,
    chats: &[tl::enums::Chat],
    users: &[tl::enums::User],
) -> Option<(String, Option<String>)> {
    match peer {
        tl::enums::Peer::Channel(p) => {
            for chat in chats {
                if let tl::enums::Chat::Channel(c) = chat {
                    if c.id == p.channel_id {
                        return primary_username(c.username.as_deref(), c.usernames.as_deref())
                            .map(|name| (name, Some(c.title.clone())));
                    }
                }
            }
            None
        }
        tl::enums::Peer::User(p) => {
            for user in users {
                if let tl::enums::User::User(u) = user {
                    if u.id == p.user_id {
                        return primary_username(u.username.as_deref(), u.usernames.as_deref())
                            .map(|name| (name, u.first_name.clone()));
                    }
                }
            }
            None
        }
        tl::enums::Peer::Chat(_) => None,
    }
}

/// Asosiy username (yoki birinchi aktiv qo'shimcha username) ni qaytaradi.
fn primary_username(
    username: Option<&str>,
    usernames: Option<&[tl::enums::Username]>,
) -> Option<String> {
    if let Some(value) = username {
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    if let Some(list) = usernames {
        for entry in list {
            let tl::enums::Username::Username(entry) = entry;
            if entry.active && !entry.username.is_empty() {
                return Some(entry.username.clone());
            }
        }
    }
    None
}

pub fn normalize_channel_ref(raw: &str) -> Option<String> {
    let mut value = raw.trim().trim_start_matches('@').trim().to_string();
    for prefix in [
        "https://t.me/",
        "http://t.me/",
        "t.me/",
        "https://telegram.me/",
        "telegram.me/",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.to_string();
        }
    }
    value = value
        .split(['?', '/', '#'])
        .next()
        .unwrap_or_default()
        .trim_start_matches('@')
        .to_string();

    if value.is_empty() {
        None
    } else {
        Some(value.to_lowercase())
    }
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
