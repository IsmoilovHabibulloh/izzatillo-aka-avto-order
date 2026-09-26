use crate::models::{
    AdResult, ChannelSegment, DEFAULT_SMMMAIN_SERVICE_ID, KeywordRule, KeywordStat,
    LEGACY_SMMMAIN_SERVICE_ID, OrderRecord, PanelLog, PersistedState, Settings, TelegramAccount,
    TelegramSettings,
};
use crate::telegram::normalize_channel_ref;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, RwLock};

pub struct Store {
    path: PathBuf,
    inner: RwLock<PersistedState>,
    /// Saqlashlarni navbatga qo'yadi: ikki parallel save bir xil `.tmp` faylga
    /// yozib, bir-birining ustidan eski holatni yozib ketmasligi uchun.
    save_lock: Mutex<()>,
    /// Tenant o'chirilgandan keyin diskka boshqa yozilmaydi.
    closed: AtomicBool,
}

/// Admin ro'yxati uchun yengil ko'rsatkichlar (natija/loglarni klonlamasdan).
#[derive(Clone, Debug, Default)]
pub struct StoreSummary {
    pub scanner_enabled: bool,
    pub keywords_total: usize,
    pub keywords_enabled: usize,
    pub accounts_total: usize,
    pub accounts_flooded: usize,
    pub results_total: usize,
    pub logs_total: usize,
    pub telegram_api_id: Option<i32>,
    pub telegram_api_hash: Option<String>,
}

impl Store {
    pub async fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("state papkasini yaratib bo'lmadi: {}", parent.display())
            })?;
        }

        let mut state = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => serde_json::from_str(&raw)
                .with_context(|| format!("state JSON buzilgan: {}", path.display()))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
            Err(err) => {
                return Err(err).with_context(|| format!("state o'qilmadi: {}", path.display()));
            }
        };
        state.settings = sanitize_settings(state.settings);
        prune_stats(&mut state);

        // Qoida eski natijalarga ham: ilgari topilgan, lekin hech bir ro'yxatda
        // yo'q kanallar ham qora ro'yxatga o'tadi.
        let targets: Vec<String> = state
            .results
            .iter()
            .filter_map(|result| result.target_channel.clone())
            .collect();
        let backfilled = blacklist_unlisted(&mut state.settings, &targets);
        if !backfilled.is_empty() {
            let mut log = PanelLog::new(
                "info",
                "Qora ro'yxatga qo'shildi",
                format!(
                    "Oq ro'yxatda bo'lmagan {} ta oldin topilgan kanal avtomatik qora ro'yxatga qo'shildi.",
                    backfilled.len()
                ),
            );
            log.target_channel = Some(
                backfilled
                    .iter()
                    .map(|channel| format!("@{channel}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            log.source_channel = Some("Avto qora ro'yxat".to_string());
            state.logs.insert(0, log);
            trim_logs(&mut state);
        }

        let store = Self {
            path,
            inner: RwLock::new(state),
            save_lock: Mutex::new(()),
            closed: AtomicBool::new(false),
        };
        if !backfilled.is_empty() {
            store.save().await?;
        }
        Ok(store)
    }

    /// Bundan keyin diskka yozishni to'xtatadi. Davom etayotgan save tugashini
    /// kutadi, shuning uchun qaytgandan keyin faylni xavfsiz ko'chirish mumkin.
    pub async fn close(&self) {
        let _guard = self.save_lock.lock().await;
        self.closed.store(true, Ordering::SeqCst);
    }

    pub async fn summary(&self, now: DateTime<Utc>) -> StoreSummary {
        let state = self.inner.read().await;
        StoreSummary {
            scanner_enabled: state.settings.enabled,
            keywords_total: state.settings.keyword_rules.len(),
            keywords_enabled: state
                .settings
                .keyword_rules
                .iter()
                .filter(|rule| rule.enabled)
                .count(),
            accounts_total: state.accounts.len(),
            accounts_flooded: state
                .accounts
                .iter()
                .filter(|account| account.flood_until.map(|until| until > now).unwrap_or(false))
                .count(),
            results_total: state.results.len(),
            logs_total: state.logs.len(),
            telegram_api_id: state.telegram.api_id,
            telegram_api_hash: state.telegram.api_hash.clone(),
        }
    }

    pub async fn snapshot(&self) -> PersistedState {
        self.inner.read().await.clone()
    }

    pub async fn settings(&self) -> Settings {
        self.inner.read().await.settings.clone()
    }

    pub async fn telegram_settings(&self) -> TelegramSettings {
        self.inner.read().await.telegram.clone()
    }

    pub async fn update_settings(&self, settings: Settings) -> Result<Settings> {
        let clean = sanitize_settings(settings);
        {
            let mut state = self.inner.write().await;
            state.settings = clean.clone();
            trim_results(&mut state);
            prune_stats(&mut state);
        }
        self.save().await?;
        Ok(clean)
    }

    pub async fn update_telegram(&self, telegram: TelegramSettings) -> Result<TelegramSettings> {
        {
            let mut state = self.inner.write().await;
            state.telegram = telegram.clone();
        }
        self.save().await?;
        Ok(telegram)
    }

    pub async fn push_results(&self, mut incoming: Vec<AdResult>) -> Result<Vec<AdResult>> {
        if incoming.is_empty() {
            return Ok(Vec::new());
        }

        let added_items = {
            let mut state = self.inner.write().await;
            let mut added_items = Vec::new();

            for item in incoming.drain(..) {
                if state.seen.insert(item.fingerprint.clone()) {
                    state.results.insert(0, item.clone());
                    added_items.push(item);
                }
            }

            trim_results(&mut state);
            added_items
        };

        if !added_items.is_empty() {
            self.save().await?;
        }

        Ok(added_items)
    }

    pub async fn clear_results(&self) -> Result<()> {
        {
            let mut state = self.inner.write().await;
            state.results.clear();
            state.seen.clear();
            state.orders.clear();
        }
        self.save().await
    }

    /// Berilgan link (key matni) uchun oxirgi order yozuvini qaytaradi.
    /// Har bir topilma uchun (kalit_so'z, kanal, sarlavha) eventlarini soatlik
    /// bucketlarga yozadi. 24 soatdan eski bucketlar shu yerda tozalanadi.
    pub async fn record_appearances(
        &self,
        events: &[(String, String, Option<String>)],
        now: DateTime<Utc>,
    ) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let hour = now.timestamp() / 3600;
        let min_hour = hour - 23;
        {
            let mut state = self.inner.write().await;
            // Scan davomida o'chirilgan key diagrammasi qaytib chiqmasin.
            let keys = rule_keys(&state.settings);
            for (keyword, channel, title) in events {
                let kw = keyword.trim().to_lowercase();
                let ch = channel.trim().to_lowercase();
                if kw.is_empty() || ch.is_empty() || !keys.contains(&kw) {
                    continue;
                }
                let channels = state.stats.entry(kw).or_default();
                let bucket = channels.entry(ch).or_default();
                if title.is_some() {
                    bucket.title = title.clone();
                }
                *bucket.hourly.entry(hour).or_insert(0) += 1;
                bucket.hourly.retain(|h, _| *h >= min_hour);
            }
            // 24 soatdan eski (bo'sh) kanallar va kalit so'zlarni tozalaymiz.
            for channels in state.stats.values_mut() {
                channels.retain(|_, bucket| {
                    bucket.hourly.retain(|h, _| *h >= min_hour);
                    !bucket.hourly.is_empty()
                });
            }
            state.stats.retain(|_, channels| !channels.is_empty());
        }
        self.save().await
    }

    /// So'nggi 24 soat statistikasi: har kalit so'z uchun kanallar ulushi (donut uchun).
    pub async fn stats_24h(&self, whitelist: &[String], now: DateTime<Utc>) -> Vec<KeywordStat> {
        let hour = now.timestamp() / 3600;
        let min_hour = hour - 23;
        let wl: HashSet<String> = whitelist
            .iter()
            .filter_map(|item| normalize_channel_ref(item))
            .collect();

        let state = self.inner.read().await;
        let mut out: Vec<KeywordStat> = Vec::new();

        for (keyword, channels) in &state.stats {
            let mut segments: Vec<ChannelSegment> = Vec::new();
            let mut total: u64 = 0;

            for (channel, bucket) in channels {
                // O'qishda ham oyna filtri: yozish siyrak bo'lsa eski bucketlar qolgan bo'lishi mumkin.
                let count: u64 = bucket
                    .hourly
                    .iter()
                    .filter(|(h, _)| **h >= min_hour)
                    .map(|(_, c)| *c)
                    .sum();
                if count == 0 {
                    continue;
                }
                total += count;
                segments.push(ChannelSegment {
                    channel: channel.clone(),
                    title: bucket.title.clone(),
                    whitelisted: wl.contains(channel),
                    count,
                    percent: 0.0,
                });
            }
            if total == 0 {
                continue;
            }

            let mut whitelist_count: u64 = 0;
            for seg in &mut segments {
                seg.percent = (seg.count as f64) * 100.0 / (total as f64);
                if seg.whitelisted {
                    whitelist_count += seg.count;
                }
            }
            segments.sort_by(|a, b| b.count.cmp(&a.count));

            let whitelist_percent = (whitelist_count as f64) * 100.0 / (total as f64);
            out.push(KeywordStat {
                keyword: keyword.clone(),
                total,
                whitelist_percent,
                order_percent: 100.0 - whitelist_percent,
                segments,
            });
        }
        out.sort_by(|a, b| a.keyword.cmp(&b.keyword));
        out
    }

    /// Topilgan kanallardan hech bir ro'yxatda yo'qlarini qora ro'yxatga qo'shadi.
    /// Joriy (eng so'nggi) ro'yxatlar bilan ishlaydi — scan davomida paneldan
    /// oq ro'yxatga qo'shilgan kanal qora ro'yxatga tushmaydi.
    pub async fn auto_blacklist(&self, targets: &[String]) -> Result<Vec<String>> {
        let added = {
            let mut state = self.inner.write().await;
            blacklist_unlisted(&mut state.settings, targets)
        };
        if !added.is_empty() {
            self.save().await?;
        }
        Ok(added)
    }

    pub async fn order_record(&self, link: &str) -> Option<OrderRecord> {
        let key = link.trim().to_lowercase();
        self.inner.read().await.orders.get(&key).cloned()
    }

    /// Order yozuvini yangilaydi va diskka saqlaydi (haqiqiy order yuborilganda).
    pub async fn upsert_order_record(&self, record: OrderRecord) -> Result<()> {
        let key = record.link.trim().to_lowercase();
        {
            let mut state = self.inner.write().await;
            state.orders.insert(key, record);
        }
        self.save().await
    }

    /// Order holatini xotirada yangilaydi (diskka yozmaydi — bu faqat kuzatuv
    /// ma'lumoti va har skanда saqlash diskni ortiqcha yuklamasligi uchun).
    pub async fn touch_order_status(
        &self,
        link: &str,
        status: Option<String>,
        checked_at: DateTime<Utc>,
    ) {
        let key = link.trim().to_lowercase();
        let mut state = self.inner.write().await;
        if let Some(record) = state.orders.get_mut(&key) {
            record.status = status;
            record.last_checked_at = Some(checked_at);
        }
    }

    pub async fn push_logs(&self, mut logs: Vec<PanelLog>) -> Result<usize> {
        if logs.is_empty() {
            return Ok(0);
        }

        let added = {
            let mut state = self.inner.write().await;
            let added = logs.len();

            while let Some(log) = logs.pop() {
                state.logs.insert(0, log);
            }

            trim_logs(&mut state);
            added
        };

        self.save().await?;
        Ok(added)
    }

    pub async fn clear_logs(&self) -> Result<()> {
        {
            let mut state = self.inner.write().await;
            state.logs.clear();
        }
        self.save().await
    }

    pub async fn mark_keywords_checked(
        &self,
        keywords: &[String],
        checked_at: DateTime<Utc>,
    ) -> Result<()> {
        if keywords.is_empty() {
            return Ok(());
        }

        let wanted = keywords
            .iter()
            .map(|keyword| keyword.trim().to_lowercase())
            .collect::<HashSet<_>>();

        let changed = {
            let mut state = self.inner.write().await;
            let mut changed = false;

            for rule in &mut state.settings.keyword_rules {
                if wanted.contains(&rule.text.trim().to_lowercase()) {
                    rule.last_checked_at = Some(checked_at);
                    rule.next_check_at =
                        Some(checked_at + Duration::seconds(rule.interval_seconds as i64));
                    changed = true;
                }
            }

            changed
        };

        if changed {
            self.save().await?;
        }

        Ok(())
    }

    pub async fn accounts(&self) -> Vec<TelegramAccount> {
        self.inner.read().await.accounts.clone()
    }

    pub async fn add_account(&self, account: TelegramAccount) -> Result<()> {
        {
            let mut state = self.inner.write().await;
            state.accounts.push(account);
        }
        self.save().await
    }

    pub async fn remove_account(&self, id: &str) -> Result<()> {
        {
            let mut state = self.inner.write().await;
            state.accounts.retain(|account| account.id != id);
        }
        self.save().await
    }

    pub async fn set_account_flood(
        &self,
        id: &str,
        until: DateTime<Utc>,
    ) -> Result<()> {
        let changed = {
            let mut state = self.inner.write().await;
            if let Some(account) = state.accounts.iter_mut().find(|a| a.id == id) {
                account.flood_until = Some(until);
                true
            } else {
                false
            }
        };
        if changed {
            self.save().await?;
        }
        Ok(())
    }

    /// Akkauntning oxirgi ishlatilgan vaqtini xotirada yangilaydi (diskka yozmaydi).
    pub async fn touch_account_used(&self, id: &str, at: DateTime<Utc>) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.iter_mut().find(|a| a.id == id) {
            account.last_used_at = Some(at);
        }
    }

    async fn save(&self) -> Result<()> {
        let _guard = self.save_lock.lock().await;
        if self.closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        let state = self.inner.read().await.clone();
        let raw = serde_json::to_vec_pretty(&state)?;
        let tmp = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp, raw)
            .await
            .with_context(|| format!("state yozilmadi: {}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .with_context(|| format!("state almashtirilmadi: {}", self.path.display()))?;
        Ok(())
    }
}

fn sanitize_settings(mut settings: Settings) -> Settings {
    settings.interval_seconds = settings.interval_seconds.clamp(2, 3600);
    settings.max_results = settings.max_results.clamp(50, 5000);
    let legacy_keywords = normalize_list(std::mem::take(&mut settings.keywords));
    settings.keyword_rules = normalize_keyword_rules(
        settings.keyword_rules,
        &legacy_keywords,
        settings.interval_seconds,
    );
    sync_legacy_keywords(&mut settings);
    settings.channels = normalize_list(settings.channels);
    settings.whitelist_channels = dedupe_channels(normalize_list(settings.whitelist_channels));
    // Kanal faqat bitta ro'yxatda turadi: oq ro'yxatga qo'shilgan kanal qora
    // ro'yxatdan chiqariladi (oq ro'yxat ustun).
    let white = channel_set(&settings.whitelist_channels);
    settings.blacklist_channels = dedupe_channels(normalize_list(settings.blacklist_channels))
        .into_iter()
        .filter(|item| normalize_channel_ref(item).is_none_or(|channel| !white.contains(&channel)))
        .collect();
    settings.order_quantity = settings.order_quantity.clamp(1, 1_000_000);
    settings
}

fn normalize_list(items: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        let cleaned = item.trim().to_string();
        if !cleaned.is_empty() && !out.iter().any(|x| x == &cleaned) {
            out.push(cleaned);
        }
    }
    out
}

/// Bir kanalning turli yozilishini ("@kanal", "https://t.me/Kanal") bitta deb
/// hisoblab, birinchisini qoldiradi.
fn dedupe_channels(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| normalize_channel_ref(item).is_none_or(|channel| seen.insert(channel)))
        .collect()
}

fn channel_set(items: &[String]) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| normalize_channel_ref(item))
        .collect()
}

/// Butun tizim qoidasi: oq ro'yxatda bo'lmagan har bir topilgan kanal/bot/profil
/// qora ro'yxatda turadi. Hali hech bir ro'yxatda bo'lmaganlarini qora ro'yxatga
/// "@username" ko'rinishida qo'shadi va yangi qo'shilganlarini qaytaradi.
fn blacklist_unlisted(settings: &mut Settings, targets: &[String]) -> Vec<String> {
    let white = channel_set(&settings.whitelist_channels);
    let mut black = channel_set(&settings.blacklist_channels);
    let mut added = Vec::new();

    for target in targets {
        let Some(channel) = normalize_channel_ref(target) else {
            continue;
        };
        // Faqat haqiqiy username (a-z, 0-9, _) — boshqa narsa ro'yxatga tushmasin.
        if !channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        if white.contains(&channel) || !black.insert(channel.clone()) {
            continue;
        }
        settings.blacklist_channels.push(format!("@{channel}"));
        added.push(channel);
    }

    added
}

fn normalize_keyword_rules(
    rules: Vec<KeywordRule>,
    legacy_keywords: &[String],
    default_interval: u64,
) -> Vec<KeywordRule> {
    let source = if rules.is_empty() {
        legacy_keywords
            .iter()
            .map(|keyword| KeywordRule::new(keyword.clone(), default_interval))
            .collect()
    } else {
        rules
    };

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for mut rule in source {
        rule.text = rule.text.trim().to_string();
        if rule.text.is_empty() {
            continue;
        }

        let key = rule.text.to_lowercase();
        if !seen.insert(key) {
            continue;
        }

        rule.interval_seconds = rule.interval_seconds.clamp(2, 86_400);
        rule.order_quantity = rule.order_quantity.clamp(1, 1_000_000);
        if rule.service_id == 0 || rule.service_id == LEGACY_SMMMAIN_SERVICE_ID {
            rule.service_id = DEFAULT_SMMMAIN_SERVICE_ID;
        }
        if rule.enabled {
            rule.next_check_at = rule.last_checked_at.map(|last_checked_at| {
                last_checked_at + Duration::seconds(rule.interval_seconds as i64)
            });
        } else {
            rule.next_check_at = None;
        }
        out.push(rule);
    }

    out
}

fn sync_legacy_keywords(settings: &mut Settings) {
    settings.keywords = settings
        .keyword_rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| rule.text.clone())
        .collect();
}

fn rule_keys(settings: &Settings) -> HashSet<String> {
    settings
        .keyword_rules
        .iter()
        .map(|rule| rule.text.trim().to_lowercase())
        .collect()
}

/// Faqat mavjud keylarning (yoqilgan yoki o'chiq) statistikasi qoladi: key
/// o'chirilsa yoki nomi o'zgartirilsa, uning diagrammasi ham o'chadi.
fn prune_stats(state: &mut PersistedState) {
    let keys = rule_keys(&state.settings);
    state.stats.retain(|keyword, _| keys.contains(keyword));
}

fn trim_results(state: &mut PersistedState) {
    let max = state.settings.max_results;
    if state.results.len() > max {
        state.results.truncate(max);
    }
}

fn trim_logs(state: &mut PersistedState) {
    const MAX_LOGS: usize = 1000;
    if state.logs.len() > MAX_LOGS {
        state.logs.truncate(MAX_LOGS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(black: &[&str], white: &[&str]) -> Settings {
        Settings {
            blacklist_channels: black.iter().map(|item| item.to_string()).collect(),
            whitelist_channels: white.iter().map(|item| item.to_string()).collect(),
            ..Settings::default()
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn whitelisting_moves_channel_out_of_blacklist() {
        let clean = sanitize_settings(settings_with(
            &["@Foo_Channel", "https://t.me/bar", "@baz"],
            &["https://t.me/foo_channel"],
        ));
        assert_eq!(clean.blacklist_channels, strings(&["https://t.me/bar", "@baz"]));
        assert_eq!(clean.whitelist_channels, strings(&["https://t.me/foo_channel"]));
    }

    #[test]
    fn one_channel_written_differently_is_kept_once() {
        let clean = sanitize_settings(settings_with(
            &["@bar", "https://t.me/Bar", "t.me/bar?start=1"],
            &[],
        ));
        assert_eq!(clean.blacklist_channels, strings(&["@bar"]));
    }

    #[test]
    fn unlisted_found_channels_go_to_blacklist() {
        let mut settings = settings_with(&["@known_black"], &["https://t.me/u1xbet_apt1"]);
        let targets = strings(&[
            "u1xbet_apt1",
            "Known_Black",
            "football_news_daily9",
            "official_onexbet_links_bot",
            "football_news_daily9",
            "bad name",
            "",
        ]);
        let added = blacklist_unlisted(&mut settings, &targets);
        assert_eq!(
            added,
            strings(&["football_news_daily9", "official_onexbet_links_bot"])
        );
        assert_eq!(
            settings.blacklist_channels,
            strings(&[
                "@known_black",
                "@football_news_daily9",
                "@official_onexbet_links_bot"
            ])
        );
        assert_eq!(settings.whitelist_channels, strings(&["https://t.me/u1xbet_apt1"]));
    }

    fn result_for(channel: &str) -> AdResult {
        AdResult {
            id: channel.to_string(),
            fingerprint: format!("1xbet:{channel}"),
            channel: channel.to_string(),
            channel_title: None,
            target_channel: Some(channel.to_string()),
            matched_keywords: vec!["1xbet".to_string()],
            title: String::new(),
            message: String::new(),
            url: format!("https://t.me/{channel}"),
            button_text: String::new(),
            sponsor_info: None,
            additional_info: None,
            recommended: false,
            random_id_hex: String::new(),
            found_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn load_blacklists_earlier_results_once() {
        let dir = std::env::temp_dir().join(format!("vipads-store-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("state.json");
        let mut state = PersistedState::default();
        state.settings = settings_with(&["@old_black"], &["https://t.me/own_channel"]);
        state.results = vec![
            result_for("own_channel"),
            result_for("old_black"),
            result_for("rival_channel"),
        ];
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(&path, serde_json::to_vec(&state).unwrap())
            .await
            .unwrap();

        let store = Store::load(&path).await.unwrap();
        let settings = store.settings().await;
        assert_eq!(
            settings.blacklist_channels,
            strings(&["@old_black", "@rival_channel"])
        );
        assert_eq!(settings.whitelist_channels, strings(&["https://t.me/own_channel"]));
        let logs = store.snapshot().await.logs;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].target_channel.as_deref(), Some("@rival_channel"));

        // Saqlangan: qayta yuklashda yana qo'shilmaydi, log takrorlanmaydi.
        drop(store);
        let again = Store::load(&path).await.unwrap();
        assert_eq!(
            again.settings().await.blacklist_channels,
            strings(&["@old_black", "@rival_channel"])
        );
        assert_eq!(again.snapshot().await.logs.len(), 1);

        let added = again
            .auto_blacklist(&strings(&["own_channel", "new_rival", "rival_channel"]))
            .await
            .unwrap();
        assert_eq!(added, strings(&["new_rival"]));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn deleted_key_loses_its_diagram() {
        let dir = std::env::temp_dir().join(format!("vipads-store-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("state.json");
        let now = Utc::now();
        let mut state = PersistedState::default();
        state.settings.keyword_rules = vec![
            KeywordRule::new("1xbet".to_string(), 5),
            KeywordRule::new("Line".to_string(), 5),
        ];
        // "1xbe" key ilgari o'chirilgan, statistikasi qolib ketgan.
        let mut stale = crate::models::ChannelBuckets::default();
        stale.hourly.insert(now.timestamp() / 3600, 3);
        state
            .stats
            .entry("1xbe".to_string())
            .or_default()
            .insert("rival".to_string(), stale);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(&path, serde_json::to_vec(&state).unwrap())
            .await
            .unwrap();

        let store = Store::load(&path).await.unwrap();
        let keywords = |stats: Vec<KeywordStat>| {
            stats.into_iter().map(|stat| stat.keyword).collect::<Vec<_>>()
        };
        assert!(store.stats_24h(&[], now).await.is_empty());

        let seen = |keyword: &str| (keyword.to_string(), "rival".to_string(), None);
        store
            .record_appearances(&[seen("1xbet"), seen("Line"), seen("1xbe")], now)
            .await
            .unwrap();
        assert_eq!(keywords(store.stats_24h(&[], now).await), strings(&["1xbet", "line"]));

        // O'chirilgan (yoqib-o'chirgich bilan) key diagrammasi qoladi.
        let mut settings = store.settings().await;
        settings.keyword_rules[1].enabled = false;
        store.update_settings(settings).await.unwrap();
        assert_eq!(keywords(store.stats_24h(&[], now).await), strings(&["1xbet", "line"]));

        // Key ro'yxatdan o'chirilsa — diagrammasi ham.
        let mut settings = store.settings().await;
        settings.keyword_rules.retain(|rule| rule.text != "Line");
        store.update_settings(settings).await.unwrap();
        assert_eq!(keywords(store.stats_24h(&[], now).await), strings(&["1xbet"]));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
