import { Box, Button, CircularProgress, Paper, Stack, Typography } from '@mui/material';
import { alpha } from '@mui/material/styles';
import { LogOut, Wrench } from 'lucide-react';

// Profilaktika paytida foydalanuvchiga faqat shu ekran ko'rinadi: panelning
// hech bir oynasi ochilmaydi. Admin ish holatiga qaytarishi bilan panel
// avtomatik qayta ochiladi (App fonda tekshirib turadi).
function MaintenanceScreen({ message, onLogout }: { message: string; onLogout: () => void }) {
  return (
    <Box className="panel-shell" sx={{ minHeight: '100vh', display: 'grid', placeItems: 'center', p: 2 }}>
      <Paper sx={{ width: '100%', maxWidth: 480, p: { xs: 3, sm: 4 }, borderTop: '4px solid #FFC107', textAlign: 'center' }}>
        <Stack spacing={2.5} sx={{ alignItems: 'center' }}>
          <Box
            sx={{
              width: 76,
              height: 76,
              borderRadius: '50%',
              display: 'grid',
              placeItems: 'center',
              color: 'primary.main',
              bgcolor: (theme) => alpha(theme.palette.secondary.main, 0.28)
            }}
          >
            <Wrench size={36} />
          </Box>
          <Box>
            <Typography variant="h5" color="primary" sx={{ mb: 1 }}>
              Vaqtinchalik profilaktika
            </Typography>
            <Typography color="text.secondary" sx={{ whiteSpace: 'pre-line' }}>
              {message}
            </Typography>
          </Box>
          <Stack direction="row" spacing={1} sx={{ alignItems: 'center' }}>
            <CircularProgress size={14} />
            <Typography variant="caption" color="text.secondary">
              Profilaktika tugashi bilan panel o'zi ochiladi
            </Typography>
          </Stack>
          <Button variant="text" color="inherit" onClick={onLogout} startIcon={<LogOut size={16} />}>
            Chiqish
          </Button>
        </Stack>
      </Paper>
    </Box>
  );
}

export default MaintenanceScreen;
