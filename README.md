# VIP Ads v1

Rust + Grammers asosidagi Telegram Ads tekshiruvchi userbot va bitta serverdan ishlaydigan MUI admin panel.

## Nima bor

- Rust `axum` backend.
- React + MUI admin panel.
- Bosh admin paneli: foydalanuvchilar, kalitlar/havolalar, profilaktika (pastda).
- Telegram userbot ulash: API ID, API hash, telefon, kod, 2FA parol.
- `contacts.getSponsoredPeers` orqali har bir key (kalit so'z) bo'yicha GLOBAL sponsored qidiruv.
- `messages.viewSponsoredMessage`, `messages.clickSponsoredMessage`, `messages.reportSponsoredMessage` chaqirilmaydi.
- Interval va keylar (kalit so'zlar) paneldan sozlanadi. Alohida "tekshiriladigan kanallar" ro'yxati yo'q — qidiruv global.
- Default interval: 5 sekund.
- Natijalar `data/state.json`da, Telegram session `data/userbot.session`da saqlanadi.

## Admin panel va foydalanuvchilar

- `SUPERADMIN_USERNAME` / `SUPERADMIN_PASSWORD` bilan kirilsa — admin panel ochiladi (oddiy login sahifasining o'zidan).
- Foydalanuvchilar `data/tenants.json` da saqlanadi (parollar argon2 xesh, fayl `0600`). Admin paneldan qo'shiladi, tahrirlanadi, o'chiriladi — restart kerak emas.
- Yangi foydalanuvchi qo'shilganda uning bazasi (`data/state-<id>.json`) va userbot sessiya papkasi (`data/<id>/`) darhol yaratiladi, skaneri ishga tushadi.
- SMM/Adsqora URL va kalitlari, "Kanal tayyorlash" havolasi, Telegram API ID/hash — har foydalanuvchiga alohida, admin paneldan.
- `tenants.json` yo'q bo'lsa, birinchi ishga tushishda eski `.env` (`TENANTS=...`, `TENANT_<NOMI>_*`) dan bir marta import qilinadi; keyin `.env` dagi `TENANT_*` o'qilmaydi.
- O'chirilgan foydalanuvchining bazasi va shaxsiy sessiya papkasi `.deleted-<vaqt>` nomiga o'tkaziladi (butunlay o'chmaydi).

### Profilaktika

Admin istalgan foydalanuvchini (yoki tanlanganlarni) profilaktikaga o'tkazadi:

- foydalanuvchiga faqat "Vaqtinchalik profilaktika" ekrani chiqadi, panelning hech bir oynasi ochilmaydi (API `423` + `code: "maintenance"` qaytaradi);
- skaner va orderlar pauzada; yarim yo'ldagi scan to'xtaydi, keylar "tekshirildi" deb belgilanmaydi;
- foydalanuvchi sozlamalari o'zgarmaydi — "Ishga qaytarish" bosilganda skaner qolgan joyidan davom etadi, ochiq panel o'zi qayta ochiladi;
- holat `tenants.json` da saqlanadi, server restartidan keyin ham saqlanib qoladi;
- admin "Panelni ochish" orqali profilaktikadagi foydalanuvchi panelini ko'ra oladi.

## Muhim izoh

Har bir key Telegram serveriga `contacts.getSponsoredPeers` orqali global qidiruv query sifatida yuboriladi. Server o'sha query bo'yicha sponsored kanallarni qaytaradi; topilgan kanal qora ro'yxatga mos kelsa SMM order yuboriladi (oq ro'yxat bo'lsa — yo'q).

Ro'yxatlar qoidasi (hamma foydalanuvchiga): oq ro'yxatda bo'lmagan har bir topilgan kanal/bot/profil avtomatik qora ro'yxatga qo'shiladi va order shu scanning o'zida ketadi. Kanal oq ro'yxatga qo'shilsa, qora ro'yxatdan chiqariladi (oq ro'yxat har doim ustun). Server ishga tushganda oldingi natijalardagi ro'yxatsiz kanallar ham qora ro'yxatga o'tkaziladi.

Bir xil reklama qayta topilsa, oldingi order holati tekshiriladi: bajarilgan bo'lsa qayta yuboriladi, bajarilmagan bo'lsa kutiladi; 10 daqiqada ham bajarilmasa baribir qayta yuboriladi.

Telegramning ichki MTProto holatlari va server tomondagi barcha hisob-kitoblarini yashirish kafolatlanmaydi. Bot faqat sponsored qidiruv natijasini oladi va ko'rildi/click/report requestlarini yubormaydi.

## Lokal ishga tushirish

```bash
cp .env.example .env
npm install --prefix frontend
npm run build --prefix frontend
cargo run -p vipads-server
```

Keyin oching:

```text
http://127.0.0.1:8080
```

## `.env`

```bash
HOST=0.0.0.0
PORT=8080
PUBLIC_DOMAIN=avto-order.vipads.uz
SUPERADMIN_USERNAME=admin
SUPERADMIN_PASSWORD=...
TENANTS_PATH=data/tenants.json
STATE_PATH=data/state.json
TELEGRAM_SESSION_PATH=data/userbot.session
STATIC_DIR=frontend/dist
```

## Serverga deploy

1. Serverga kodni joylang.
2. Node va Rust o'rnating.
3. Frontend build qiling:

```bash
npm install --prefix frontend
npm run build --prefix frontend
```

4. Backendni release build qiling:

```bash
cargo build --release -p vipads-server
```

5. Systemd service namunasi:

```ini
[Unit]
Description=VIP Ads server
After=network.target

[Service]
WorkingDirectory=/opt/vipads
EnvironmentFile=/opt/vipads/.env
ExecStart=/opt/vipads/target/release/vipads-server
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

6. Nginx reverse proxy:

```nginx
server {
    server_name avto-order.vipads.uz;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

SSL uchun odatda:

```bash
certbot --nginx -d avto-order.vipads.uz
```

## Admin panel oqimi

1. Admin sifatida kirib, foydalanuvchi qo'shing (login, parol, kalitlar, havola).
2. Foydalanuvchi o'z login/paroli bilan kiradi (yoki admin "Panelni ochish" bosadi).
3. `Userbot` tabida "QR bilan akkaunt qo'shish" — Telegram ilovasidan QR skanerlanadi (2FA bo'lsa parol so'raladi).
4. `Sozlamalar` tabida keylar (kalit so'zlar), interval va qora/oq ro'yxatni sozlang. Sozlamalar avtomatik saqlanadi.
5. `Natijalar` tabida avtomatik yoki qo'lda scan natijalarini ko'ring.

## Manbalar

- Grammers: https://codeberg.org/Lonami/grammers
- Telegram TL schema ichidagi ads metodlari: `messages.getSponsoredMessages`, `messages.viewSponsoredMessage`
