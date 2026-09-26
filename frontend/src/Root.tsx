import { useEffect, useState } from 'react';
import { Alert, Box, Button, CircularProgress, Paper, Stack, TextField, Typography } from '@mui/material';
import { KeyRound } from 'lucide-react';
import App from './App';
import AdminPanel from './AdminPanel';
import { LoginResponse, OpenPanelResponse, apiFetch } from './api';

const TENANT_TOKEN_KEY = 'vipads_token';
const ADMIN_TOKEN_KEY = 'vipads_admin_token';
const ADMIN_VIEW_KEY = 'vipads_admin_view';
const LAST_LOGIN_KEY = 'vipads_last_login';

// Admin paneldan "Panelni ochish" orqali ochilgan foydalanuvchi paneli.
export type AdminView = {
  tenantId: string;
  displayName: string;
};

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string | null) {
  try {
    if (value === null) {
      localStorage.removeItem(key);
    } else {
      localStorage.setItem(key, value);
    }
  } catch {
    // Saqlab bo'lmadi (masalan private rejim) — sessiya sahifa yangilanguncha ishlaydi.
  }
}

function readAdminView(): AdminView | null {
  const raw = readStorage(ADMIN_VIEW_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as AdminView;
    return parsed.tenantId ? parsed : null;
  } catch {
    return null;
  }
}

function Root() {
  const [tenantToken, setTenantToken] = useState(() => readStorage(TENANT_TOKEN_KEY));
  const [adminToken, setAdminToken] = useState(() => readStorage(ADMIN_TOKEN_KEY));
  const [adminView, setAdminView] = useState<AdminView | null>(readAdminView);

  // Admin panel ↔ foydalanuvchi paneli almashganda sahifa tepadan boshlansin.
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [tenantToken, adminToken]);

  const saveTenant = (token: string | null, view: AdminView | null) => {
    writeStorage(TENANT_TOKEN_KEY, token);
    writeStorage(ADMIN_VIEW_KEY, view ? JSON.stringify(view) : null);
    setTenantToken(token);
    setAdminView(view);
  };

  const saveAdmin = (token: string | null) => {
    writeStorage(ADMIN_TOKEN_KEY, token);
    setAdminToken(token);
  };

  const handleLoggedIn = (data: LoginResponse) => {
    if (data.role === 'admin') {
      saveTenant(null, null);
      saveAdmin(data.token);
    } else {
      saveTenant(data.token, null);
    }
  };

  const openTenantPanel = (data: OpenPanelResponse) => {
    saveTenant(data.token, { tenantId: data.tenant_id, displayName: data.display_name });
  };

  const adminLogout = () => {
    saveAdmin(null);
    if (adminView) saveTenant(null, null);
  };

  if (tenantToken) {
    return (
      <App
        key={tenantToken}
        token={tenantToken}
        adminView={adminToken ? adminView : null}
        onSessionEnd={() => saveTenant(null, null)}
        onAdminSession={() => {
          // Admin tokeni foydalanuvchi joyiga yozilib qolgan — o'z joyiga ko'chiramiz.
          saveAdmin(tenantToken);
          saveTenant(null, null);
        }}
      />
    );
  }
  if (adminToken) {
    return <AdminPanel token={adminToken} onLogout={adminLogout} onOpenPanel={openTenantPanel} />;
  }
  return <LoginScreen onLoggedIn={handleLoggedIn} />;
}

function LoginScreen({ onLoggedIn }: { onLoggedIn: (data: LoginResponse) => void }) {
  const [username, setUsername] = useState(() => readStorage(LAST_LOGIN_KEY) ?? '');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleLogin = async () => {
    setBusy(true);
    setError(null);
    try {
      const data = await apiFetch<LoginResponse>('/auth/login', null, {
        method: 'POST',
        body: JSON.stringify({ username: username.trim(), password })
      });
      writeStorage(LAST_LOGIN_KEY, username.trim());
      onLoggedIn(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Login xato');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Box className="panel-shell" sx={{ minHeight: '100vh', display: 'grid', placeItems: 'center', p: 2 }}>
      <Paper sx={{ width: '100%', maxWidth: 420, p: { xs: 2.5, sm: 4 }, borderTop: '4px solid #FFC107' }}>
        <Stack spacing={2.5}>
          <Box>
            <Typography variant="h4" color="primary">
              VIP Ads
            </Typography>
            <Typography color="text.secondary">Avto order paneli</Typography>
          </Box>
          {error && <Alert severity="error">{error}</Alert>}
          <TextField
            label="Login"
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            autoComplete="username"
            autoCapitalize="none"
            fullWidth
          />
          <TextField
            label="Parol"
            type="password"
            value={password}
            onChange={(event) => setPassword(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !busy) void handleLogin();
            }}
            autoComplete="current-password"
            fullWidth
          />
          <Button
            variant="contained"
            color="primary"
            onClick={() => void handleLogin()}
            disabled={busy}
            size="large"
            startIcon={busy ? <CircularProgress size={16} color="inherit" /> : <KeyRound size={18} />}
          >
            Kirish
          </Button>
        </Stack>
      </Paper>
    </Box>
  );
}

export default Root;
