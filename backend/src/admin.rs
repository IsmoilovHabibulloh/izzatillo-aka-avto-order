//! Bosh admin paneli API (`/api/admin/*`): foydalanuvchilarni qo'shish,
//! tahrirlash, o'chirish, kalit/havolalarni sozlash va profilaktika rejimi.
//! Barcha o'zgarishlar restartsiz kuchga kiradi va `tenants.json`ga yoziladi.

use crate::api::{ApiError, AppState, Session, TenantState, new_session, require_session};
use crate::models::PanelLog;
use crate::tenants::{self, TenantRecord};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;

const MAX_MAINTENANCE_MESSAGE: usize = 500;

/// Bosh admin login/paroli (env: SUPERADMIN_USERNAME / SUPERADMIN_PASSWORD).
pub struct AdminCredentials {
    pub username: String,
    password: String,
}

impl AdminCredentials {
    pub fn from_env() -> Option<Self> {
        let read = |name: &str| {
            env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Some(Self {
            username: read("SUPERADMIN_USERNAME")?,
            password: read("SUPERADMIN_PASSWORD")?,
        })
    }

    pub fn matches(&self, username: &str, password: &str) -> bool {
        self.username.eq_ignore_ascii_case(username.trim())
            && constant_time_eq(self.password.as_bytes(), password.as_bytes())
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tenants", get(list_tenants).post(create_tenant))
        .route("/tenants/maintenance", post(set_maintenance))
        .route("/tenants/{id}", put(update_tenant).delete(delete_tenant))
        .route("/tenants/{id}/login", post(open_tenant_panel))
        .route("/tenants/{id}/check", post(check_tenant))
}

async fn require_admin(headers: &HeaderMap, state: &AppState) -> Result<(), ApiError> {
    match require_session(headers, state).await? {
        Session::Admin => Ok(()),
        Session::Tenant { .. } => Err(ApiError::forbidden("Bu bo'lim faqat admin uchun")),
    }
}

#[derive(Serialize)]
struct AdminTenantView {
    id: String,
    display_name: String,
    username: String,
    smm_api_key: String,
    smm_api_url: String,
    adsqora_api_key: String,
    adsqora_api_url: String,
    userbot_url: String,
    telegram_api_id: Option<i32>,
    telegram_api_hash: Option<String>,
    maintenance: bool,
    maintenance_message: String,
    maintenance_since: Option<DateTime<Utc>>,
    state_path: String,
    session_dir: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    scanner_enabled: bool,
    scanning: bool,
    last_run_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    keywords_total: usize,
    keywords_enabled: usize,
    accounts_total: usize,
    accounts_flooded: usize,
    results_total: usize,
    logs_total: usize,
    /// Foydalanuvchining o'zi ochgan sessiyalar soni (admin sessiyalari hisoblanmaydi).
    active_sessions: usize,
}

#[derive(Serialize)]
struct AdminDefaultsView {
    smm_api_url: String,
    adsqora_api_url: String,
    telegram_api_configured: bool,
    maintenance_message: String,
    data_dir: String,
}

#[derive(Serialize)]
struct AdminTenantsResponse {
    admin_username: String,
    tenants: Vec<AdminTenantView>,
    defaults: AdminDefaultsView,
}

#[derive(Serialize)]
struct AdminMessage {
    message: String,
}

async fn tenant_view(tenant: &TenantState, sessions: &HashMap<String, Session>) -> AdminTenantView {
    let config = tenant.config.read().await.clone();
    let summary = tenant.store.summary(Utc::now()).await;
    let runtime = tenant.runtime.read().await.clone();
    let active_sessions = sessions
        .values()
        .filter(|session| {
            matches!(session, Session::Tenant { id, via_admin: false } if *id == tenant.id)
        })
        .count();

    AdminTenantView {
        id: config.id,
        display_name: config.display_name,
        username: config.username,
        smm_api_key: config.smm_api_key,
        smm_api_url: config.smm_api_url,
        adsqora_api_key: config.adsqora_api_key,
        adsqora_api_url: config.adsqora_api_url,
        userbot_url: config.userbot_url,
        telegram_api_id: summary.telegram_api_id,
        telegram_api_hash: summary.telegram_api_hash,
        maintenance: config.maintenance,
        maintenance_message: config.maintenance_message,
        maintenance_since: config.maintenance_since,
        state_path: config.state_path,
        session_dir: config.session_dir,
        created_at: config.created_at,
        updated_at: config.updated_at,
        scanner_enabled: summary.scanner_enabled,
        scanning: runtime.scanning,
        last_run_at: runtime.last_run_at,
        last_error: runtime.last_error,
        keywords_total: summary.keywords_total,
        keywords_enabled: summary.keywords_enabled,
        accounts_total: summary.accounts_total,
        accounts_flooded: summary.accounts_flooded,
        results_total: summary.results_total,
        logs_total: summary.logs_total,
        active_sessions,
    }
}

async fn single_view(state: &AppState, tenant: &TenantState) -> AdminTenantView {
    let sessions = state.sessions.read().await.clone();
    tenant_view(tenant, &sessions).await
}

async fn list_tenants(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminTenantsResponse>, ApiError> {
    require_admin(&headers, &state).await?;
    let list = state.tenants.read().await.clone();
    let sessions = state.sessions.read().await.clone();
    let mut views = Vec::with_capacity(list.len());
    for tenant in &list {
        views.push(tenant_view(tenant, &sessions).await);
    }
    let defaults = &state.defaults;
    Ok(Json(AdminTenantsResponse {
        admin_username: state
            .admin
            .as_ref()
            .map(|admin| admin.username.clone())
            .unwrap_or_default(),
        tenants: views,
        defaults: AdminDefaultsView {
            smm_api_url: defaults.smm_api_url.clone(),
            adsqora_api_url: defaults.adsqora_api_url.clone(),
            telegram_api_configured: defaults.telegram_api_id.is_some()
                && defaults.telegram_api_hash.is_some(),
            maintenance_message: tenants::DEFAULT_MAINTENANCE_MESSAGE.to_string(),
            data_dir: state.data_dir.display().to_string(),
        },
    }))
}

/// Qo'shish va tahrirlash uchun umumiy forma. Tahrirlashda `None` — o'zgarmaydi;
/// bo'sh parol ham o'zgarmaydi.
#[derive(Deserialize)]
struct TenantUpsertRequest {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    smm_api_key: Option<String>,
    #[serde(default)]
    smm_api_url: Option<String>,
    #[serde(default)]
    adsqora_api_key: Option<String>,
    #[serde(default)]
    adsqora_api_url: Option<String>,
    #[serde(default)]
    userbot_url: Option<String>,
    /// Bo'sh bo'lsa serverdagi umumiy Telegram API ishlatiladi.
    #[serde(default)]
    telegram_api_id: Option<String>,
    #[serde(default)]
    telegram_api_hash: Option<String>,
    #[serde(default)]
    maintenance_message: Option<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string())
}

fn validate_url(label: &str, value: &str) -> Result<(), ApiError> {
    if value.is_empty() || value.starts_with("https://") || value.starts_with("http://") {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "{label} http:// yoki https:// bilan boshlanishi kerak"
        )))
    }
}

fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < 4 {
        return Err(ApiError::bad_request(
            "Parol kamida 4 belgidan iborat bo'lsin",
        ));
    }
    if password.chars().count() > 128 {
        return Err(ApiError::bad_request("Parol juda uzun"));
    }
    if password.trim() != password {
        return Err(ApiError::bad_request(
            "Parolning boshida yoki oxirida bo'sh joy bo'lmasin",
        ));
    }
    Ok(())
}

fn validate_message(message: &str) -> Result<(), ApiError> {
    if message.chars().count() > MAX_MAINTENANCE_MESSAGE {
        Err(ApiError::bad_request(format!(
            "Profilaktika matni {MAX_MAINTENANCE_MESSAGE} belgidan oshmasin"
        )))
    } else {
        Ok(())
    }
}

fn parse_api_id(value: Option<&str>) -> Result<Option<i32>, ApiError> {
    let value = value.unwrap_or("").trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i32>()
        .ok()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or_else(|| ApiError::bad_request("Telegram API ID musbat raqam bo'lishi kerak"))
}

async fn ensure_username_free(
    state: &AppState,
    username: &str,
    except_id: Option<&str>,
) -> Result<(), ApiError> {
    if username.is_empty() {
        return Err(ApiError::bad_request("Login kiriting"));
    }
    if username.chars().count() > 64 || username.chars().any(char::is_whitespace) {
        return Err(ApiError::bad_request(
            "Login bo'sh joysiz va 64 belgidan qisqa bo'lsin",
        ));
    }
    if let Some(admin) = &state.admin
        && admin.username.eq_ignore_ascii_case(username)
    {
        return Err(ApiError::conflict("Bu login admin uchun band"));
    }
    for tenant in state.tenants.read().await.iter() {
        if Some(tenant.id.as_str()) == except_id {
            continue;
        }
        let config = tenant.config.read().await;
        if config.username.eq_ignore_ascii_case(username) {
            return Err(ApiError::conflict(format!(
                "\"{}\" logini band ({})",
                config.username,
                config.display()
            )));
        }
    }
    Ok(())
}

/// Logindan bo'sh ID yasaydi: band bo'lsa `-2`, `-3`... qo'shiladi.
async fn free_id(state: &AppState, username: &str) -> String {
    let base = tenants::slugify(username);
    let list = state.tenants.read().await;
    let taken = |candidate: &str| {
        list.iter().any(|tenant| tenant.id == candidate)
            || state
                .data_dir
                .join(format!("state-{candidate}.json"))
                .exists()
            || state.data_dir.join(candidate).exists()
    };
    if !taken(&base) {
        return base;
    }
    let short: String = base.chars().take(28).collect();
    (2..)
        .map(|n| format!("{short}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

/// Admin bergan Telegram API ID/hash'ni foydalanuvchi bazasiga yozadi.
async fn apply_telegram_settings(
    tenant: &TenantState,
    api_id: Option<i32>,
    api_hash: Option<String>,
) -> Result<(), ApiError> {
    if api_id.is_none() && api_hash.is_none() {
        return Ok(());
    }
    let mut settings = tenant.store.telegram_settings().await;
    if let Some(api_id) = api_id {
        settings.api_id = Some(api_id);
    }
    if let Some(api_hash) = api_hash {
        settings.api_hash = Some(api_hash);
    }
    tenant.store.update_telegram(settings).await?;
    Ok(())
}

async fn create_tenant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<TenantUpsertRequest>,
) -> Result<Json<AdminTenantView>, ApiError> {
    require_admin(&headers, &state).await?;
    let _guard = state.admin_lock.lock().await;

    let username = clean(payload.username).unwrap_or_default();
    ensure_username_free(&state, &username, None).await?;
    let password = payload.password.unwrap_or_default();
    validate_password(&password)?;

    let id = match clean(payload.id).filter(|id| !id.is_empty()) {
        Some(id) => {
            if !tenants::is_valid_id(&id) {
                return Err(ApiError::bad_request(
                    "ID faqat kichik lotin harf, raqam, - va _ dan iborat bo'lsin (1-32 belgi)",
                ));
            }
            if state.tenant(&id).await.is_some() {
                return Err(ApiError::conflict(format!("\"{id}\" ID band")));
            }
            id
        }
        None => free_id(&state, &username).await,
    };
    let state_path = state.data_dir.join(format!("state-{id}.json"));
    if tokio::fs::try_exists(&state_path).await.unwrap_or(false) {
        return Err(ApiError::conflict(format!(
            "{} allaqachon mavjud — boshqa ID tanlang",
            state_path.display()
        )));
    }
    let session_dir = state.data_dir.join(&id);
    // Boshqa (yoki eski) sessiyalar bilan aralashib ketmasin — papka bo'sh joyda yaratiladi.
    if tokio::fs::try_exists(&session_dir).await.unwrap_or(false) {
        return Err(ApiError::conflict(format!(
            "{} papkasi allaqachon mavjud — boshqa ID tanlang",
            session_dir.display()
        )));
    }

    let smm_api_url = clean(payload.smm_api_url)
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| state.defaults.smm_api_url.clone());
    validate_url("SMM API URL", &smm_api_url)?;
    let adsqora_api_url = clean(payload.adsqora_api_url)
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| state.defaults.adsqora_api_url.clone());
    validate_url("Adsqora API URL", &adsqora_api_url)?;
    let userbot_url = clean(payload.userbot_url).unwrap_or_default();
    validate_url("Kanal tayyorlash havolasi", &userbot_url)?;
    let maintenance_message = clean(payload.maintenance_message).unwrap_or_default();
    validate_message(&maintenance_message)?;
    let telegram_api_id = parse_api_id(payload.telegram_api_id.as_deref())?;
    let telegram_api_hash = clean(payload.telegram_api_hash).filter(|hash| !hash.is_empty());

    let now = Utc::now();
    let record = TenantRecord {
        id: id.clone(),
        display_name: clean(payload.display_name).unwrap_or_default(),
        username,
        password_hash: tenants::hash_password_async(password).await?,
        state_path: state_path.to_string_lossy().to_string(),
        session_dir: session_dir.to_string_lossy().to_string(),
        smm_api_key: clean(payload.smm_api_key).unwrap_or_default(),
        smm_api_url,
        adsqora_api_key: clean(payload.adsqora_api_key).unwrap_or_default(),
        adsqora_api_url,
        userbot_url,
        maintenance: false,
        maintenance_message,
        maintenance_since: None,
        created_at: now,
        updated_at: now,
    };

    // Baza (state fayli) va sessiya papkasi shu yerda yaratiladi.
    let tenant = tenants::build_tenant(record.clone(), &state.defaults).await?;
    apply_telegram_settings(&tenant, telegram_api_id, telegram_api_hash).await?;
    let mut log = PanelLog::new(
        "info",
        "Foydalanuvchi yaratildi",
        "Admin panel orqali yaratildi: baza, sessiya papkasi va kalitlar sozlandi.",
    );
    log.source_channel = Some("Admin panel".to_string());
    tenant.store.push_logs(vec![log]).await?;

    let list = {
        let mut list = state.tenants.write().await;
        list.push(tenant.clone());
        list.clone()
    };
    if let Err(err) = tenants::save_registry(&state.registry_path, &list).await {
        // Ro'yxatga yozilmadi — xotiradan ham olib tashlaymiz va yaratilgan
        // fayllarni chetga suramiz, shunda qayta urinish toza boshlanadi.
        state.tenants.write().await.retain(|item| item.id != id);
        tenant.telegram.close().await;
        tenant.store.close().await;
        tenants::archive_tenant_files(&record, &[]).await;
        return Err(err.into());
    }
    tenants::spawn_tenant_tasks(&tenant);
    tracing::info!(tenant = %id, "admin: yangi foydalanuvchi yaratildi");

    Ok(Json(single_view(&state, &tenant).await))
}

async fn update_tenant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<TenantUpsertRequest>,
) -> Result<Json<AdminTenantView>, ApiError> {
    require_admin(&headers, &state).await?;
    let _guard = state.admin_lock.lock().await;
    let tenant = state
        .tenant(&id)
        .await
        .ok_or_else(|| ApiError::not_found("Foydalanuvchi topilmadi"))?;
    let current = tenant.config.read().await.clone();
    let mut next = current.clone();

    // Avval hammasini tekshiramiz — xato bo'lsa hech narsa o'zgarmaydi.
    if let Some(username) = clean(payload.username) {
        if username != current.username {
            ensure_username_free(&state, &username, Some(&id)).await?;
        }
        next.username = username;
    }
    let new_password = payload.password.filter(|password| !password.is_empty());
    if let Some(password) = &new_password {
        validate_password(password)?;
    }
    if let Some(display_name) = clean(payload.display_name) {
        next.display_name = display_name;
    }
    if let Some(key) = clean(payload.smm_api_key) {
        next.smm_api_key = key;
    }
    if let Some(url) = clean(payload.smm_api_url) {
        validate_url("SMM API URL", &url)?;
        next.smm_api_url = if url.is_empty() {
            state.defaults.smm_api_url.clone()
        } else {
            url
        };
    }
    if let Some(key) = clean(payload.adsqora_api_key) {
        next.adsqora_api_key = key;
    }
    if let Some(url) = clean(payload.adsqora_api_url) {
        validate_url("Adsqora API URL", &url)?;
        next.adsqora_api_url = if url.is_empty() {
            state.defaults.adsqora_api_url.clone()
        } else {
            url
        };
    }
    if let Some(url) = clean(payload.userbot_url) {
        validate_url("Kanal tayyorlash havolasi", &url)?;
        next.userbot_url = url;
    }
    if let Some(message) = clean(payload.maintenance_message) {
        validate_message(&message)?;
        next.maintenance_message = message;
    }
    let telegram_api_id = parse_api_id(payload.telegram_api_id.as_deref())?;
    let telegram_api_hash = clean(payload.telegram_api_hash).filter(|hash| !hash.is_empty());
    if let Some(password) = new_password.clone() {
        next.password_hash = tenants::hash_password_async(password).await?;
    }
    next.updated_at = Utc::now();

    // Avval tenants.json ga yozamiz; yozilmasa — eski holat tiklanadi va ishlab
    // turgan servislarga tegilmaydi (xotira va disk bir-biridan farq qilmasin).
    *tenant.config.write().await = next.clone();
    let list = state.tenants.read().await.clone();
    if let Err(err) = tenants::save_registry(&state.registry_path, &list).await {
        *tenant.config.write().await = current;
        return Err(err.into());
    }

    // Kuchga kiritamiz: servislar yangi kalitlarni darhol ishlatadi.
    tenant
        .smmmain
        .update(next.smm_api_key.clone(), next.smm_api_url.clone());
    tenant
        .adsqora
        .update(next.adsqora_api_key.clone(), next.adsqora_api_url.clone());

    if new_password.is_some() {
        // Parol almashdi — foydalanuvchining eski sessiyalari yopiladi.
        state.sessions.write().await.retain(|_, session| {
            !matches!(session, Session::Tenant { id: sid, via_admin: false } if *sid == id)
        });
    }
    tracing::info!(tenant = %id, password_changed = new_password.is_some(), "admin: foydalanuvchi yangilandi");
    apply_telegram_settings(&tenant, telegram_api_id, telegram_api_hash).await?;

    Ok(Json(single_view(&state, &tenant).await))
}

async fn delete_tenant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AdminMessage>, ApiError> {
    require_admin(&headers, &state).await?;
    let _guard = state.admin_lock.lock().await;

    let (tenant, index, remaining) = {
        let mut list = state.tenants.write().await;
        let index = list
            .iter()
            .position(|tenant| tenant.id == id)
            .ok_or_else(|| ApiError::not_found("Foydalanuvchi topilmadi"))?;
        let tenant = list.remove(index);
        (tenant, index, list.clone())
    };
    if let Err(err) = tenants::save_registry(&state.registry_path, &remaining).await {
        state.tenants.write().await.insert(index, tenant);
        return Err(err.into());
    }

    // Fon ishlari to'xtaydi, sessiyalar yopiladi, Telegram uziladi.
    tenant
        .stopped
        .store(true, std::sync::atomic::Ordering::SeqCst);
    state
        .sessions
        .write()
        .await
        .retain(|_, session| !matches!(session, Session::Tenant { id: sid, .. } if *sid == id));
    tenant.telegram.close().await;
    tenant.store.close().await;

    // Boshqa foydalanuvchi ishlatayotgan yo'l (masalan umumiy data/) arxivlanmaydi.
    let mut in_use = Vec::new();
    for other in &remaining {
        let config = other.config.read().await;
        in_use.push(config.state_path.clone());
        in_use.push(config.session_dir.clone());
    }
    let record = tenant.config.read().await.clone();
    let moved = tenants::archive_tenant_files(&record, &in_use).await;
    tracing::info!(tenant = %id, archived = ?moved, "admin: foydalanuvchi o'chirildi");

    let message = if moved.is_empty() {
        format!("{} o'chirildi", record.display())
    } else {
        format!(
            "{} o'chirildi. Ma'lumotlari arxivda: {}",
            record.display(),
            moved.join(", ")
        )
    };
    Ok(Json(AdminMessage { message }))
}

#[derive(Deserialize)]
struct MaintenanceRequest {
    ids: Vec<String>,
    enabled: bool,
    #[serde(default)]
    message: Option<String>,
}

/// Tanlangan foydalanuvchilarni profilaktikaga o'tkazadi yoki ishga qaytaradi.
/// Profilaktikada: panel yopiq, skaner va orderlar pauzada. Foydalanuvchi
/// sozlamalariga tegilmaydi, shuning uchun ishga qaytganda qolgan joyidan davom etadi.
async fn set_maintenance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<MaintenanceRequest>,
) -> Result<Json<AdminMessage>, ApiError> {
    require_admin(&headers, &state).await?;
    let _guard = state.admin_lock.lock().await;
    if payload.ids.is_empty() {
        return Err(ApiError::bad_request("Hech kim tanlanmagan"));
    }
    // Matn berilsa (bo'sh ham) — yoqilayotganlarga yoziladi; bo'sh matn standart
    // yozuvni ko'rsatadi. Berilmasa — har kimning oldingi matni qoladi.
    let message = clean(payload.message);
    if let Some(message) = &message {
        validate_message(message)?;
    }

    let list = state.tenants.read().await.clone();
    let now = Utc::now();
    // (tenant, oldingi holat, holati almashdimi) — yozish xato bersa qaytarish uchun.
    let mut touched: Vec<(TenantState, TenantRecord, bool)> = Vec::new();
    for tenant in list
        .iter()
        .filter(|tenant| payload.ids.contains(&tenant.id))
    {
        let mut config = tenant.config.write().await;
        let previous = config.clone();
        if payload.enabled
            && let Some(message) = &message
        {
            config.maintenance_message = message.clone();
        }
        let switched = config.maintenance != payload.enabled;
        if switched {
            config.maintenance = payload.enabled;
            config.maintenance_since = payload.enabled.then_some(now);
        }
        config.updated_at = now;
        drop(config);
        touched.push((tenant.clone(), previous, switched));
    }
    if touched.is_empty() {
        return Err(ApiError::not_found("Tanlangan foydalanuvchilar topilmadi"));
    }
    if let Err(err) = tenants::save_registry(&state.registry_path, &list).await {
        for (tenant, previous, _) in &touched {
            *tenant.config.write().await = previous.clone();
        }
        return Err(err.into());
    }

    let (level, title, text) = if payload.enabled {
        (
            "warning",
            "Profilaktika yoqildi",
            "Admin profilaktika rejimini yoqdi: panel yopildi, skaner va orderlar pauzada.",
        )
    } else {
        (
            "success",
            "Profilaktika tugadi",
            "Admin ish holatiga qaytardi: skaner qolgan joyidan davom etadi.",
        )
    };
    let mut changed = 0usize;
    for (tenant, _, switched) in &touched {
        if !switched {
            continue;
        }
        changed += 1;
        let mut log = PanelLog::new(level, title, text);
        log.source_channel = Some("Admin panel".to_string());
        let _ = tenant.store.push_logs(vec![log]).await;
        tracing::info!(tenant = %tenant.id, maintenance = payload.enabled, "admin: profilaktika holati o'zgardi");
    }
    let touched = touched.len();

    let action = if payload.enabled {
        "profilaktikaga o'tkazildi"
    } else {
        "ish holatiga qaytarildi"
    };
    let message = if changed == touched {
        format!("{changed} ta foydalanuvchi {action}")
    } else {
        format!(
            "{changed} ta foydalanuvchi {action} ({} tasi allaqachon shu holatda edi)",
            touched - changed
        )
    };
    Ok(Json(AdminMessage { message }))
}

#[derive(Serialize)]
struct OpenPanelResponse {
    token: String,
    tenant_id: String,
    display_name: String,
    maintenance: bool,
}

/// Admin foydalanuvchi panelini o'zi ochadi (sozlash, QR ulash uchun).
/// Bu sessiyaga profilaktika ta'sir qilmaydi.
async fn open_tenant_panel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<OpenPanelResponse>, ApiError> {
    require_admin(&headers, &state).await?;
    let tenant = state
        .tenant(&id)
        .await
        .ok_or_else(|| ApiError::not_found("Foydalanuvchi topilmadi"))?;
    let token = new_session(
        &state,
        Session::Tenant {
            id: tenant.id.clone(),
            via_admin: true,
        },
    )
    .await;
    let config = tenant.config.read().await;
    Ok(Json(OpenPanelResponse {
        token,
        tenant_id: config.id.clone(),
        display_name: config.display(),
        maintenance: config.maintenance,
    }))
}

#[derive(Serialize)]
struct CheckItem {
    ok: bool,
    message: String,
}

#[derive(Serialize)]
struct TenantCheckResponse {
    smm: CheckItem,
    adsqora: CheckItem,
    telegram: CheckItem,
}

/// Kalitlar ishlayaptimi — yon ta'sirsiz tekshiruv (balans, kanal tekshiruvi).
async fn check_tenant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<TenantCheckResponse>, ApiError> {
    require_admin(&headers, &state).await?;
    let tenant = state
        .tenant(&id)
        .await
        .ok_or_else(|| ApiError::not_found("Foydalanuvchi topilmadi"))?;

    let smm = async {
        if !tenant.smmmain.is_configured() {
            CheckItem {
                ok: false,
                message: "SMM API kaliti kiritilmagan".to_string(),
            }
        } else {
            match tenant.smmmain.balance().await {
                Ok(outcome) => CheckItem {
                    ok: true,
                    message: format!(
                        "Balans: {} {}",
                        outcome.balance.unwrap_or_else(|| "-".to_string()),
                        outcome.currency.unwrap_or_default()
                    )
                    .trim()
                    .to_string(),
                },
                Err(err) => CheckItem {
                    ok: false,
                    message: format!("{err:#}"),
                },
            }
        }
    };

    let adsqora = async {
        match tenant.adsqora.check_channel("durov").await {
            Ok(outcome) if (200..300).contains(&outcome.status) => CheckItem {
                ok: true,
                message: "Kalit ishlayapti".to_string(),
            },
            Ok(outcome) if outcome.status == 401 || outcome.status == 403 => CheckItem {
                ok: false,
                message: format!("Kalit noto'g'ri (HTTP {})", outcome.status),
            },
            Ok(outcome) => CheckItem {
                ok: false,
                message: format!("Kutilmagan javob: HTTP {}", outcome.status),
            },
            Err(err) => CheckItem {
                ok: false,
                message: format!("{err:#}"),
            },
        }
    };

    let telegram = async {
        let settings = tenant.store.telegram_settings().await;
        let api_ready = settings.api_id.is_some()
            && settings
                .api_hash
                .as_deref()
                .map(|hash| !hash.trim().is_empty())
                .unwrap_or(false);
        if !api_ready {
            CheckItem {
                ok: false,
                message: "Telegram API ID/hash kiritilmagan".to_string(),
            }
        } else {
            let accounts = tenant.store.accounts().await;
            let mut checks = tokio::task::JoinSet::new();
            for account in &accounts {
                let telegram = tenant.telegram.clone();
                let account_id = account.id.clone();
                checks.spawn(async move { telegram.is_account_connected(&account_id).await });
            }
            let mut connected = 0usize;
            while let Some(result) = checks.join_next().await {
                if matches!(result, Ok(true)) {
                    connected += 1;
                }
            }
            CheckItem {
                ok: connected > 0,
                message: if accounts.is_empty() {
                    "API sozlangan, lekin userbot akkaunt yo'q (panelda QR bilan ulanadi)"
                        .to_string()
                } else {
                    format!("{connected}/{} ta akkaunt ulangan", accounts.len())
                },
            }
        }
    };

    // Uchala tekshiruv parallel — sekin servis boshqasini kutib turmaydi.
    let (smm, adsqora, telegram) = tokio::join!(smm, adsqora, telegram);

    Ok(Json(TenantCheckResponse {
        smm,
        adsqora,
        telegram,
    }))
}
