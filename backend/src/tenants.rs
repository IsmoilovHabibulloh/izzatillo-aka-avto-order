//! Panel foydalanuvchilari (tenantlar) ro'yxati — `data/tenants.json`.
//!
//! Foydalanuvchilar admin paneldan qo'shiladi, tahrirlanadi va o'chiriladi;
//! o'zgarishlar restartsiz kuchga kiradi. Eski `.env` (`TENANTS=...`,
//! `TENANT_<NOMI>_*`) faqat `tenants.json` hali yo'q bo'lgan birinchi ishga
//! tushishda bir martalik import uchun o'qiladi.

use crate::adsqora::AdsQoraService;
use crate::api::{RuntimeInfo, TenantState};
use crate::models::{DEFAULT_SMMMAIN_SERVICE_ID, TelegramAccount};
use crate::scanner;
use crate::smmmain::SmmMainService;
use crate::store::Store;
use crate::telegram::TelegramService;
use anyhow::{Context, Result, anyhow};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;

pub const DEFAULT_SMM_API_URL: &str = "https://smm.vipads.uz/api/v2";
pub const DEFAULT_ADSQORA_API_URL: &str = "https://adsqora.vipads.uz/api";
pub const DEFAULT_MAINTENANCE_MESSAGE: &str =
    "Serverda profilaktika ishlari olib borilmoqda. Tez orada panel yana ishga tushadi.";

/// Bitta foydalanuvchining doimiy yozuvi (`tenants.json` ichida).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TenantRecord {
    /// O'zgarmas ID (kichik lotin harflari, raqam, `-`, `_`).
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    /// Kirish logini (katta-kichik harf farqlanmaydi).
    pub username: String,
    /// Argon2 xesh — parolning o'zi saqlanmaydi.
    pub password_hash: String,
    /// Foydalanuvchi bazasi (sozlamalar, natijalar, loglar, akkauntlar).
    pub state_path: String,
    /// Userbot sessiya fayllari papkasi.
    pub session_dir: String,
    #[serde(default)]
    pub smm_api_key: String,
    #[serde(default = "default_smm_api_url")]
    pub smm_api_url: String,
    #[serde(default)]
    pub adsqora_api_key: String,
    #[serde(default = "default_adsqora_api_url")]
    pub adsqora_api_url: String,
    /// "Kanal tayyorlash" tabi ochadigan havola.
    #[serde(default)]
    pub userbot_url: String,
    /// Profilaktika: panel yopiq, skaner va orderlar pauzada.
    #[serde(default)]
    pub maintenance: bool,
    #[serde(default)]
    pub maintenance_message: String,
    #[serde(default)]
    pub maintenance_since: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TenantRecord {
    /// Foydalanuvchiga ko'rsatiladigan profilaktika matni.
    pub fn maintenance_text(&self) -> String {
        let text = self.maintenance_message.trim();
        if text.is_empty() {
            DEFAULT_MAINTENANCE_MESSAGE.to_string()
        } else {
            text.to_string()
        }
    }

    pub fn display(&self) -> String {
        let name = self.display_name.trim();
        if name.is_empty() {
            self.username.clone()
        } else {
            name.to_string()
        }
    }
}

fn default_smm_api_url() -> String {
    DEFAULT_SMM_API_URL.to_string()
}

fn default_adsqora_api_url() -> String {
    DEFAULT_ADSQORA_API_URL.to_string()
}

#[derive(Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    tenants: Vec<TenantRecord>,
}

/// Yangi foydalanuvchilar uchun serverdagi (env) standart qiymatlar.
#[derive(Clone, Debug)]
pub struct TenantDefaults {
    pub smm_api_url: String,
    pub adsqora_api_url: String,
    pub telegram_api_id: Option<i32>,
    pub telegram_api_hash: Option<String>,
    pub telegram_phone: Option<String>,
}

pub fn defaults_from_env() -> TenantDefaults {
    TenantDefaults {
        smm_api_url: env_value("SMMMAIN_API_URL").unwrap_or_else(default_smm_api_url),
        adsqora_api_url: env_value("ADSQORA_API_URL").unwrap_or_else(default_adsqora_api_url),
        telegram_api_id: env_value("TELEGRAM_API_ID").and_then(|value| value.parse::<i32>().ok()),
        telegram_api_hash: env_value("TELEGRAM_API_HASH"),
        telegram_phone: env_value("TELEGRAM_PHONE"),
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `tenants.json`ni o'qiydi. Fayl hali yo'q bo'lsa, `.env`dan import qilib
/// yaratadi (bir martalik migratsiya). Ikkinchi qiymat — import bo'ldimi.
///
/// Import bo'lgach yonida `.imported` belgisi qoladi: keyin fayl yo'qolsa,
/// eski `.env` jimgina qayta import qilinmaydi (o'chirilgan foydalanuvchilar
/// va eski parollar qaytib kelmasin) — server xato bilan to'xtaydi.
pub async fn load_or_import(path: &Path) -> Result<(Vec<TenantRecord>, bool)> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => {
            let file: RegistryFile = serde_json::from_str(&raw)
                .with_context(|| format!("tenants JSON buzilgan: {}", path.display()))?;
            Ok((file.tenants, false))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let marker = path.with_extension("json.imported");
            if tokio::fs::try_exists(&marker).await.unwrap_or(false) {
                anyhow::bail!(
                    "{} topilmadi, lekin .env avval import qilingan ({}). Faylni {}.bak dan tiklang",
                    path.display(),
                    marker.display(),
                    path.display()
                );
            }
            let records = import_from_env(Utc::now())?;
            write_registry(path, &records).await?;
            tokio::fs::write(&marker, Utc::now().to_rfc3339()).await?;
            Ok((records, true))
        }
        Err(err) => Err(err).with_context(|| format!("tenants o'qilmadi: {}", path.display())),
    }
}

/// Ishlayotgan tenantlar konfiguratsiyasini `tenants.json`ga yozadi.
pub async fn save_registry(path: &Path, tenants: &[TenantState]) -> Result<()> {
    let mut records = Vec::with_capacity(tenants.len());
    for tenant in tenants {
        records.push(tenant.config.read().await.clone());
    }
    write_registry(path, &records).await
}

async fn write_registry(path: &Path, records: &[TenantRecord]) -> Result<()> {
    let raw = serde_json::to_vec_pretty(&RegistryFile {
        tenants: records.to_vec(),
    })?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Faylda parol xeshlari va API kalitlar bor — faqat egasi o'qiy oladi (0600).
    let tmp = path.with_extension("json.tmp");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&tmp)
        .await
        .with_context(|| format!("tenants yozilmadi: {}", tmp.display()))?;
    file.write_all(&raw).await?;
    file.sync_all().await?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).await?;
    }
    // Oldingi nusxa `.bak` bo'lib qoladi — tasodifan buzilsa tiklash oson.
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        let backup = path.with_extension("json.bak");
        let _ = tokio::fs::copy(path, &backup).await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                tokio::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o600)).await;
        }
    }
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("tenants almashtirilmadi: {}", path.display()))?;
    Ok(())
}

/// Eski `.env` sozlamalaridan yozuvlar yasaydi — avvalgi `build_tenant` bilan
/// aynan bir xil yo'llar va kalitlar (ma'lumotlar joyidan qo'zg'almaydi).
fn import_from_env(now: DateTime<Utc>) -> Result<Vec<TenantRecord>> {
    let names: Vec<String> = env::var("TENANTS")
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect();

    let smm_url = env_value("SMMMAIN_API_URL").unwrap_or_else(default_smm_api_url);
    let adsqora_url = env_value("ADSQORA_API_URL").unwrap_or_else(default_adsqora_api_url);

    let mut records = Vec::new();
    if names.is_empty() {
        // Eski bitta-admin rejimi.
        let username = env_value("ADMIN_USERNAME").unwrap_or_else(|| "Izzatillo".to_string());
        let password = env_value("ADMIN_PASSWORD").unwrap_or_else(|| "Izzatilloaka".to_string());
        let session_path = env_value("TELEGRAM_SESSION_PATH")
            .unwrap_or_else(|| "data/userbot.session".to_string());
        let id = unique_id(&slugify(&username), &records);
        records.push(TenantRecord {
            display_name: username.clone(),
            password_hash: hash_password(&password)?,
            username,
            state_path: env_value("STATE_PATH").unwrap_or_else(|| "data/state.json".to_string()),
            session_dir: parent_dir(&session_path),
            smm_api_key: env_value("SMMMAIN_API_KEY").unwrap_or_default(),
            smm_api_url: smm_url.clone(),
            adsqora_api_key: env_value("ADSQORA_API_KEY").unwrap_or_default(),
            adsqora_api_url: adsqora_url.clone(),
            userbot_url: String::new(),
            maintenance: false,
            maintenance_message: String::new(),
            maintenance_since: None,
            created_at: now,
            updated_at: now,
            id,
        });
        return Ok(records);
    }

    for name in names {
        let upper = name.to_uppercase();
        let tenant_var = |suffix: &str| env_value(&format!("TENANT_{upper}_{suffix}"));
        let username = tenant_var("USERNAME").unwrap_or_else(|| name.clone());
        let password_hash = match tenant_var("PASSWORD") {
            Some(password) => hash_password(&password)?,
            None => {
                // Bo'sh parol bilan kirib bo'lmasin: admin yangi parol qo'ygunicha yopiq.
                tracing::warn!(tenant = %name, "tenant paroli yo'q — admin paneldan parol qo'ying");
                hash_password(&uuid::Uuid::new_v4().to_string())?
            }
        };
        let session_path =
            tenant_var("SESSION_PATH").unwrap_or_else(|| format!("data/{name}/userbot.session"));
        let id = unique_id(&slugify(&name), &records);
        records.push(TenantRecord {
            display_name: username.clone(),
            username,
            password_hash,
            state_path: tenant_var("STATE_PATH")
                .unwrap_or_else(|| format!("data/state-{name}.json")),
            session_dir: parent_dir(&session_path),
            smm_api_key: tenant_var("SMMMAIN_API_KEY").unwrap_or_default(),
            smm_api_url: tenant_var("SMMMAIN_API_URL").unwrap_or_else(|| smm_url.clone()),
            adsqora_api_key: tenant_var("ADSQORA_API_KEY").unwrap_or_default(),
            adsqora_api_url: tenant_var("ADSQORA_API_URL").unwrap_or_else(|| adsqora_url.clone()),
            userbot_url: tenant_var("USERBOT_URL").unwrap_or_default(),
            maintenance: false,
            maintenance_message: String::new(),
            maintenance_since: None,
            created_at: now,
            updated_at: now,
            id,
        });
    }
    Ok(records)
}

/// Sessiya faylining papkasi. Papkasiz nom (`userbot.session`) — joriy papka,
/// avvalgi `TelegramService::new` bilan bir xil.
fn parent_dir(session_path: &str) -> String {
    Path::new(session_path)
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .filter(|parent| !parent.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

/// Yozuvdan ishlaydigan tenantni quradi: bazani ochadi (yo'q bo'lsa yaratadi),
/// sessiya papkasini tayyorlaydi va servislarni ulaydi.
pub async fn build_tenant(record: TenantRecord, defaults: &TenantDefaults) -> Result<TenantState> {
    let store = Arc::new(Store::load(&record.state_path).await?);
    let session_dir = PathBuf::from(&record.session_dir);
    tokio::fs::create_dir_all(&session_dir)
        .await
        .with_context(|| format!("sessiya papkasi yaratilmadi: {}", session_dir.display()))?;
    let legacy_session = session_dir.join("userbot.session");
    seed_telegram_defaults(&store, defaults).await?;
    migrate_legacy_session(&store, &legacy_session).await?;

    Ok(TenantState {
        id: record.id.clone(),
        store,
        telegram: Arc::new(TelegramService::new(&legacy_session)),
        smmmain: Arc::new(SmmMainService::new(
            record.smm_api_key.clone(),
            record.smm_api_url.clone(),
            DEFAULT_SMMMAIN_SERVICE_ID,
        )),
        adsqora: Arc::new(AdsQoraService::new(
            record.adsqora_api_key.clone(),
            record.adsqora_api_url.clone(),
        )),
        runtime: Arc::new(RwLock::new(RuntimeInfo::default())),
        rr: Arc::new(AtomicUsize::new(0)),
        config: Arc::new(RwLock::new(record)),
        stopped: Arc::new(AtomicBool::new(false)),
    })
}

/// Tenantning fon ishlarini boshlaydi: akkauntlarni ulab qo'yish va skaner sikli.
pub fn spawn_tenant_tasks(tenant: &TenantState) {
    let warm = tenant.clone();
    tokio::spawn(async move {
        if let Some(api_id) = warm.store.telegram_settings().await.api_id {
            for account in warm.store.accounts().await {
                if warm.is_stopped() {
                    break;
                }
                let _ = warm
                    .telegram
                    .ensure_account_client(&account.id, api_id)
                    .await;
            }
        }
    });
    tokio::spawn(scanner::scanner_loop(tenant.clone()));
}

/// O'chirilgan tenant bazasini va shaxsiy sessiya papkasini `.deleted-<vaqt>`
/// nomiga o'tkazadi (hech narsa butunlay o'chmaydi). Umumiy `data/` papkasi
/// (eski izzatillo sxemasi) joyida qoladi.
pub async fn archive_tenant_files(record: &TenantRecord, in_use: &[String]) -> Vec<String> {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let mut moved = Vec::new();

    let state = Path::new(&record.state_path);
    let shared = |path: &str| in_use.iter().any(|used| used == path);
    if !shared(&record.state_path) && tokio::fs::try_exists(state).await.unwrap_or(false) {
        let target = format!("{}.deleted-{stamp}", record.state_path);
        match tokio::fs::rename(state, &target).await {
            Ok(()) => moved.push(target),
            Err(err) => tracing::warn!(error = %err, "tenant bazasi arxivlanmadi"),
        }
    }

    let dir = Path::new(&record.session_dir);
    let dedicated = dir.file_name().and_then(|name| name.to_str()) == Some(record.id.as_str())
        && !shared(&record.session_dir);
    if dedicated && tokio::fs::try_exists(dir).await.unwrap_or(false) {
        let target = format!(
            "{}.deleted-{stamp}",
            record.session_dir.trim_end_matches('/')
        );
        match tokio::fs::rename(dir, &target).await {
            Ok(()) => moved.push(target),
            Err(err) => tracing::warn!(error = %err, "tenant sessiya papkasi arxivlanmadi"),
        }
    }
    moved
}

/// Serverdagi umumiy Telegram API sozlamalarini faqat bo'sh joyga yozadi —
/// admin paneldan foydalanuvchiga alohida qo'yilgan qiymat ustidan yozilmaydi.
async fn seed_telegram_defaults(store: &Store, defaults: &TenantDefaults) -> Result<()> {
    let mut settings = store.telegram_settings().await;
    let mut changed = false;
    if settings.api_id.is_none()
        && let Some(api_id) = defaults.telegram_api_id
    {
        settings.api_id = Some(api_id);
        changed = true;
    }
    let hash_missing = settings
        .api_hash
        .as_deref()
        .map(|hash| hash.trim().is_empty())
        .unwrap_or(true);
    if hash_missing && let Some(api_hash) = &defaults.telegram_api_hash {
        settings.api_hash = Some(api_hash.clone());
        changed = true;
    }
    if settings.phone.is_none()
        && let Some(phone) = &defaults.telegram_phone
    {
        settings.phone = Some(phone.clone());
        changed = true;
    }
    if changed {
        store.update_telegram(settings).await?;
    }
    Ok(())
}

/// Eski (bitta) userbot sessiyasini ko'p-akkaunt ro'yxatidagi birinchi akkauntga
/// ko'chiradi. Faqat akkauntlar bo'sh bo'lsa va eski sessiya fayli mavjud bo'lsa ishlaydi.
async fn migrate_legacy_session(store: &Store, legacy_session: &Path) -> Result<()> {
    if !store.accounts().await.is_empty() {
        return Ok(());
    }
    if !legacy_session.exists() {
        return Ok(());
    }
    let settings = store.telegram_settings().await;
    if settings.api_id.is_none() {
        return Ok(());
    }

    let id = uuid::Uuid::new_v4().to_string();
    let dir = legacy_session.parent().unwrap_or_else(|| Path::new("data"));
    let target = dir.join(format!("userbot-{id}.session"));
    let legacy_str = legacy_session.to_string_lossy().to_string();
    let target_str = target.to_string_lossy().to_string();
    for suffix in ["", "-wal", "-shm"] {
        let from = format!("{legacy_str}{suffix}");
        let to = format!("{target_str}{suffix}");
        if Path::new(&from).exists() {
            let _ = tokio::fs::rename(&from, &to).await;
        }
    }

    let label = settings
        .phone
        .clone()
        .unwrap_or_else(|| "Akkaunt 1".to_string());
    store
        .add_account(TelegramAccount {
            id,
            label: Some(label),
            username: None,
            created_at: Utc::now(),
            last_used_at: None,
            flood_until: None,
        })
        .await?;
    tracing::info!("eski userbot sessiyasi yangi akkauntga ko'chirildi");
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String> {
    // Tuz (salt) uchun UUID v4 ning 16 tasodifiy bayti yetarli.
    let salt = SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes())
        .map_err(|err| anyhow!("salt yaratilmadi: {err}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|err| anyhow!("parol xeshlanmadi: {err}"))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Argon2 CPU'ni band qiladi — async runtime'ni to'sib qo'ymaslik uchun alohida thread'da.
pub async fn hash_password_async(password: String) -> Result<String> {
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|err| anyhow!("parol xeshlashda ichki xato: {err}"))?
}

pub async fn verify_password_async(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false)
}

/// Login yoki ismdan ID yasaydi: `Feruz aka` → `feruz-aka`.
pub fn slugify(input: &str) -> String {
    let mut out = String::new();
    for ch in input.trim().to_lowercase().chars() {
        match ch {
            'a'..='z' | '0'..='9' | '_' => out.push(ch),
            // O'zbekcha apostroflar (o', g') shunchaki tashlab ketiladi.
            '\'' | '`' | 'ʻ' | 'ʼ' | '‘' | '’' => {}
            _ => {
                if !out.is_empty() && !out.ends_with('-') {
                    out.push('-');
                }
            }
        }
    }
    let trimmed: String = out.trim_matches(['-', '_']).chars().take(32).collect();
    let trimmed = trimmed.trim_end_matches(['-', '_']).to_string();
    if trimmed.is_empty() {
        "user".to_string()
    } else {
        trimmed
    }
}

pub fn is_valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    id.len() <= 32
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

fn unique_id(base: &str, records: &[TenantRecord]) -> String {
    let taken = |candidate: &str| records.iter().any(|record| record.id == candidate);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}
