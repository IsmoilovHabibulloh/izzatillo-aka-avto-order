mod adsqora;
mod api;
mod models;
mod scanner;
mod smmmain;
mod store;
mod telegram;

use crate::adsqora::AdsQoraService;
use crate::api::{AppState, RuntimeInfo, TenantState};
use crate::models::DEFAULT_SMMMAIN_SERVICE_ID;
use crate::smmmain::SmmMainService;
use crate::store::Store;
use crate::telegram::TelegramService;
use anyhow::{Context, Result};
use axum::Router;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let static_dir = env::var("STATIC_DIR").unwrap_or_else(|_| "frontend/dist".to_string());

    // Tenantlar: TENANTS=izzatillo,nodiraka bo'lsa har biri o'z sozlamalari bilan;
    // bo'lmasa eski (bitta admin) rejim — legacy env nomlaridan o'qiladi.
    let tenant_names: Vec<String> = env::var("TENANTS")
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect();

    let mut tenants: Vec<TenantState> = Vec::new();
    if tenant_names.is_empty() {
        tenants.push(build_tenant("default", None).await?);
    } else {
        for name in &tenant_names {
            tenants.push(build_tenant(name, Some(name)).await?);
        }
    }

    let app_state = AppState {
        tenants: tenants.clone(),
        sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    for tenant in &tenants {
        tracing::info!(
            tenant = %tenant.name,
            login = %tenant.admin_username,
            "tenant yuklandi"
        );
        // Mavjud akkauntlarni fonda ulab qo'yamiz (status to'g'ri ko'rinishi uchun).
        let warm = tenant.clone();
        tokio::spawn(async move {
            if let Some(api_id) = warm.store.telegram_settings().await.api_id {
                for account in warm.store.accounts().await {
                    let _ = warm
                        .telegram
                        .ensure_account_client(&account.id, api_id)
                        .await;
                }
            }
        });

        tokio::spawn(scanner::scanner_loop(tenant.clone()));
    }

    let app = build_router(app_state, PathBuf::from(static_dir));
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .context("PORT noto'g'ri")?;
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .context("HOST/PORT noto'g'ri")?;

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("VIP Ads server ishga tushdi: http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Bitta tenantni env'dan quradi. `prefix` berilsa TENANT_{PREFIX}_* nomlari
/// ustuvor, topilmasa umumiy (legacy) nomlarga qaytadi.
async fn build_tenant(name: &str, prefix: Option<&str>) -> Result<TenantState> {
    let upper = prefix.map(|value| value.to_uppercase());
    let var = |suffix: &str, legacy: &str, default: &str| -> String {
        let mut value = None;
        if let Some(upper) = &upper {
            value = env::var(format!("TENANT_{upper}_{suffix}")).ok();
        }
        value
            .or_else(|| env::var(legacy).ok())
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .unwrap_or_else(|| default.to_string())
    };
    // Faqat tenant-prefiksli env (legacy'ga qaytmaydi) — kalitlar aralashib ketmasligi uchun.
    let tenant_only = |suffix: &str, default: &str| -> String {
        upper
            .as_ref()
            .and_then(|upper| env::var(format!("TENANT_{upper}_{suffix}")).ok())
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .unwrap_or_else(|| default.to_string())
    };

    let (state_path, session_path, smm_key, adsqora_key);
    if prefix.is_none() {
        // Legacy bitta-admin rejimi: eski env nomlari.
        state_path = env::var("STATE_PATH").unwrap_or_else(|_| "data/state.json".to_string());
        session_path = env::var("TELEGRAM_SESSION_PATH")
            .unwrap_or_else(|_| "data/userbot.session".to_string());
        smm_key = env::var("SMMMAIN_API_KEY").unwrap_or_default();
        adsqora_key = env::var("ADSQORA_API_KEY").unwrap_or_default();
    } else {
        state_path = tenant_only("STATE_PATH", &format!("data/state-{name}.json"));
        session_path = tenant_only("SESSION_PATH", &format!("data/{name}/userbot.session"));
        smm_key = tenant_only("SMMMAIN_API_KEY", "");
        adsqora_key = tenant_only("ADSQORA_API_KEY", "");
    }
    let smm_url = var(
        "SMMMAIN_API_URL",
        "SMMMAIN_API_URL",
        "https://smm.vipads.uz/api/v2",
    );
    let adsqora_url = var(
        "ADSQORA_API_URL",
        "ADSQORA_API_URL",
        "https://adsqora.vipads.uz/api",
    );
    let admin_username = tenant_only("USERNAME", name);
    let admin_password = tenant_only("PASSWORD", "");
    let (admin_username, admin_password) = if prefix.is_none() {
        (
            env::var("ADMIN_USERNAME").unwrap_or_else(|_| "Izzatillo".to_string()),
            env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "Izzatilloaka".to_string()),
        )
    } else {
        (admin_username, admin_password)
    };

    let store = Arc::new(Store::load(&state_path).await?);
    seed_telegram_from_env(&store).await?;
    migrate_legacy_session(&store, Path::new(&session_path)).await?;

    Ok(TenantState {
        name: name.to_string(),
        store,
        telegram: Arc::new(TelegramService::new(&session_path)),
        smmmain: Arc::new(SmmMainService::new(
            smm_key,
            smm_url,
            DEFAULT_SMMMAIN_SERVICE_ID,
        )),
        adsqora: Arc::new(AdsQoraService::new(adsqora_key, adsqora_url)),
        runtime: Arc::new(RwLock::new(RuntimeInfo::default())),
        rr: Arc::new(AtomicUsize::new(0)),
        admin_username,
        admin_password,
    })
}

async fn seed_telegram_from_env(store: &Store) -> Result<()> {
    let api_id = env::var("TELEGRAM_API_ID")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok());
    let api_hash = env::var("TELEGRAM_API_HASH")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let phone = env::var("TELEGRAM_PHONE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if api_id.is_none() && api_hash.is_none() && phone.is_none() {
        return Ok(());
    }

    let mut settings = store.telegram_settings().await;
    if let Some(api_id) = api_id {
        settings.api_id = Some(api_id);
    }
    if let Some(api_hash) = api_hash {
        settings.api_hash = Some(api_hash);
    }
    if let Some(phone) = phone {
        settings.phone = Some(phone);
    }

    store.update_telegram(settings).await?;
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
        .add_account(crate::models::TelegramAccount {
            id,
            label: Some(label),
            username: None,
            created_at: chrono::Utc::now(),
            last_used_at: None,
            flood_until: None,
        })
        .await?;
    tracing::info!("eski userbot sessiyasi yangi akkauntga ko'chirildi");
    Ok(())
}

fn build_router(state: AppState, static_dir: PathBuf) -> Router {
    let index = static_dir.join("index.html");
    let frontend = ServeDir::new(static_dir).not_found_service(ServeFile::new(index));

    Router::new()
        .nest("/api", api::router(state))
        .fallback_service(frontend)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("vipads_server=info,tower_http=info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}
