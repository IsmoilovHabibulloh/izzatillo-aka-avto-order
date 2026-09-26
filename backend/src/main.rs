mod adsqora;
mod admin;
mod api;
mod models;
mod scanner;
mod smmmain;
mod store;
mod telegram;
mod tenants;

use crate::admin::AdminCredentials;
use crate::api::AppState;
use anyhow::{Context, Result};
use axum::Router;
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let static_dir = env::var("STATIC_DIR").unwrap_or_else(|_| "frontend/dist".to_string());

    // Foydalanuvchilar data/tenants.json da — admin paneldan boshqariladi. Fayl
    // hali yo'q bo'lsa, eski .env (TENANTS=..., TENANT_*) dan bir marta import qilinadi.
    let registry_path = PathBuf::from(
        env::var("TENANTS_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "data/tenants.json".to_string()),
    );
    let data_dir = registry_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("data"));
    let defaults = tenants::defaults_from_env();

    let (records, imported) = tenants::load_or_import(&registry_path).await?;
    if imported {
        tracing::info!(
            count = records.len(),
            path = %registry_path.display(),
            ".env dagi foydalanuvchilar tenants.json ga ko'chirildi"
        );
    } else if env::var("TENANTS").is_ok_and(|value| !value.trim().is_empty()) {
        tracing::info!("TENANTS env endi o'qilmaydi — foydalanuvchilar admin paneldan boshqariladi");
    }

    let mut tenant_states = Vec::with_capacity(records.len());
    for record in records {
        tenant_states.push(tenants::build_tenant(record, &defaults).await?);
    }

    let admin = AdminCredentials::from_env().map(Arc::new);
    if admin.is_none() {
        tracing::warn!("SUPERADMIN_USERNAME/SUPERADMIN_PASSWORD berilmagan — admin panelga kirib bo'lmaydi");
    }

    for tenant in &tenant_states {
        {
            let config = tenant.config.read().await;
            tracing::info!(
                tenant = %tenant.id,
                login = %config.username,
                maintenance = config.maintenance,
                "tenant yuklandi"
            );
        }
        tenants::spawn_tenant_tasks(tenant);
    }

    let app_state = AppState {
        tenants: Arc::new(RwLock::new(tenant_states)),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        admin,
        registry_path: Arc::new(registry_path),
        data_dir: Arc::new(data_dir),
        defaults: Arc::new(defaults),
        admin_lock: Arc::new(Mutex::new(())),
        login_failures: Arc::new(Mutex::new(HashMap::new())),
        password_checks: Arc::new(Semaphore::new(4)),
    };

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
