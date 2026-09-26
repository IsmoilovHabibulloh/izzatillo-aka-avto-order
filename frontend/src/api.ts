export type KeywordRule = {
  text: string;
  interval_seconds: number;
  order_quantity: number;
  service_id: number;
  enabled: boolean;
  last_checked_at?: string | null;
  next_check_at?: string | null;
};

export type Settings = {
  enabled: boolean;
  interval_seconds: number;
  keywords: string[];
  keyword_rules: KeywordRule[];
  channels: string[];
  blacklist_channels: string[];
  whitelist_channels: string[];
  order_quantity: number;
  max_results: number;
};

export type TelegramSettings = {
  api_id?: number | null;
  api_hash?: string | null;
  phone?: string | null;
};

export type RuntimeStatus = {
  telegram_connected: boolean;
  login_waiting_for?: string | null;
  scanning: boolean;
  last_run_at?: string | null;
  next_run_at?: string | null;
  last_error?: string | null;
  total_results: number;
  total_logs: number;
};

export type AdResult = {
  id: string;
  fingerprint: string;
  channel: string;
  channel_title?: string | null;
  target_channel?: string | null;
  matched_keywords: string[];
  title: string;
  message: string;
  url: string;
  button_text: string;
  sponsor_info?: string | null;
  additional_info?: string | null;
  recommended: boolean;
  random_id_hex: string;
  found_at: string;
};

export type PanelLog = {
  id: string;
  created_at: string;
  level: string;
  title: string;
  message: string;
  keyword?: string | null;
  source_channel?: string | null;
  target_channel?: string | null;
  ad_url?: string | null;
  order_link?: string | null;
  quantity?: number | null;
  service_id?: number | null;
  order_id?: string | null;
  raw_response?: string | null;
};

export type SmmBalance = {
  configured: boolean;
  balance?: string | null;
  currency?: string | null;
  error?: string | null;
  checked_at: string;
};

export type AccountStatus = {
  id: string;
  label?: string | null;
  username?: string | null;
  connected: boolean;
  flooded: boolean;
  flood_until?: string | null;
  created_at: string;
  last_used_at?: string | null;
};

export type ChannelSegment = {
  channel: string;
  title?: string | null;
  whitelisted: boolean;
  count: number;
  percent: number;
};

export type KeywordStat = {
  keyword: string;
  total: number;
  whitelist_percent: number;
  order_percent: number;
  segments: ChannelSegment[];
};

export type Dashboard = {
  settings: Settings;
  telegram: TelegramSettings;
  smm_balance: SmmBalance;
  status: RuntimeStatus;
  results: AdResult[];
  logs: PanelLog[];
  accounts: AccountStatus[];
  stats_24h: KeywordStat[];
  userbot_url: string;
  display_name: string;
  maintenance: boolean;
};

export type Role = 'admin' | 'tenant';

export type LoginResponse = {
  token: string;
  role: Role;
};

export type MeResponse = {
  role: Role;
  username: string;
  tenant_id?: string | null;
  display_name?: string | null;
  via_admin: boolean;
  maintenance: boolean;
  maintenance_message?: string | null;
  maintenance_since?: string | null;
};

export type AdminTenant = {
  id: string;
  display_name: string;
  username: string;
  smm_api_key: string;
  smm_api_url: string;
  adsqora_api_key: string;
  adsqora_api_url: string;
  userbot_url: string;
  telegram_api_id?: number | null;
  telegram_api_hash?: string | null;
  maintenance: boolean;
  maintenance_message: string;
  maintenance_since?: string | null;
  state_path: string;
  session_dir: string;
  created_at: string;
  updated_at: string;
  scanner_enabled: boolean;
  scanning: boolean;
  last_run_at?: string | null;
  last_error?: string | null;
  keywords_total: number;
  keywords_enabled: number;
  accounts_total: number;
  accounts_flooded: number;
  results_total: number;
  logs_total: number;
  active_sessions: number;
};

export type AdminDefaults = {
  smm_api_url: string;
  adsqora_api_url: string;
  telegram_api_configured: boolean;
  maintenance_message: string;
  data_dir: string;
};

export type AdminTenantsResponse = {
  admin_username: string;
  tenants: AdminTenant[];
  defaults: AdminDefaults;
};

// Qo'shish/tahrirlash formasi (tahrirlashda bo'sh parol — o'zgarmaydi).
export type TenantUpsert = {
  id?: string;
  display_name: string;
  username: string;
  password: string;
  smm_api_key: string;
  smm_api_url: string;
  adsqora_api_key: string;
  adsqora_api_url: string;
  userbot_url: string;
  telegram_api_id: string;
  telegram_api_hash: string;
  maintenance_message: string;
};

export type TenantCheckItem = {
  ok: boolean;
  message: string;
};

export type TenantCheckResponse = {
  smm: TenantCheckItem;
  adsqora: TenantCheckItem;
  telegram: TenantCheckItem;
};

export type OpenPanelResponse = {
  token: string;
  tenant_id: string;
  display_name: string;
  maintenance: boolean;
};

export type QrStartResponse = {
  account_id: string;
  qr_url: string;
  expires_at: string;
};

export type QrPollResponse = {
  account_id: string;
  status: 'waiting' | 'password' | 'connected' | 'error';
  qr_url?: string | null;
  expires_at?: string | null;
  message: string;
};

export type ScanResponse = {
  added: number;
  checked_channels: number;
  checked_keywords: number;
  message: string;
};

export class ApiError extends Error {
  status: number;
  // Serverning mashina uchun kodi, masalan 'maintenance' (profilaktika).
  code: string | null;
  constructor(message: string, status: number, code: string | null = null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

export function isMaintenanceError(err: unknown): err is ApiError {
  return err instanceof ApiError && err.code === 'maintenance';
}

export async function apiFetch<T>(
  path: string,
  token: string | null,
  init: RequestInit = {}
): Promise<T> {
  const headers = new Headers(init.headers);
  if (token) {
    headers.set('Authorization', `Bearer ${token}`);
  }
  if (init.body && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  // 30s timeout: backend javob bermay qolsa so'rov abadiy osilib qolmasin —
  // UI xatoni ko'rsatadi va autosave himoyasi ishlashda davom etadi.
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), 30_000);

  let response: Response;
  try {
    response = await fetch(`/api${path}`, { ...init, headers, signal: controller.signal });
  } catch {
    throw new ApiError("Serverga ulanib bo'lmadi yoki javob kutish vaqti tugadi", 0);
  } finally {
    window.clearTimeout(timer);
  }

  const raw = await response.text();
  let data: unknown = null;
  if (raw) {
    try {
      data = JSON.parse(raw);
    } catch {
      // JSON bo'lmagan javob (masalan proxy HTML xatosi) — null qoldiramiz.
      data = null;
    }
  }

  if (!response.ok) {
    const body = data && typeof data === 'object' ? (data as { error?: unknown; code?: unknown }) : null;
    const message = body && 'error' in body ? String(body.error) : `HTTP ${response.status}`;
    const code = body && typeof body.code === 'string' ? body.code : null;
    throw new ApiError(message, response.status, code);
  }

  return data as T;
}
