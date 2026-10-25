# 🏝️ Kripton — Native Windows Dynamic Island in Rust

Kripton adalah aplikasi desktop *living notch / dynamic island* untuk Windows 10 & 11 yang dibangun **100% menggunakan Rust murni dan Slint UI**. Didesain dengan filosofi performa ultra-ringan (**< 35 MB RAM**, **0.0% CPU saat idle**, single native process, tanpa browser/webview).

---

## 🚀 Fitur Utama

1. **Morphing Dynamic Notch (3 Mode Tampilan):**
   - **Stealth Edge (`180 × 8 px`):** Mode tersembunyi ultra-tipis saat sistem idle.
   - **Compact Pill (`340 × 38 px`):** Aktif saat lagu berputar, kursor menyentuh wake-strip, atau sistem mengirimkan notifikasi HUD. Menampilkan siluet geometris **K**, cover album mini, visualizer audio, dan jam aktif.
   - **Expanded HUD (`460 × 156 px`):** Terbuka saat pulau diklik, menyediakan kontrol multimedia lengkap, slider durasi lagu, dan tombol aksi shelf.

2. **Universal Media Controller (WinRT SMTC):**
   - Terhubung secara *event-driven* ke Spotify, Apple Music, YouTube di Chrome/Edge/Brave, VLC, dll.
   - Kontrol Play / Pause, Next, dan Previous responsif tanpa blocking UI.
   - Dekode album art cover PNG/JPEG secara native dari stream Windows Runtime.

3. **Procedural Sine-Wave Visualizer:**
   - 4-bar equalizer ritmis dinamis berbasis osilasi gelombang sinus tanpa beban CPU (otomatis mendatar saat lagu dijeda).

4. **System HUD Sensor (WASAPI & Power):**
   - **Master Volume HUD:** Mendeteksi perubahan volume master Windows via `IAudioEndpointVolumeCallback` dan memunculkan pill volume interaktif dengan auto-dismiss.
   - **Battery & Charger Alert:** Mendeteksi status colok/cabut kabel charger (`⚡ Charging` / `🔋 On Battery`) dan peringatan baterai rendah.

5. **Quick Screenshot & File Shelf (`Win + Shift + S`):**
   - Mendeteksi tangkapan layar baru dari clipboard sequence secara instan.
   - Otomatis menyimpan file PNG ke direktori `Pictures\Screenshots\Kripton`.
   - Pratinjau thumbnail langsung di pulau dengan tombol aksi **Buka File (`👁`)**, **Salin Path (`📋`)**, dan **Tutup (`✕`)**.

6. **Windows Taskbar System Tray:**
   - Ikon tray di taskbar notifikasi Windows (`Shell_NotifyIconW`).
   - Menu klik kanan: Toggle Island, Buka Screenshots Shelf, About Kripton, dan Keluar.
   - Klik kiri ikon tray untuk langsung membuka/menutup Dynamic Island.
   - Bebas dari jendela konsol cmd hitam di latar belakang (`#![windows_subsystem = "windows"]`).

---

## 🛠️ Stack Teknologi

- **Bahasa:** Rust (Edition 2021)
- **UI Framework:** Slint v1.18 (GPU Direct Renderer via Direct3D/FemtoVG)
- **Windows APIs:** Crate `windows` v0.62 (Win32 Core Audio, Power, Shell Tray, Media Control SMTC)
- **Image Processing:** `image` v0.25 (Fast PNG/JPEG RGBA decoding & thumbnailing)
- **Clipboard:** `arboard` v3.7 (Native clipboard image extraction)

---

## 🏃 Cara Menjalankan

Jalankan langsung melalui Cargo:
```bash
cargo run
```

Atau jalankan file executable hasil kompilasi:
```bash
.\target\debug\kripton.exe
```

---

## 📋 Dokumentasi Spesifikasi Lengkap

Lihat file [PRD.md](./PRD.md) untuk detail arsitektur teknis dan status penyelesaian roadmap seluruh milestone (Sprint 1 s.d. Sprint 5 Selesai ✅).
