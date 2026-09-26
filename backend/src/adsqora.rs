use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;
use std::sync::RwLock;

/// adsqora.vipads.uz — markaziy qora kanal bazasi mijozi.
/// Panel shu servis orqali qora kanal qo'shadi/tekshiradi (API kalit backendda qoladi).
pub struct AdsQoraService {
    /// Kalit va URL admin paneldan ishlayotgan paytda almashtiriladi.
    creds: RwLock<AdsQoraCreds>,
    http: Client,
}

#[derive(Clone)]
struct AdsQoraCreds {
    api_key: String,
    api_url: String,
}

#[derive(Clone, Debug)]
pub struct AdsQoraOutcome {
    pub status: u16,
    pub body: Value,
}

impl AdsQoraService {
    pub fn new(api_key: String, api_url: String) -> Self {
        Self {
            creds: RwLock::new(AdsQoraCreds { api_key, api_url }),
            // reqwest default'da umumiy timeout yo'q — javob kelmasa handler
            // abadiy kutib qolmasligi uchun aniq chegara qo'yamiz.
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn creds(&self) -> AdsQoraCreds {
        self.creds
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Admin paneldan kalit/URL o'zgarganda chaqiriladi (restart shart emas).
    pub fn update(&self, api_key: String, api_url: String) {
        *self
            .creds
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = AdsQoraCreds { api_key, api_url };
    }

    fn parse(status: u16, raw: &str) -> AdsQoraOutcome {
        let body = serde_json::from_str(raw)
            .unwrap_or_else(|_| Value::String(raw.trim().to_string()));
        AdsQoraOutcome { status, body }
    }

    /// POST /channels — kanal qo'shish. Takror qo'shish xato emas (200, created: false).
    pub async fn add_channel(&self, link: &str) -> Result<AdsQoraOutcome> {
        let creds = self.creds();
        if creds.api_key.trim().is_empty() {
            bail!("Adsqora API kaliti kiritilmagan (admin panelda sozlanadi)");
        }
        let url = format!("{}/channels", creds.api_url.trim().trim_end_matches('/'));
        let response = self
            .http
            .post(url)
            .header("X-API-Key", creds.api_key.trim())
            .json(&serde_json::json!({ "link": link.trim() }))
            .send()
            .await
            .context("adsqora API ga ulanishda xatolik")?;
        let status = response.status().as_u16();
        let raw = response.text().await.context("adsqora javobini o'qib bo'lmadi")?;
        Ok(Self::parse(status, &raw))
    }

    /// GET /channels/check?link=... — kanal bazada bormi.
    pub async fn check_channel(&self, link: &str) -> Result<AdsQoraOutcome> {
        let creds = self.creds();
        if creds.api_key.trim().is_empty() {
            bail!("Adsqora API kaliti kiritilmagan (admin panelda sozlanadi)");
        }
        let url = format!("{}/channels/check", creds.api_url.trim().trim_end_matches('/'));
        let response = self
            .http
            .get(url)
            .query(&[("link", link.trim())])
            .header("X-API-Key", creds.api_key.trim())
            .send()
            .await
            .context("adsqora API ga ulanishda xatolik")?;
        let status = response.status().as_u16();
        let raw = response.text().await.context("adsqora javobini o'qib bo'lmadi")?;
        Ok(Self::parse(status, &raw))
    }
}
