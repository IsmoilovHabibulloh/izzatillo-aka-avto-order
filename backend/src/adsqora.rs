use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::Value;

/// adsqora.vipads.uz — markaziy qora kanal bazasi mijozi.
/// Panel shu servis orqali qora kanal qo'shadi/tekshiradi (API kalit backendda qoladi).
#[derive(Clone)]
pub struct AdsQoraService {
    api_key: String,
    api_url: String,
    http: Client,
}

#[derive(Clone, Debug)]
pub struct AdsQoraOutcome {
    pub status: u16,
    pub body: Value,
}

impl AdsQoraService {
    pub fn new(api_key: String, api_url: String) -> Self {
        Self {
            api_key,
            api_url,
            http: Client::new(),
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    fn parse(status: u16, raw: &str) -> AdsQoraOutcome {
        let body = serde_json::from_str(raw)
            .unwrap_or_else(|_| Value::String(raw.trim().to_string()));
        AdsQoraOutcome { status, body }
    }

    /// POST /channels — kanal qo'shish. Takror qo'shish xato emas (200, created: false).
    pub async fn add_channel(&self, link: &str) -> Result<AdsQoraOutcome> {
        if !self.is_configured() {
            bail!("ADSQORA_API_KEY .env ichida kiritilmagan");
        }
        let url = format!("{}/channels", self.api_url.trim_end_matches('/'));
        let response = self
            .http
            .post(url)
            .header("X-API-Key", self.api_key.trim())
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
        if !self.is_configured() {
            bail!("ADSQORA_API_KEY .env ichida kiritilmagan");
        }
        let url = format!("{}/channels/check", self.api_url.trim_end_matches('/'));
        let response = self
            .http
            .get(url)
            .query(&[("link", link.trim())])
            .header("X-API-Key", self.api_key.trim())
            .send()
            .await
            .context("adsqora API ga ulanishda xatolik")?;
        let status = response.status().as_u16();
        let raw = response.text().await.context("adsqora javobini o'qib bo'lmadi")?;
        Ok(Self::parse(status, &raw))
    }
}
