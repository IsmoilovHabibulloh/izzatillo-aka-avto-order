import { useCallback, useEffect, useRef, useState, type ChangeEvent, type ReactNode } from 'react';
import {
  Alert,
  AppBar,
  Box,
  Button,
  Checkbox,
  Chip,
  CircularProgress,
  Container,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  FormControlLabel,
  IconButton,
  InputAdornment,
  Link,
  Paper,
  Stack,
  Switch,
  TextField,
  Toolbar,
  Tooltip,
  Typography,
  useMediaQuery
} from '@mui/material';
import { alpha, useTheme } from '@mui/material/styles';
import {
  CircleCheck,
  CircleOff,
  CirclePlay,
  Copy,
  ExternalLink,
  Eye,
  EyeOff,
  LogOut,
  Pencil,
  RefreshCw,
  Save,
  ShieldCheck,
  Shuffle,
  Trash2,
  UserPlus,
  Users,
  Wrench
} from 'lucide-react';
import {
  AdminDefaults,
  AdminTenant,
  AdminTenantsResponse,
  ApiError,
  OpenPanelResponse,
  TenantCheckResponse,
  TenantUpsert,
  apiFetch
} from './api';
import { EmptyHint, FieldRow, formatDate } from './App';

type AdminPanelProps = {
  token: string;
  onLogout: () => void;
  onOpenPanel: (data: OpenPanelResponse) => void;
};

type FormTarget = { mode: 'create' } | { mode: 'edit'; tenant: AdminTenant };

type Credentials = {
  name: string;
  username: string;
  password: string;
};

type CheckState = TenantCheckResponse | 'loading';

function AdminPanel({ token, onLogout, onOpenPanel }: AdminPanelProps) {
  const [data, setData] = useState<AdminTenantsResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [busyIds, setBusyIds] = useState<string[]>([]);
  const [form, setForm] = useState<FormTarget | null>(null);
  const [maintenanceIds, setMaintenanceIds] = useState<string[] | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AdminTenant | null>(null);
  const [credentials, setCredentials] = useState<Credentials | null>(null);
  const [checks, setChecks] = useState<Record<string, CheckState>>({});

  // Root har renderda yangi funksiya beradi — poll qayta yaralmasligi uchun ref.
  const onLogoutRef = useRef(onLogout);
  useEffect(() => {
    onLogoutRef.current = onLogout;
  }, [onLogout]);

  const handleError = useCallback((err: unknown, fallback: string) => {
    if (err instanceof ApiError && err.status === 401) {
      onLogoutRef.current();
      return;
    }
    setError(err instanceof Error ? err.message : fallback);
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await apiFetch<AdminTenantsResponse>('/admin/tenants', token);
      setData(next);
      // O'chirilgan foydalanuvchilar tanlovdan chiqib ketadi.
      setSelected((current) => current.filter((id) => next.tenants.some((tenant) => tenant.id === id)));
      setError(null);
    } catch (err) {
      handleError(err, 'Yuklashda xato');
    } finally {
      setLoading(false);
    }
  }, [token, handleError]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const id = window.setInterval(() => void refresh(), 15_000);
    return () => window.clearInterval(id);
  }, [refresh]);

  const tenants = data?.tenants ?? [];
  const maintenanceCount = tenants.filter((tenant) => tenant.maintenance).length;
  const allSelected = tenants.length > 0 && selected.length === tenants.length;

  const withBusy = async (ids: string[], action: () => Promise<void>) => {
    setBusyIds((current) => [...current, ...ids]);
    try {
      await action();
    } finally {
      setBusyIds((current) => current.filter((id) => !ids.includes(id)));
    }
  };

  const setMaintenance = (ids: string[], enabled: boolean, message?: string) =>
    withBusy(ids, async () => {
      setError(null);
      try {
        const result = await apiFetch<{ message: string }>('/admin/tenants/maintenance', token, {
          method: 'POST',
          body: JSON.stringify({ ids, enabled, message })
        });
        setNotice(result.message);
        await refresh();
      } catch (err) {
        handleError(err, "Holatni o'zgartirib bo'lmadi");
      }
    });

  const openPanel = (tenant: AdminTenant) =>
    withBusy([tenant.id], async () => {
      try {
        const result = await apiFetch<OpenPanelResponse>(
          `/admin/tenants/${encodeURIComponent(tenant.id)}/login`,
          token,
          { method: 'POST' }
        );
        onOpenPanel(result);
      } catch (err) {
        handleError(err, "Panelni ochib bo'lmadi");
      }
    });

  const runCheck = async (tenant: AdminTenant) => {
    setChecks((current) => ({ ...current, [tenant.id]: 'loading' }));
    try {
      const result = await apiFetch<TenantCheckResponse>(
        `/admin/tenants/${encodeURIComponent(tenant.id)}/check`,
        token,
        { method: 'POST' }
      );
      setChecks((current) => ({ ...current, [tenant.id]: result }));
    } catch (err) {
      setChecks((current) => {
        const next = { ...current };
        delete next[tenant.id];
        return next;
      });
      handleError(err, 'Tekshirishda xato');
    }
  };

  const logout = async () => {
    try {
      await apiFetch('/auth/logout', token, { method: 'POST' });
    } catch {
      // chiqishda xatoni jim o'tkazib yuboramiz
    }
    onLogoutRef.current();
  };

  const toggleSelected = (id: string, checked: boolean) => {
    setSelected((current) =>
      checked ? Array.from(new Set([...current, id])) : current.filter((item) => item !== id)
    );
  };

  const selectionBusy = selected.some((id) => busyIds.includes(id));

  return (
    <Box className="panel-shell" sx={{ pb: 4 }}>
      <AppBar position="sticky" elevation={0} className="top-band">
        <Toolbar sx={{ gap: 1, py: 1, minHeight: { xs: 56, sm: 64 } }}>
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography variant="h6" sx={{ lineHeight: 1.1 }}>
              VIP Ads · Admin
            </Typography>
            <Typography variant="caption" sx={{ opacity: 0.8 }} noWrap>
              Foydalanuvchilar boshqaruvi{data?.admin_username ? ` · ${data.admin_username}` : ''}
            </Typography>
          </Box>
          <Tooltip title="Yangilash">
            <span>
              <IconButton color="inherit" onClick={() => void refresh()} disabled={loading}>
                <RefreshCw size={20} />
              </IconButton>
            </span>
          </Tooltip>
          <Tooltip title="Chiqish">
            <IconButton color="inherit" onClick={() => void logout()}>
              <LogOut size={20} />
            </IconButton>
          </Tooltip>
        </Toolbar>
        <Box sx={{ px: 1.5, pb: 1.25, display: 'flex', gap: 1, flexWrap: 'wrap' }}>
          <Chip
            size="small"
            color="secondary"
            icon={<Users size={15} />}
            label={`Foydalanuvchilar: ${tenants.length}`}
            sx={{ fontWeight: 800 }}
          />
          <Chip
            size="small"
            icon={<CircleCheck size={15} />}
            label={`Ishda: ${tenants.length - maintenanceCount}`}
            sx={{ fontWeight: 800, bgcolor: '#e8f5e9' }}
          />
          <Chip
            size="small"
            icon={<Wrench size={15} />}
            label={`Profilaktikada: ${maintenanceCount}`}
            sx={{ fontWeight: 800, bgcolor: maintenanceCount ? '#fff3e0' : undefined }}
          />
        </Box>
      </AppBar>

      <Container maxWidth="xl" sx={{ py: { xs: 2, md: 3 }, px: { xs: 1.5, sm: 2, md: 3 } }}>
        <Stack spacing={2}>
          {error && (
            <Alert severity="error" onClose={() => setError(null)}>
              {error}
            </Alert>
          )}
          {notice && (
            <Alert severity="success" onClose={() => setNotice(null)}>
              {notice}
            </Alert>
          )}

          <Paper sx={{ p: { xs: 1.5, md: 2 }, borderLeft: '4px solid', borderColor: 'secondary.main' }}>
            <Stack
              direction={{ xs: 'column', lg: 'row' }}
              spacing={1.5}
              sx={{ alignItems: { xs: 'stretch', lg: 'center' }, justifyContent: 'space-between' }}
            >
              <Button
                variant="contained"
                startIcon={<UserPlus size={18} />}
                onClick={() => setForm({ mode: 'create' })}
                disabled={!data}
              >
                Yangi foydalanuvchi
              </Button>
              <Stack
                direction={{ xs: 'column', sm: 'row' }}
                spacing={1}
                sx={{ alignItems: { xs: 'stretch', sm: 'center' } }}
              >
                <FormControlLabel
                  control={
                    <Checkbox
                      checked={allSelected}
                      indeterminate={selected.length > 0 && !allSelected}
                      onChange={(event) =>
                        setSelected(event.target.checked ? tenants.map((tenant) => tenant.id) : [])
                      }
                    />
                  }
                  label={selected.length ? `${selected.length} ta tanlandi` : 'Hammasini tanlash'}
                  sx={{ mr: 1 }}
                />
                <Button
                  variant="contained"
                  color="warning"
                  startIcon={<Wrench size={18} />}
                  disabled={!selected.length || selectionBusy}
                  onClick={() => setMaintenanceIds(selected)}
                >
                  Profilaktikaga o'tkazish
                </Button>
                <Button
                  variant="outlined"
                  color="primary"
                  startIcon={<CirclePlay size={18} />}
                  disabled={!selected.length || selectionBusy}
                  onClick={() => void setMaintenance(selected, false)}
                >
                  Ishga qaytarish
                </Button>
              </Stack>
            </Stack>
          </Paper>

          {!data && loading && (
            <Box sx={{ display: 'grid', placeItems: 'center', py: 6 }}>
              <CircularProgress />
            </Box>
          )}
          {data && !tenants.length && (
            <EmptyHint text="Foydalanuvchi yo'q — «Yangi foydalanuvchi» tugmasi bilan qo'shing" />
          )}

          <Box
            sx={{
              display: 'grid',
              gridTemplateColumns: { xs: '1fr', md: '1fr 1fr', xl: 'repeat(3, 1fr)' },
              gap: { xs: 1.5, md: 2 },
              alignItems: 'start'
            }}
          >
            {tenants.map((tenant) => (
              <TenantCard
                key={tenant.id}
                tenant={tenant}
                selected={selected.includes(tenant.id)}
                busy={busyIds.includes(tenant.id)}
                check={checks[tenant.id]}
                onSelect={(checked) => toggleSelected(tenant.id, checked)}
                onMaintenance={(enabled) => {
                  if (enabled) {
                    setMaintenanceIds([tenant.id]);
                  } else {
                    void setMaintenance([tenant.id], false);
                  }
                }}
                onEdit={() => setForm({ mode: 'edit', tenant })}
                onOpen={() => void openPanel(tenant)}
                onCheck={() => void runCheck(tenant)}
                onDelete={() => setDeleteTarget(tenant)}
              />
            ))}
          </Box>
        </Stack>
      </Container>

      <TenantFormDialog
        target={form}
        defaults={data?.defaults ?? null}
        token={token}
        onClose={() => setForm(null)}
        onAuthError={(err) => handleError(err, '')}
        onSaved={(message, saved) => {
          setForm(null);
          setNotice(message);
          if (saved) setCredentials(saved);
          void refresh();
        }}
      />
      <MaintenanceDialog
        ids={maintenanceIds}
        tenants={tenants}
        defaultMessage={data?.defaults.maintenance_message ?? ''}
        onClose={() => setMaintenanceIds(null)}
        onConfirm={(message) => {
          const ids = maintenanceIds ?? [];
          setMaintenanceIds(null);
          // Bir nechta tanlanganda bo'sh matn yuborilmaydi — har kimning o'z matni qoladi.
          void setMaintenance(ids, true, ids.length > 1 && !message ? undefined : message);
        }}
      />
      <DeleteDialog
        tenant={deleteTarget}
        token={token}
        onClose={() => setDeleteTarget(null)}
        onAuthError={(err) => handleError(err, '')}
        onDeleted={(message) => {
          setDeleteTarget(null);
          setNotice(message);
          void refresh();
        }}
      />
      <CredentialsDialog credentials={credentials} onClose={() => setCredentials(null)} />
    </Box>
  );
}

function TenantCard({
  tenant,
  selected,
  busy,
  check,
  onSelect,
  onMaintenance,
  onEdit,
  onOpen,
  onCheck,
  onDelete
}: {
  tenant: AdminTenant;
  selected: boolean;
  busy: boolean;
  check?: CheckState;
  onSelect: (checked: boolean) => void;
  onMaintenance: (enabled: boolean) => void;
  onEdit: () => void;
  onOpen: () => void;
  onCheck: () => void;
  onDelete: () => void;
}) {
  const telegramReady = Boolean(tenant.telegram_api_id) && Boolean(tenant.telegram_api_hash);
  const scannerText = tenant.maintenance
    ? 'Pauzada (profilaktika)'
    : tenant.scanner_enabled
      ? tenant.scanning
        ? 'Tekshiryapti'
        : 'Yoqilgan'
      : "To'xtagan";

  return (
    <Paper
      variant="outlined"
      sx={(theme) => ({
        p: { xs: 1.5, md: 2 },
        borderLeft: `4px solid ${tenant.maintenance ? theme.palette.warning.main : theme.palette.success.main}`,
        ...(selected && {
          borderColor: theme.palette.primary.main,
          bgcolor: alpha(theme.palette.primary.main, 0.03)
        })
      })}
    >
      <Stack spacing={1.25}>
        <Stack direction="row" spacing={0.5} sx={{ alignItems: 'flex-start' }}>
          <Checkbox
            checked={selected}
            onChange={(event) => onSelect(event.target.checked)}
            sx={{ mt: -0.5, ml: -1 }}
            slotProps={{ input: { 'aria-label': `${tenant.username} tanlash` } }}
          />
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography variant="h6" className="text-clamp" sx={{ lineHeight: 1.2 }}>
              {tenant.display_name || tenant.username}
            </Typography>
            <Typography variant="caption" color="text.secondary" className="text-clamp">
              login: <strong>{tenant.username}</strong> · id: {tenant.id}
            </Typography>
          </Box>
          <Chip
            size="small"
            color={tenant.maintenance ? 'warning' : 'success'}
            icon={tenant.maintenance ? <Wrench size={14} /> : <CircleCheck size={14} />}
            label={tenant.maintenance ? 'Profilaktika' : 'Ishda'}
            sx={{ flexShrink: 0 }}
          />
        </Stack>

        <Paper
          variant="outlined"
          sx={(theme) => ({
            px: 1.5,
            py: 0.75,
            bgcolor: tenant.maintenance ? alpha(theme.palette.warning.main, 0.12) : 'transparent'
          })}
        >
          <Stack direction="row" sx={{ alignItems: 'center', justifyContent: 'space-between', gap: 1 }}>
            <Box sx={{ minWidth: 0 }}>
              <Typography sx={{ fontWeight: 800 }}>Profilaktika</Typography>
              <Typography variant="caption" color="text.secondary" className="text-clamp" sx={{ display: 'block' }}>
                {tenant.maintenance
                  ? `${formatDate(tenant.maintenance_since)} dan beri · panel yopiq, skaner pauzada`
                  : "O'chiq — panel ishlayapti"}
              </Typography>
            </Box>
            {busy ? (
              <CircularProgress size={22} sx={{ mx: 1.5 }} />
            ) : (
              <Switch
                checked={tenant.maintenance}
                color="warning"
                onChange={(event) => onMaintenance(event.target.checked)}
                slotProps={{ input: { 'aria-label': 'Profilaktika' } }}
              />
            )}
          </Stack>
          {tenant.maintenance && tenant.maintenance_message && (
            <Typography variant="caption" className="text-clamp" sx={{ display: 'block', mt: 0.5 }}>
              «{tenant.maintenance_message}»
            </Typography>
          )}
        </Paper>

        <Stack spacing={0.5}>
          <StatusRow label="SMM kalit" ok={Boolean(tenant.smm_api_key)}>
            {tenant.smm_api_key ? <SecretText value={tenant.smm_api_key} /> : 'kiritilmagan'}
          </StatusRow>
          <StatusRow label="Adsqora kalit" ok={Boolean(tenant.adsqora_api_key)}>
            {tenant.adsqora_api_key ? <SecretText value={tenant.adsqora_api_key} /> : 'kiritilmagan'}
          </StatusRow>
          <StatusRow label="Kanal tayyorlash" ok={Boolean(tenant.userbot_url)}>
            {tenant.userbot_url ? (
              <Link href={tenant.userbot_url} target="_blank" rel="noopener" variant="body2">
                havola
              </Link>
            ) : (
              'kiritilmagan'
            )}
          </StatusRow>
          <StatusRow label="Telegram API" ok={telegramReady}>
            {tenant.telegram_api_id ? `ID ${tenant.telegram_api_id}` : 'kiritilmagan'}
          </StatusRow>
          <FieldRow label="Userbotlar">
            {`${tenant.accounts_total} ta${tenant.accounts_flooded ? ` (${tenant.accounts_flooded} ta limitda)` : ''}`}
          </FieldRow>
          <FieldRow label="Skaner">{`${scannerText} · ${tenant.keywords_enabled}/${tenant.keywords_total} key`}</FieldRow>
          <FieldRow label="Oxirgi scan">{formatDate(tenant.last_run_at)}</FieldRow>
          <FieldRow label="Natija / log">{`${tenant.results_total} / ${tenant.logs_total}`}</FieldRow>
          <FieldRow label="Kirgan qurilmalar">{String(tenant.active_sessions)}</FieldRow>
          <FieldRow label="Baza">{tenant.state_path}</FieldRow>
        </Stack>

        {tenant.last_error && (
          <Alert severity="warning" sx={{ py: 0 }}>
            <span className="text-clamp">{tenant.last_error}</span>
          </Alert>
        )}

        {check && <CheckResults check={check} />}

        <Divider />
        <Stack direction="row" sx={{ gap: 1, flexWrap: 'wrap', alignItems: 'center' }}>
          <Button size="small" variant="outlined" startIcon={<Pencil size={16} />} onClick={onEdit}>
            Tahrirlash
          </Button>
          <Button
            size="small"
            variant="contained"
            color="secondary"
            startIcon={<ExternalLink size={16} />}
            onClick={onOpen}
            disabled={busy}
          >
            Panelni ochish
          </Button>
          <Button
            size="small"
            variant="outlined"
            startIcon={check === 'loading' ? <CircularProgress size={14} /> : <ShieldCheck size={16} />}
            onClick={onCheck}
            disabled={check === 'loading'}
          >
            Tekshirish
          </Button>
          <Box sx={{ flex: 1 }} />
          <Tooltip title="O'chirish">
            <IconButton color="error" onClick={onDelete} aria-label="O'chirish">
              <Trash2 size={18} />
            </IconButton>
          </Tooltip>
        </Stack>
      </Stack>
    </Paper>
  );
}

function StatusRow({ label, ok, children }: { label: string; ok: boolean; children: ReactNode }) {
  return (
    <FieldRow label={label}>
      <Stack direction="row" spacing={0.75} sx={{ alignItems: 'center', justifyContent: 'flex-end' }}>
        <Box component="span" sx={{ display: 'inline-flex', color: ok ? 'success.main' : 'error.main' }}>
          {ok ? <CircleCheck size={15} /> : <CircleOff size={15} />}
        </Box>
        {typeof children === 'string' ? (
          <Typography variant="body2" color={ok ? 'text.primary' : 'text.secondary'}>
            {children}
          </Typography>
        ) : (
          children
        )}
      </Stack>
    </FieldRow>
  );
}

// Kalitni yashirin ko'rsatadi (oxirgi 4 belgi), ko'z tugmasi bilan to'liq ochiladi.
function SecretText({ value }: { value: string }) {
  const [visible, setVisible] = useState(false);
  const masked = value.length > 4 ? `••••${value.slice(-4)}` : '••••';
  return (
    <Stack direction="row" spacing={0.25} sx={{ alignItems: 'center', minWidth: 0 }}>
      <Typography variant="body2" className="text-clamp" sx={{ fontFamily: 'monospace' }}>
        {visible ? value : masked}
      </Typography>
      <IconButton
        size="small"
        onClick={() => setVisible((current) => !current)}
        aria-label={visible ? 'Yashirish' : "Ko'rsatish"}
        sx={{ p: 0.5 }}
      >
        {visible ? <EyeOff size={14} /> : <Eye size={14} />}
      </IconButton>
    </Stack>
  );
}

function CheckResults({ check }: { check: CheckState }) {
  if (check === 'loading') {
    return (
      <Stack direction="row" spacing={1} sx={{ alignItems: 'center' }}>
        <CircularProgress size={16} />
        <Typography variant="body2" color="text.secondary">
          Kalitlar tekshirilmoqda…
        </Typography>
      </Stack>
    );
  }
  const items: [string, TenantCheckResponse['smm']][] = [
    ['SMM panel', check.smm],
    ['Adsqora', check.adsqora],
    ['Telegram', check.telegram]
  ];
  return (
    <Paper variant="outlined" sx={{ p: 1.25 }}>
      <Stack spacing={0.75}>
        {items.map(([label, item]) => (
          <Stack key={label} direction="row" spacing={1} sx={{ alignItems: 'flex-start' }}>
            <Box sx={{ display: 'inline-flex', pt: '2px', color: item.ok ? 'success.main' : 'error.main' }}>
              {item.ok ? <CircleCheck size={16} /> : <CircleOff size={16} />}
            </Box>
            <Typography variant="body2" className="text-clamp">
              <strong>{label}:</strong> {item.message}
            </Typography>
          </Stack>
        ))}
      </Stack>
    </Paper>
  );
}

function emptyForm(defaults: AdminDefaults | null): TenantUpsert {
  return {
    id: '',
    display_name: '',
    username: '',
    password: '',
    smm_api_key: '',
    smm_api_url: defaults?.smm_api_url ?? '',
    adsqora_api_key: '',
    adsqora_api_url: defaults?.adsqora_api_url ?? '',
    userbot_url: '',
    telegram_api_id: '',
    telegram_api_hash: '',
    maintenance_message: ''
  };
}

function formFromTenant(tenant: AdminTenant): TenantUpsert {
  return {
    display_name: tenant.display_name,
    username: tenant.username,
    password: '',
    smm_api_key: tenant.smm_api_key,
    smm_api_url: tenant.smm_api_url,
    adsqora_api_key: tenant.adsqora_api_key,
    adsqora_api_url: tenant.adsqora_api_url,
    userbot_url: tenant.userbot_url,
    telegram_api_id: tenant.telegram_api_id ? String(tenant.telegram_api_id) : '',
    telegram_api_hash: tenant.telegram_api_hash ?? '',
    maintenance_message: tenant.maintenance_message
  };
}

function generatePassword(length = 10) {
  // Adashtiradigan belgilar (0/o, 1/l/i) yo'q — telefonda yozish oson.
  const alphabet = 'abcdefghjkmnpqrstuvwxyz23456789';
  const bytes = new Uint32Array(length);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join('');
}

function TenantFormDialog({
  target,
  defaults,
  token,
  onClose,
  onSaved,
  onAuthError
}: {
  target: FormTarget | null;
  defaults: AdminDefaults | null;
  token: string;
  onClose: () => void;
  onSaved: (message: string, credentials: Credentials | null) => void;
  onAuthError: (err: unknown) => void;
}) {
  const theme = useTheme();
  const fullScreen = useMediaQuery(theme.breakpoints.down('sm'));
  const [values, setValues] = useState<TenantUpsert>(() => emptyForm(defaults));
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [showPassword, setShowPassword] = useState(false);

  const isCreate = target?.mode === 'create';
  const targetKey = target ? (target.mode === 'create' ? 'create' : `edit:${target.tenant.id}`) : null;

  // Forma faqat ochilganda to'ldiriladi — fondagi poll yozilayotgan qiymatlarni bosib ketmaydi.
  useEffect(() => {
    if (!target) return;
    setValues(target.mode === 'create' ? emptyForm(defaults) : formFromTenant(target.tenant));
    setFormError(null);
    setShowPassword(target.mode === 'create');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey]);

  const bind = (field: keyof TenantUpsert) => ({
    value: values[field] ?? '',
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
      setValues((current) => ({ ...current, [field]: event.target.value }))
  });

  const submit = async () => {
    if (!target) return;
    setSaving(true);
    setFormError(null);
    try {
      const body: TenantUpsert = { ...values };
      if (target.mode === 'edit') delete body.id;
      const path =
        target.mode === 'create' ? '/admin/tenants' : `/admin/tenants/${encodeURIComponent(target.tenant.id)}`;
      const saved = await apiFetch<AdminTenant>(path, token, {
        method: target.mode === 'create' ? 'POST' : 'PUT',
        body: JSON.stringify(body)
      });
      const name = saved.display_name || saved.username;
      onSaved(
        target.mode === 'create' ? `${name} qo'shildi — baza va sessiya papkasi yaratildi` : `${name} saqlandi`,
        values.password ? { name, username: saved.username, password: values.password } : null
      );
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        onAuthError(err);
        return;
      }
      setFormError(err instanceof Error ? err.message : 'Saqlashda xato');
    } finally {
      setSaving(false);
    }
  };

  const title =
    target?.mode === 'edit'
      ? `Tahrirlash: ${target.tenant.display_name || target.tenant.username}`
      : 'Yangi foydalanuvchi';

  return (
    <Dialog
      open={Boolean(target)}
      onClose={saving ? undefined : onClose}
      fullScreen={fullScreen}
      fullWidth
      maxWidth="md"
    >
      <DialogTitle>{title}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2.5}>
          {formError && <Alert severity="error">{formError}</Alert>}

          <FormSection title="Kirish ma'lumotlari" hint="Foydalanuvchi panelga shu login va parol bilan kiradi.">
            <TextField label="Ism (ko'rinadigan nom)" placeholder="Masalan: Feruz aka" {...bind('display_name')} fullWidth />
            <TextField
              label="Login"
              required
              autoCapitalize="none"
              autoComplete="off"
              {...bind('username')}
              fullWidth
            />
            <TextField
              label={isCreate ? 'Parol' : 'Yangi parol'}
              required={isCreate}
              type={showPassword ? 'text' : 'password'}
              autoComplete="new-password"
              helperText={isCreate ? 'Kamida 4 belgi' : "O'zgartirmaslik uchun bo'sh qoldiring"}
              {...bind('password')}
              slotProps={{
                input: {
                  endAdornment: (
                    <InputAdornment position="end">
                      <Tooltip title="Parol yaratish">
                        <IconButton
                          onClick={() => {
                            setValues((current) => ({ ...current, password: generatePassword() }));
                            setShowPassword(true);
                          }}
                          aria-label="Parol yaratish"
                        >
                          <Shuffle size={18} />
                        </IconButton>
                      </Tooltip>
                      <IconButton
                        edge="end"
                        onClick={() => setShowPassword((current) => !current)}
                        aria-label={showPassword ? 'Parolni yashirish' : "Parolni ko'rsatish"}
                      >
                        {showPassword ? <EyeOff size={18} /> : <Eye size={18} />}
                      </IconButton>
                    </InputAdornment>
                  )
                }
              }}
              fullWidth
            />
            {isCreate && (
              <TextField
                label="ID (ixtiyoriy)"
                autoCapitalize="none"
                autoComplete="off"
                helperText={`Bo'sh qolsa logindan yasaladi. Baza: ${defaults?.data_dir ?? 'data'}/state-<id>.json`}
                {...bind('id')}
                fullWidth
              />
            )}
          </FormSection>

          <FormSection title="SMM panel" hint="Orderlar shu panel orqali, shu kalit bilan yuboriladi.">
            <TextField label="SMM API URL" placeholder={defaults?.smm_api_url} {...bind('smm_api_url')} fullWidth />
            <SecretField label="SMM API kalit" {...bind('smm_api_key')} />
          </FormSection>

          <FormSection title="Adsqora — qora kanal bazasi" hint="«Qora kanal» tabi kanallarni shu kalit bilan qo'shadi.">
            <TextField
              label="Adsqora API URL"
              placeholder={defaults?.adsqora_api_url}
              {...bind('adsqora_api_url')}
              fullWidth
            />
            <SecretField label="Adsqora API kalit" {...bind('adsqora_api_key')} />
          </FormSection>

          <FormSection title="Havola" hint="Panelda «Kanal tayyorlash» tabi shu havolani ochadi. Bo'sh bo'lsa tab ko'rinmaydi.">
            <TextField
              label="Kanal tayyorlash havolasi"
              placeholder="https://userbot.vipads.uz/?token=..."
              {...bind('userbot_url')}
              fullWidth
              sx={{ gridColumn: '1 / -1' }}
            />
          </FormSection>

          <FormSection
            title="Telegram API"
            hint={
              defaults?.telegram_api_configured
                ? "Bo'sh qolsa serverdagi umumiy API ishlatiladi."
                : 'my.telegram.org dan olinadi. Userbot akkauntlari panelda QR bilan ulanadi.'
            }
          >
            <TextField
              label="API ID"
              autoComplete="off"
              {...bind('telegram_api_id')}
              slotProps={{ htmlInput: { inputMode: 'numeric' } }}
              fullWidth
            />
            <SecretField label="API hash" {...bind('telegram_api_hash')} />
          </FormSection>

          <FormSection title="Profilaktika matni" hint="Profilaktika yoqilganda foydalanuvchi shu matnni ko'radi. Bo'sh — standart matn.">
            <TextField
              multiline
              minRows={2}
              placeholder={defaults?.maintenance_message}
              {...bind('maintenance_message')}
              fullWidth
              sx={{ gridColumn: '1 / -1' }}
            />
          </FormSection>

          {target?.mode === 'edit' ? (
            <Typography variant="caption" color="text.secondary">
              Baza: {target.tenant.state_path} · Sessiyalar: {target.tenant.session_dir}/
            </Typography>
          ) : (
            <Alert severity="info">
              Saqlashingiz bilan foydalanuvchiga alohida baza va sessiya papkasi yaratiladi, skaneri ishga
              tushadi — serverni qayta ishga tushirish shart emas.
            </Alert>
          )}
        </Stack>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 1.5 }}>
        <Button onClick={onClose} disabled={saving}>
          Bekor qilish
        </Button>
        <Button
          variant="contained"
          onClick={() => void submit()}
          disabled={saving}
          startIcon={
            saving ? <CircularProgress size={16} color="inherit" /> : isCreate ? <UserPlus size={18} /> : <Save size={18} />
          }
        >
          {isCreate ? "Qo'shish" : 'Saqlash'}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function FormSection({ title, hint, children }: { title: string; hint?: string; children: ReactNode }) {
  return (
    <Box>
      <Typography sx={{ fontWeight: 800 }}>{title}</Typography>
      {hint && (
        <Typography variant="caption" color="text.secondary" sx={{ display: 'block', mb: 1 }}>
          {hint}
        </Typography>
      )}
      <Box
        sx={{
          display: 'grid',
          gridTemplateColumns: { xs: '1fr', sm: '1fr 1fr' },
          gap: 1.5,
          mt: hint ? 0 : 1
        }}
      >
        {children}
      </Box>
    </Box>
  );
}

function SecretField({
  label,
  value,
  onChange
}: {
  label: string;
  value: string;
  onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => void;
}) {
  const [visible, setVisible] = useState(false);
  return (
    <TextField
      label={label}
      value={value}
      onChange={onChange}
      type={visible ? 'text' : 'password'}
      autoComplete="new-password"
      fullWidth
      slotProps={{
        input: {
          endAdornment: (
            <InputAdornment position="end">
              <IconButton
                edge="end"
                onClick={() => setVisible((current) => !current)}
                aria-label={visible ? 'Yashirish' : "Ko'rsatish"}
              >
                {visible ? <EyeOff size={18} /> : <Eye size={18} />}
              </IconButton>
            </InputAdornment>
          )
        }
      }}
    />
  );
}

function MaintenanceDialog({
  ids,
  tenants,
  defaultMessage,
  onClose,
  onConfirm
}: {
  ids: string[] | null;
  tenants: AdminTenant[];
  defaultMessage: string;
  onClose: () => void;
  onConfirm: (message: string) => void;
}) {
  const [message, setMessage] = useState('');
  const key = ids ? ids.join(',') : null;

  // Bitta foydalanuvchi bo'lsa uning oldingi matnini taklif qilamiz.
  useEffect(() => {
    if (!ids) return;
    const own = ids.length === 1 ? tenants.find((tenant) => tenant.id === ids[0])?.maintenance_message : '';
    setMessage(own ?? '');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const names = tenants
    .filter((tenant) => ids?.includes(tenant.id))
    .map((tenant) => tenant.display_name || tenant.username);

  return (
    <Dialog open={Boolean(ids)} onClose={onClose} fullWidth maxWidth="sm">
      <DialogTitle>Profilaktikaga o'tkazish</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Box>
            <Typography variant="body2" color="text.secondary" sx={{ mb: 1 }}>
              Tanlanganlar ({names.length}):
            </Typography>
            <Stack direction="row" sx={{ gap: 0.75, flexWrap: 'wrap' }}>
              {names.map((name) => (
                <Chip key={name} size="small" label={name} />
              ))}
            </Stack>
          </Box>
          <Alert severity="warning">
            Panel yopiladi — foydalanuvchi faqat «Vaqtinchalik profilaktika» yozuvini ko'radi. Skaner va orderlar
            pauzaga tushadi. «Ishga qaytarish» bosilganda hammasi qolgan joyidan davom etadi.
          </Alert>
          <TextField
            label="Foydalanuvchiga ko'rinadigan matn"
            multiline
            minRows={3}
            value={message}
            onChange={(event) => setMessage(event.target.value)}
            placeholder={defaultMessage}
            helperText={
              names.length > 1
                ? "Bo'sh qolsa har kimning o'z matni (yoki standart matn) ko'rsatiladi"
                : "Bo'sh qolsa standart matn ko'rsatiladi"
            }
            fullWidth
          />
        </Stack>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 1.5 }}>
        <Button onClick={onClose}>Bekor qilish</Button>
        <Button
          variant="contained"
          color="warning"
          startIcon={<Wrench size={18} />}
          onClick={() => onConfirm(message.trim())}
        >
          Profilaktikani yoqish
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function DeleteDialog({
  tenant,
  token,
  onClose,
  onDeleted,
  onAuthError
}: {
  tenant: AdminTenant | null;
  token: string;
  onClose: () => void;
  onDeleted: (message: string) => void;
  onAuthError: (err: unknown) => void;
}) {
  const [confirmText, setConfirmText] = useState('');
  const [busy, setBusy] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  useEffect(() => {
    setConfirmText('');
    setDeleteError(null);
  }, [tenant?.id]);

  const matches = Boolean(tenant) && confirmText.trim().toLowerCase() === tenant?.username.toLowerCase();

  const submit = async () => {
    if (!tenant) return;
    setBusy(true);
    setDeleteError(null);
    try {
      const result = await apiFetch<{ message: string }>(
        `/admin/tenants/${encodeURIComponent(tenant.id)}`,
        token,
        { method: 'DELETE' }
      );
      onDeleted(result.message);
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        onAuthError(err);
        return;
      }
      setDeleteError(err instanceof Error ? err.message : "O'chirishda xato");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={Boolean(tenant)} onClose={busy ? undefined : onClose} fullWidth maxWidth="xs">
      <DialogTitle>Foydalanuvchini o'chirish</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Typography>
            <strong>{tenant?.display_name || tenant?.username}</strong> o'chirilsinmi?
          </Typography>
          <Alert severity="warning">
            U panelga kira olmaydi, skaneri to'xtaydi, Telegram akkauntlari uziladi. Bazasi va sessiyalari butunlay
            o'chmaydi — serverda arxiv nomi bilan saqlanib qoladi.
          </Alert>
          <TextField
            label={`Tasdiqlash uchun loginni yozing: ${tenant?.username ?? ''}`}
            value={confirmText}
            onChange={(event) => setConfirmText(event.target.value)}
            autoCapitalize="none"
            autoComplete="off"
            fullWidth
          />
          {deleteError && <Alert severity="error">{deleteError}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 1.5 }}>
        <Button onClick={onClose} disabled={busy}>
          Bekor qilish
        </Button>
        <Button
          variant="contained"
          color="error"
          disabled={!matches || busy}
          onClick={() => void submit()}
          startIcon={busy ? <CircularProgress size={16} color="inherit" /> : <Trash2 size={18} />}
        >
          O'chirish
        </Button>
      </DialogActions>
    </Dialog>
  );
}

function CredentialsDialog({ credentials, onClose }: { credentials: Credentials | null; onClose: () => void }) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    setCopied(false);
  }, [credentials]);

  const text = credentials
    ? `Sayt: ${window.location.origin}\nLogin: ${credentials.username}\nParol: ${credentials.password}`
    : '';

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <Dialog open={Boolean(credentials)} onClose={onClose} fullWidth maxWidth="xs">
      <DialogTitle>{credentials?.name} — kirish ma'lumotlari</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={1.5}>
          <Alert severity="info">
            Parol serverda shifrlangan saqlanadi va keyin ko'rsatilmaydi — hozir nusxa olib, foydalanuvchiga yuboring.
          </Alert>
          <Paper
            variant="outlined"
            sx={{ p: 1.5, fontFamily: 'monospace', whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}
          >
            {text}
          </Paper>
        </Stack>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 1.5 }}>
        <Button onClick={() => void copy()} startIcon={copied ? <CircleCheck size={16} /> : <Copy size={16} />}>
          {copied ? 'Nusxa olindi' : 'Nusxa olish'}
        </Button>
        <Button variant="contained" onClick={onClose}>
          Tayyor
        </Button>
      </DialogActions>
    </Dialog>
  );
}

export default AdminPanel;
