use crate::admin::{self, AdminCredentials};
use crate::models::{
    AccountIdRequest, AccountStatus, CredentialsRequest, DashboardResponse, ErrorResponse,
    LoginRequest, LoginResponse, MeResponse, PanelLog, QrPasswordRequest, QrPollResponse,
    QrStartResponse, RuntimeStatus, Settings, SmmBalance, TelegramAccount, TelegramSettings,
};
use crate::telegram::QrOutcome;
use crate::scanner;
use crate::smmmain::SmmMainService;
use crate::store::Store;
use crate::telegram::TelegramService;
use crate::tenants::{self, TenantDefaults, TenantRecord};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use uuid::Uuid;

#[derive(Clone)]
pub struct TenantState {
    /// O'zgarmas tenant ID (masalan "izzatillo").
    pub id: String,
    pub store: Arc<Store>,
    pub telegram: Arc<TelegramService>,
    pub smmmain: Arc<SmmMainService>,
    pub adsqora: Arc<crate::adsqora::AdsQoraService>,
    pub runtime: Arc<RwLock<RuntimeInfo>>,
    /// Akkauntlar bo'yicha round-robin hisoblagich.
    pub rr: Arc<AtomicUsize>,
    /// Login, parol xeshi, havolalar va profilaktika holati — admin paneldan
    /// o'zgaradi, barcha klonlar (skaner sikli ham) yangi qiymatni ko'radi.
    pub config: Arc<RwLock<TenantRecord>>,
    /// Tenant o'chirilganda true — skaner sikli to'xtaydi.
    pub stopped: Arc<AtomicBool>,
}

/// Router holati: tenantlar ro'yxati (admin paneldan restartsiz qo'shiladi) +
/// umumiy sessiya jadvali (token -> kim kirgan).
#[derive(Clone)]
pub struct AppState {
    pub tenants: Arc<RwLock<Vec<TenantState>>>,
    pub sessions: Arc<RwLock<HashMap<String, Session>>>,
    /// Bosh admin (env: SUPERADMIN_USERNAME / SUPERADMIN_PASSWORD). Yo'q bo'lsa
    /// admin panelga kirib bo'lmaydi.
    pub admin: Option<Arc<AdminCredentials>>,
    pub registry_path: Arc<PathBuf>,
    /// Yangi foydalanuvchilar bazasi shu papkada yaratiladi.
    pub data_dir: Arc<PathBuf>,
    pub defaults: Arc<TenantDefaults>,
    /// Tenant qo'shish/tahrirlash/o'chirishni ketma-ket bajaradi.
    pub admin_lock: Arc<Mutex<()>>,
    /// Noto'g'ri login urinishlari (IP -> soni, oynaning boshlanishi).
    pub login_failures: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    /// Argon2 tekshiruvi ~19 MiB xotira oladi — bir vaqtda faqat shuncha.
    pub password_checks: Arc<Semaphore>,
}

/// Bir IP'dan shuncha noto'g'ri urinishdan keyin login vaqtincha yopiladi.
const LOGIN_MAX_FAILURES: u32 = 10;
const LOGIN_FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

impl AppState {
    pub async fn tenant(&self, id: &str) -> Option<TenantState> {
        self.tenants
            .read()
            .await
            .iter()
            .find(|tenant| tenant.id == id)
            .cloned()
    }
}

#[derive(Clone, Debug)]
pub enum Session {
    Admin,
    /// `via_admin` — admin paneldan "Panelni ochish" orqali kirilgan:
    /// profilaktika bu sessiyani to'smaydi.
    Tenant { id: String, via_admin: bool },
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeInfo {
    pub scanning: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl TenantState {
    pub async fn in_maintenance(&self) -> bool {
        self.config.read().await.maintenance
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Skaner/order to'xtashi kerakmi: profilaktika yoki tenant o'chirilgan.
    pub async fn should_pause(&self) -> bool {
        self.is_stopped() || self.in_maintenance().await
    }

    pub async fn status(&self) -> RuntimeStatus {
        let runtime = self.runtime.read().await.clone();
        let snapshot = self.store.snapshot().await;
        let next_run_at = next_keyword_run_at(&snapshot.settings).or(runtime.next_run_at);
        let mut telegram_connected = false;
        for account in &snapshot.accounts {
            if self.telegram.is_account_connected(&account.id).await {
                telegram_connected = true;
                break;
            }
        }

        RuntimeStatus {
            telegram_connected,
            login_waiting_for: None,
            scanning: runtime.scanning,
            last_run_at: runtime.last_run_at,
            next_run_at,
            last_error: runtime.last_error,
            total_results: snapshot.results.len(),
            total_logs: snapshot.logs.len(),
        }
    }
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
    /// Mashina uchun kod (masalan "maintenance") — frontend shunga qarab ekran tanlaydi.
    code: Option<&'static str>,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Avtorizatsiya kerak")
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    /// Foydalanuvchi profilaktikada. 423 (Locked) — 5xx emas, shuning uchun
    /// Cloudflare/nginx javobni o'z xato sahifasi bilan almashtirmaydi.
    pub fn maintenance(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::LOCKED,
            message: message.into(),
            code: Some("maintenance"),
        }
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, value.into().to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.code.map(str::to_string),
            }),
        )
            .into_response()
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/me", get(me))
        .route("/dashboard", get(dashboard))
        .route("/settings", get(get_settings).put(update_settings))
        .route("/results", get(get_results).delete(clear_results))
        .route("/logs", get(get_logs).delete(clear_logs))
        .route("/smmmain/balance", get(smmmain_balance))
        .route("/adsqora/channels", post(adsqora_add_channel))
        .route("/adsqora/channels/check", get(adsqora_check_channel))
        .route("/status", get(status))
        .route("/scan/run", post(run_scan))
        .route("/telegram/credentials", post(telegram_credentials))
        .route("/telegram/accounts", get(telegram_accounts))
        .route("/telegram/qr/start", post(telegram_qr_start))
        .route("/telegram/qr/poll", post(telegram_qr_poll))
        .route("/telegram/qr/password", post(telegram_qr_password))
        .route("/telegram/account/remove", post(telegram_account_remove))
        .nest("/admin", admin::router())
        .with_state(state)
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let username = payload.username.trim();

    // Parolni terib topishdan himoya: bir IP'dan ko'p xato bo'lsa vaqtincha yopiq.
    // Navbat ruxsatidan keyin tekshiriladi — parallel so'rovlar blokni chetlab o'tmaydi.
    let ip = client_ip(&headers);
    let permit = state
        .password_checks
        .acquire()
        .await
        .map_err(|_| ApiError::unauthorized())?;
    {
        let mut failures = state.login_failures.lock().await;
        failures.retain(|_, (_, since)| since.elapsed() < LOGIN_FAILURE_WINDOW);
        if failures
            .get(&ip)
            .is_some_and(|(count, _)| *count >= LOGIN_MAX_FAILURES)
        {
            return Err(ApiError::too_many_requests(
                "Juda ko'p noto'g'ri urinish. 15 daqiqadan keyin qayta urinib ko'ring",
            ));
        }
    }

    // 1) Bosh admin.
    if let Some(admin) = &state.admin
        && admin.matches(username, &payload.password)
    {
        state.login_failures.lock().await.remove(&ip);
        let token = new_session(&state, Session::Admin).await;
        tracing::info!(login = %username, "admin panelga kirildi");
        return Ok(Json(LoginResponse {
            token,
            role: "admin".to_string(),
        }));
    }

    // 2) Foydalanuvchilar: login bo'yicha topib, parol xeshini tekshiramiz.
    //    Profilaktikadagi foydalanuvchi ham kiradi — unga profilaktika ekrani chiqadi.
    let mut candidate = None;
    for tenant in state.tenants.read().await.iter() {
        let config = tenant.config.read().await;
        if config.username.eq_ignore_ascii_case(username) {
            candidate = Some((tenant.id.clone(), config.password_hash.clone()));
            break;
        }
    }
    if let Some((id, hash)) = candidate
        && tenants::verify_password_async(payload.password.clone(), hash).await
    {
        state.login_failures.lock().await.remove(&ip);
        let token = new_session(
            &state,
            Session::Tenant {
                id,
                via_admin: false,
            },
        )
        .await;
        return Ok(Json(LoginResponse {
            token,
            role: "tenant".to_string(),
        }));
    }

    {
        let mut failures = state.login_failures.lock().await;
        let entry = failures.entry(ip.clone()).or_insert((0, Instant::now()));
        entry.0 += 1;
        if entry.0 == LOGIN_MAX_FAILURES {
            tracing::warn!(ip = %ip, login = %username, "ko'p noto'g'ri login urinishi — IP vaqtincha yopildi");
        }
    }
    drop(permit);
    // Parol terib ko'rishni sekinlashtirish uchun kichik kechikish.
    tokio::time::sleep(Duration::from_millis(400)).await;
    Err(ApiError::new(StatusCode::UNAUTHORIZED, "Login yoki parol noto'g'ri"))
}

/// Mijoz IP manzili: Cloudflare (`CF-Connecting-IP`) → nginx (`X-Forwarded-For`,
/// `X-Real-IP`). Lokal ishga tushirishda sarlavha bo'lmaydi.
fn client_ip(headers: &HeaderMap) -> String {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    header("cf-connecting-ip")
        .or_else(|| header("x-forwarded-for").and_then(|value| value.split(',').next().map(str::trim)))
        .or_else(|| header("x-real-ip"))
        .unwrap_or("local")
        .to_string()
}

pub async fn new_session(state: &AppState, session: Session) -> String {
    let token = Uuid::new_v4().to_string();
    state.sessions.write().await.insert(token.clone(), session);
    token
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SimpleMessage>, ApiError> {
    if let Some(token) = bearer_token(&headers) {
        let mut sessions = state.sessions.write().await;
        // Admin chiqsa, u ochgan foydalanuvchi panellari ham yopiladi.
        if let Some(Session::Admin) = sessions.remove(&token) {
            sessions.retain(|_, session| !matches!(session, Session::Tenant { via_admin: true, .. }));
        }
    }
    Ok(Json(SimpleMessage::new("Chiqildi")))
}

/// Kim kirganini qaytaradi. Profilaktikada ham ishlaydi — frontend shu orqali
/// profilaktika tugaganini bilib, panelni qayta ochadi.
async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, ApiError> {
    match require_session(&headers, &state).await? {
        Session::Admin => Ok(Json(MeResponse {
            role: "admin".to_string(),
            username: state
                .admin
                .as_ref()
                .map(|admin| admin.username.clone())
                .unwrap_or_default(),
            tenant_id: None,
            display_name: None,
            via_admin: false,
            maintenance: false,
            maintenance_message: None,
            maintenance_since: None,
        })),
        Session::Tenant { id, via_admin } => {
            let tenant = state.tenant(&id).await.ok_or_else(ApiError::unauthorized)?;
            let config = tenant.config.read().await;
            Ok(Json(MeResponse {
                role: "tenant".to_string(),
                username: config.username.clone(),
                tenant_id: Some(config.id.clone()),
                display_name: Some(config.display()),
                via_admin,
                maintenance: config.maintenance,
                maintenance_message: config.maintenance.then(|| config.maintenance_text()),
                maintenance_since: config.maintenance_since,
            }))
        }
    }
}

async fn dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let snapshot = state.store.snapshot().await;
    let accounts = account_statuses(&state, &snapshot.accounts).await;
    let stats_24h = state
        .store
        .stats_24h(&snapshot.settings.whitelist_channels, Utc::now())
        .await;
    let (display_name, userbot_url, maintenance) = {
        let config = state.config.read().await;
        (config.display(), config.userbot_url.clone(), config.maintenance)
    };
    Ok(Json(DashboardResponse {
        settings: snapshot.settings,
        telegram: public_telegram_settings(snapshot.telegram),
        smm_balance: public_smm_balance(&state).await,
        status: state.status().await,
        results: snapshot.results,
        logs: snapshot.logs,
        accounts,
        stats_24h,
        userbot_url,
        display_name,
        maintenance,
    }))
}

async fn account_statuses(state: &TenantState, accounts: &[TelegramAccount]) -> Vec<AccountStatus> {
    let now = Utc::now();
    let mut out = Vec::with_capacity(accounts.len());
    for account in accounts {
        let connected = state.telegram.is_account_connected(&account.id).await;
        let flooded = account.flood_until.map(|until| until > now).unwrap_or(false);
        out.push(AccountStatus {
            id: account.id.clone(),
            label: account.label.clone(),
            username: account.username.clone(),
            connected,
            flooded,
            flood_until: account.flood_until,
            created_at: account.created_at,
            last_used_at: account.last_used_at,
        });
    }
    out
}

#[derive(serde::Deserialize)]
struct AdsQoraAddRequest {
    link: String,
}

#[derive(serde::Deserialize)]
struct AdsQoraCheckQuery {
    link: String,
}

/// adsqora javobini (status + JSON) frontendga o'zgarishsiz uzatamiz.
fn adsqora_response(outcome: crate::adsqora::AdsQoraOutcome) -> Response {
    let status = StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::BAD_GATEWAY);
    (status, Json(outcome.body)).into_response()
}

async fn adsqora_add_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AdsQoraAddRequest>,
) -> Result<Response, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let link = payload.link.trim();
    if link.is_empty() {
        return Err(ApiError::bad_request("Kanal linki bo'sh"));
    }
    let outcome = state
        .adsqora
        .add_channel(link)
        .await
        .map_err(|err| ApiError::bad_request(err.to_string()))?;
    Ok(adsqora_response(outcome))
}

async fn adsqora_check_channel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdsQoraCheckQuery>,
) -> Result<Response, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let link = query.link.trim();
    if link.is_empty() {
        return Err(ApiError::bad_request("Kanal linki bo'sh"));
    }
    let outcome = state
        .adsqora
        .check_channel(link)
        .await
        .map_err(|err| ApiError::bad_request(err.to_string()))?;
    Ok(adsqora_response(outcome))
}

async fn smmmain_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SmmBalance>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(public_smm_balance(&state).await))
}

async fn get_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Settings>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(state.store.settings().await))
}

async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let previous = state.store.settings().await;
    let clean = state.store.update_settings(settings).await?;

    if previous.enabled != clean.enabled {
        let (title, message) = if clean.enabled {
            (
                "Skaner boshlandi",
                format!(
                    "Avtomatik skaner yoqildi. Umumiy interval: {} sekund.",
                    clean.interval_seconds
                ),
            )
        } else {
            (
                "Skaner to'xtatildi",
                "Avtomatik skaner admin tomonidan to'xtatildi.".to_string(),
            )
        };
        let mut log = PanelLog::new("info", title, message);
        log.source_channel = Some("Admin panel".to_string());
        log.raw_response = Some(if clean.enabled {
            format!("Interval: {} sekund", clean.interval_seconds)
        } else {
            "Admin to'xtatdi".to_string()
        });
        state.store.push_logs(vec![log]).await?;
    }

    Ok(Json(clean))
}

async fn get_results(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::models::AdResult>>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(state.store.snapshot().await.results))
}

async fn clear_results(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SimpleMessage>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    state.store.clear_results().await?;
    Ok(Json(SimpleMessage::new("Natijalar tozalandi")))
}

async fn get_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PanelLog>>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(state.store.snapshot().await.logs))
}

async fn clear_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SimpleMessage>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    state.store.clear_logs().await?;
    Ok(Json(SimpleMessage::new("Loglar tozalandi")))
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RuntimeStatus>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(state.status().await))
}

async fn run_scan(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::models::ScanResponse>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    Ok(Json(scanner::scan_once(state).await?))
}

async fn telegram_credentials(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CredentialsRequest>,
) -> Result<Json<SimpleMessage>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    if payload.api_id <= 0 || payload.api_hash.trim().is_empty() {
        return Err(ApiError::bad_request("API ID va API hash kerak"));
    }
    let mut settings = state.store.telegram_settings().await;
    settings.api_id = Some(payload.api_id);
    settings.api_hash = Some(payload.api_hash.trim().to_string());
    state.store.update_telegram(settings).await?;
    Ok(Json(SimpleMessage::new("API ma'lumotlari saqlandi")))
}

async fn telegram_accounts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccountStatus>>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let accounts = state.store.accounts().await;
    Ok(Json(account_statuses(&state, &accounts).await))
}

async fn telegram_qr_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<QrStartResponse>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let settings = state.store.telegram_settings().await;
    let api_id = settings
        .api_id
        .ok_or_else(|| ApiError::bad_request("Avval API ID/hash kiriting"))?;
    let api_hash = settings
        .api_hash
        .filter(|h| !h.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("Avval API ID/hash kiriting"))?;

    let account_id = Uuid::new_v4().to_string();
    let (qr_url, expires_at) = state
        .telegram
        .start_qr(&account_id, api_id, &api_hash)
        .await?;

    Ok(Json(QrStartResponse {
        account_id,
        qr_url,
        expires_at,
    }))
}

async fn telegram_qr_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AccountIdRequest>,
) -> Result<Json<QrPollResponse>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    let outcome = state.telegram.poll_qr(&payload.account_id).await?;
    Ok(Json(qr_response(&state, &payload.account_id, outcome).await?))
}

async fn telegram_qr_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<QrPasswordRequest>,
) -> Result<Json<QrPollResponse>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    if payload.password.trim().is_empty() {
        return Err(ApiError::bad_request("2FA parol kerak"));
    }
    let outcome = state
        .telegram
        .submit_qr_password(&payload.account_id, payload.password.trim())
        .await?;
    Ok(Json(qr_response(&state, &payload.account_id, outcome).await?))
}

async fn telegram_account_remove(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AccountIdRequest>,
) -> Result<Json<SimpleMessage>, ApiError> {
    let state = require_auth(&headers, &state).await?;
    state.telegram.remove_account_session(&payload.account_id).await?;
    state.store.remove_account(&payload.account_id).await?;
    Ok(Json(SimpleMessage::new("Akkaunt o'chirildi")))
}

/// QR natijasini javobga aylantiradi; ulanganda akkauntni saqlaydi.
async fn qr_response(
    state: &TenantState,
    account_id: &str,
    outcome: QrOutcome,
) -> Result<QrPollResponse, ApiError> {
    match outcome {
        QrOutcome::Waiting { qr_url, expires_at } => Ok(QrPollResponse {
            account_id: account_id.to_string(),
            status: "waiting".to_string(),
            qr_url: Some(qr_url),
            expires_at: Some(expires_at),
            message: "QR kutilmoqda".to_string(),
        }),
        QrOutcome::NeedPassword => Ok(QrPollResponse {
            account_id: account_id.to_string(),
            status: "password".to_string(),
            qr_url: None,
            expires_at: None,
            message: "2FA parol kerak".to_string(),
        }),
        QrOutcome::Connected { username } => {
            // Akkaunt allaqachon ro'yxatda bo'lmasa, qo'shamiz.
            let exists = state
                .store
                .accounts()
                .await
                .iter()
                .any(|a| a.id == account_id);
            if !exists {
                let label = username
                    .clone()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| "Akkaunt".to_string());
                state
                    .store
                    .add_account(TelegramAccount {
                        id: account_id.to_string(),
                        label: Some(label),
                        username,
                        created_at: Utc::now(),
                        last_used_at: None,
                        flood_until: None,
                    })
                    .await?;
            }
            Ok(QrPollResponse {
                account_id: account_id.to_string(),
                status: "connected".to_string(),
                qr_url: None,
                expires_at: None,
                message: "Akkaunt ulandi".to_string(),
            })
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| value.to_string())
}

pub async fn require_session(headers: &HeaderMap, state: &AppState) -> Result<Session, ApiError> {
    let token = bearer_token(headers).ok_or_else(ApiError::unauthorized)?;
    state
        .sessions
        .read()
        .await
        .get(&token)
        .cloned()
        .ok_or_else(ApiError::unauthorized)
}

/// Foydalanuvchi paneli endpointlari uchun: sessiya tenantini qaytaradi.
/// Tenant profilaktikada bo'lsa (va admin orqali kirilmagan bo'lsa) — hech bir
/// oyna ochilmaydi, faqat profilaktika xabari qaytadi.
async fn require_auth(headers: &HeaderMap, state: &AppState) -> Result<TenantState, ApiError> {
    let (id, via_admin) = match require_session(headers, state).await? {
        Session::Tenant { id, via_admin } => (id, via_admin),
        Session::Admin => {
            return Err(ApiError::forbidden(
                "Bu admin sessiyasi — foydalanuvchi panelini admin paneldan oching",
            ));
        }
    };
    let tenant = state.tenant(&id).await.ok_or_else(ApiError::unauthorized)?;
    if !via_admin {
        let config = tenant.config.read().await;
        if config.maintenance {
            return Err(ApiError::maintenance(config.maintenance_text()));
        }
    }
    Ok(tenant)
}

fn public_telegram_settings(mut settings: TelegramSettings) -> TelegramSettings {
    if settings.api_hash.is_some() {
        settings.api_hash = Some("configured".to_string());
    }
    settings
}

async fn public_smm_balance(state: &TenantState) -> SmmBalance {
    let checked_at = Utc::now();
    if !state.smmmain.is_configured() {
        return SmmBalance {
            configured: false,
            balance: None,
            currency: None,
            error: Some("SMM API kaliti kiritilmagan".to_string()),
            checked_at,
        };
    }

    match state.smmmain.balance().await {
        Ok(outcome) => SmmBalance {
            configured: true,
            balance: outcome.balance,
            currency: outcome.currency,
            error: None,
            checked_at,
        },
        Err(err) => SmmBalance {
            configured: true,
            balance: None,
            currency: None,
            error: Some(err.to_string()),
            checked_at,
        },
    }
}

fn next_keyword_run_at(settings: &Settings) -> Option<DateTime<Utc>> {
    settings
        .keyword_rules
        .iter()
        .filter(|rule| rule.enabled)
        .filter_map(|rule| rule.next_check_at)
        .min()
}

#[derive(Serialize)]
struct SimpleMessage {
    message: String,
}

impl SimpleMessage {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
