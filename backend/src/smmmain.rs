use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde_json::Value;
use std::sync::RwLock;

pub struct SmmMainService {
    /// Kalit va URL admin paneldan ishlayotgan paytda almashtiriladi, shuning
    /// uchun qulf ichida; har chaqiruv boshida nusxasi olinadi.
    creds: RwLock<SmmCreds>,
    service_id: u64,
    http: Client,
}

#[derive(Clone)]
struct SmmCreds {
    api_key: String,
    api_url: String,
}

#[derive(Clone, Debug)]
pub struct SmmOrderOutcome {
    pub order_id: Option<String>,
    pub raw_response: String,
}

#[derive(Clone, Debug)]
pub struct SmmBalanceOutcome {
    pub balance: Option<String>,
    pub currency: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SmmStatusOutcome {
    pub status: Option<String>,
}

impl SmmMainService {
    pub fn new(api_key: String, api_url: String, service_id: u64) -> Self {
        Self {
            creds: RwLock::new(SmmCreds { api_key, api_url }),
            service_id,
            // reqwest default'da umumiy timeout yo'q — javob kelmasa skaner
            // abadiy kutib qolmasligi uchun aniq chegara qo'yamiz.
            http: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    fn creds(&self) -> SmmCreds {
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
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = SmmCreds { api_key, api_url };
    }

    pub fn is_configured(&self) -> bool {
        !self.creds().api_key.trim().is_empty()
    }

    pub async fn send_order(
        &self,
        service_id: u64,
        link: &str,
        quantity: u64,
    ) -> Result<SmmOrderOutcome> {
        let creds = self.creds();
        if creds.api_key.trim().is_empty() {
            bail!("SMM API kaliti kiritilmagan (admin panelda sozlanadi)");
        }

        let service = if service_id == 0 {
            self.service_id
        } else {
            service_id
        }
        .to_string();
        let quantity = quantity.to_string();
        let form = [
            ("key", creds.api_key.trim()),
            ("action", "add"),
            ("service", service.as_str()),
            ("link", link.trim()),
            ("quantity", quantity.as_str()),
        ];

        let response = self
            .http
            .post(creds.api_url.trim())
            .form(&form)
            .send()
            .await
            .context("SMMMAIN API ga ulanishda xatolik")?;

        let status = response.status();
        let raw_response = response
            .text()
            .await
            .context("SMMMAIN javobini o'qib bo'lmadi")?;

        if !status.is_success() {
            bail!("SMMMAIN HTTP {status}: {raw_response}");
        }

        let value = serde_json::from_str::<Value>(&raw_response)
            .with_context(|| format!("SMMMAIN JSON javobi tushunarsiz: {raw_response}"))?;

        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(anyhow!("SMMMAIN xato qaytardi: {error}"));
        }

        // Muvaffaqiyatli javobda doim "order" id bo'ladi. Bo'lmasa, orderni
        // muvaffaqiyatli deb hisoblamaymiz (aks holda rad etilgan order ham
        // "yuborildi" deb belgilanib qolardi).
        let order_id = value.get("order").map(value_to_string);
        if order_id.is_none() {
            return Err(anyhow!(
                "SMMMAIN order id qaytarmadi, javob: {raw_response}"
            ));
        }

        Ok(SmmOrderOutcome {
            order_id,
            raw_response,
        })
    }

    /// Berilgan order id holatini smmmain.com `status` action orqali oladi.
    pub async fn order_status(&self, order_id: &str) -> Result<SmmStatusOutcome> {
        let creds = self.creds();
        if creds.api_key.trim().is_empty() {
            bail!("SMM API kaliti kiritilmagan (admin panelda sozlanadi)");
        }

        let form = [
            ("key", creds.api_key.trim()),
            ("action", "status"),
            ("order", order_id.trim()),
        ];

        let response = self
            .http
            .post(creds.api_url.trim())
            .form(&form)
            .send()
            .await
            .context("SMMMAIN status API ga ulanishda xatolik")?;

        let status = response.status();
        let raw_response = response
            .text()
            .await
            .context("SMMMAIN status javobini o'qib bo'lmadi")?;

        if !status.is_success() {
            bail!("SMMMAIN status HTTP {status}: {raw_response}");
        }

        let value = serde_json::from_str::<Value>(&raw_response)
            .with_context(|| format!("SMMMAIN status JSON javobi tushunarsiz: {raw_response}"))?;

        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(anyhow!("SMMMAIN status xato qaytardi: {error}"));
        }

        Ok(SmmStatusOutcome {
            status: value.get("status").map(value_to_string),
        })
    }

    pub async fn balance(&self) -> Result<SmmBalanceOutcome> {
        let creds = self.creds();
        if creds.api_key.trim().is_empty() {
            bail!("SMM API kaliti kiritilmagan (admin panelda sozlanadi)");
        }

        let form = [("key", creds.api_key.trim()), ("action", "balance")];
        let response = self
            .http
            .post(creds.api_url.trim())
            .form(&form)
            .send()
            .await
            .context("SMMMAIN balans API ga ulanishda xatolik")?;

        let status = response.status();
        let raw_response = response
            .text()
            .await
            .context("SMMMAIN balans javobini o'qib bo'lmadi")?;

        if !status.is_success() {
            bail!("SMMMAIN balans HTTP {status}: {raw_response}");
        }

        let value = serde_json::from_str::<Value>(&raw_response)
            .with_context(|| format!("SMMMAIN balans JSON javobi tushunarsiz: {raw_response}"))?;

        if let Some(error) = value.get("error").and_then(Value::as_str) {
            return Err(anyhow!("SMMMAIN balans xato qaytardi: {error}"));
        }

        Ok(SmmBalanceOutcome {
            balance: value.get("balance").map(value_to_string),
            currency: value.get("currency").map(value_to_string),
        })
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}
