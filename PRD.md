# 🦀 Product Requirements Document (PRD)
# Project: "Kripton" (Native Windows Dynamic Island in Rust)
**Repository Path:** `E:\Project\Kripton`  
**Current Status:** Fase 3 (Selesai — System HUD Sensor WASAPI Volume & Battery Berhasil Terintegrasi & Teruji)

---

## 1. Executive Summary & Vision

**Kripton** adalah aplikasi *desktop overlay* mandiri (standalone) berperforma ultra-tinggi untuk Windows 10 & 11 yang mengadopsi konsep **Dynamic Island / Living Notch**. Kripton dibangun **100% menggunakan bahasa pemrograman Rust murni dengan Slint UI**, menggantikan arsitektur lama berbasis Electron/Node.js yang boros memori.

### Metrik Target vs Baseline Lama
| Metrik | Target Kripton (Rust + Slint) | Pembanding (Dynasis / Electron) |
| :--- | :--- | :--- |
| **Idle Memory (RAM)** | **< 35 MB** | ~450 – 550 MB |
| **Active Memory (RAM)** | **< 60 MB** | ~600 MB |
| **Idle CPU Usage** | **0.0%** (Event-driven rendering) | ~1.5 – 4% (Polling loop) |
| **Startup Time** | **< 300 ms** | 2 – 4 detik |
| **Ukuran Executable** | **Single portable `.exe`** | ~180 – 250 MB |
| **Jumlah Child Process**| **0** (Single native process) | 5 – 6 proses |

---

## 2. Non-Goals (Fitur yang Ditiadakan)

Sesuai simplifikasi arsitektur:
1. ❌ **Tidak ada WhatsApp Web Bridge / Scraping:** Menghilangkan seluruh kebutuhan browser runtime/Webview sekunder.
2. ❌ **Tidak ada Discord Integration:** Tanpa integrasi RPC Discord atau kontrol mute mikrofon pihak ketiga.
3. ❌ **Tidak ada Skrip Eksternal (PowerShell / Node.js):** Semua komunikasi sistem berjalan *in-process* melalui Win32/WinRT C-ABI.
4. ❌ **Tidak ada Database Berat:** Menggunakan konfigurasi lokal sederhana (TOML/JSON ringan).

---

## 3. Core Architecture & Tech Stack

```mermaid
graph TD
    subgraph Single Binary [kripton.exe (Rust Native)]
        WinApp[Win32 Main Event Loop / Slint Window]
        
        subgraph Subsystems
            SMTC[WinRT Media Manager<br/>GlobalSystemMediaTransportControls]
            SysHUD[Win32 System Sensors<br/>Audio Endpoint WASAPI, Battery]
            ClipShelf[Clipboard & Drop Target<br/>Win32 Clipboard Listener]
            Physics[Spring Physics Engine<br/>Slint Easing & Direct Transitions]
        end
        
        Renderer[GPU Direct Renderer<br/>Slint / FemtoVG / Direct3D]
        DWM[Windows DWM / DirectComposition<br/>Per-Pixel Alpha Transparent Window]
    end

    SMTC --> Physics
    SysHUD --> Physics
    ClipShelf --> Physics
    Physics --> Renderer
    Renderer --> DWM
```

### Dependencies Stack
- **GUI Engine:** `slint = "1.18"` & `slint-build = "1.18"`
- **Windows System & WinRT API:** `windows = "0.62"` (Fitur: `Media_Control`, `Win32_Media_Audio`, `Win32_System_Power`, `Win32_UI_WindowsAndMessaging`)
- **Image & Thumbnailing:** `image = "0.25"`
- **Clipboard & Shelf:** `arboard = "3.7"`

---

## 4. Spesifikasi Fitur Utama

### 4.1. Morphing Notch (3 Mode Tampilan)
1. **Mode 1: Stealth Edge (`180 × 8 px`)**
   - Muncul saat kursor berada jauh di luar pulau dan tidak ada musik yang aktif.
   - Area klik tembus pandang (`WS_EX_TRANSPARENT`), sehingga tab browser paling atas tetap bisa diklik bebas.
2. **Mode 2: Compact Pill (`320 × 38 px`)**
   - Aktif saat kursor menyentuh *wake-strip* teratas atau saat lagu berganti.
   - Menampilkan: Cover album mini, judul lagu, dan jam digital.
3. **Mode 3: Expanded HUD (`480 × 140 px`)**
   - Terbuka saat pulau diklik.
   - Berisi kontrol lengkap: Play/Pause, Next/Prev, Cover Art berukuran penuh, judul/artis, dan slider.

### 4.2. Universal Media Controller (WinRT SMTC)
- **Dukungan Player:** Spotify, Apple Music, YouTube / SoundCloud di browser, VLC.
- **Aksi Cepat:** Play / Pause / Next / Previous langsung lewat WinRT API.
- **Procedural Equalizer Bars:** Bar visualizer bertingkat menggunakan osilasi gelombang sinus tanpa memakan CPU.

### 4.3. Minimalist System HUDs & Overlays
- **Master Volume HUD:** Membaca event `IAudioEndpointVolumeCallback` dan memunculkan pop-up pill volume sementara.
- **Battery & Charger Alert:** Notifikasi saat kabel charger dicolokkan atau baterai lemah.
- **Caps Lock / Key Indicator:** Indikator visual saat Caps Lock aktif/mati.

### 4.4. Quick Screenshot & File Drop Shelf
- **Auto-Detect Screenshot (`Win + Shift + S`):** Clipboard listener mendeteksi tangkapan layar baru dan menyiapkannya di shelf.
- **File Drop & Drag-out:** Menampung file sementara dan memungkinkan *drag* langsung ke aplikasi lain.

---

## 5. Rencana Struktur Proyek (Directory Layout)

```
E:\Project\Kripton/
├── Cargo.toml               # Dependensi Rust (slint, windows, dll)
├── build.rs                 # Build script compiler Slint
├── PRD.md                   # Dokumen spesifikasi proyek
├── README.md                # Dokumentasi & panduan run
├── ui/                      # Template antarmuka Slint
│   └── appwindow.slint      # Layout & animasi Dynamic Island
└── src/
    ├── main.rs              # Entry point aplikasi
    ├── media/               # Modul WinRT SMTC (Fase 2)
    ├── system/              # Modul Volume & Baterai (Fase 3)
    └── shelf/               # Modul Clipboard & File Shelf (Fase 4)
```

---

## 6. Status & Milestone Roadmap

| Fase | Milestone | Status | Detail Luaran |
| :--- | :--- | :---: | :--- |
| **Fase 1** | **Scaffolding & UI Prototype** | **SELESAI ✅** | Inisialisasi Cargo, integrasi Slint 1.18, animasi morphing 3-state, dan kompilasi biner `kripton.exe` (Exit Code 0). |
| **Fase 2** | **WinRT Media Controller (SMTC)** | **SELESAI ✅** | Integrasi `windows::Media::Control` (SMTC), kontrol Play/Pause/Next/Prev, dekode album art native, procedural 4-bar equalizer, timeline progress bar, dan background event-driven worker. |
| **Fase 3** | **System HUD Sensor** | **SELESAI ✅** | Integrasi listener Windows Core Audio (WASAPI `IAudioEndpointVolumeCallback`), sensor status daya/baterai (`GetSystemPowerStatus`), pop-up pill overlay dinamis, auto-dismiss timer, dan prioritisasi HUD. |
| **Fase 4** | **Screenshot & File Shelf** | *Next Up 🎯* | Listener clipboard untuk tangkapan layar otomatis (`Win + Shift + S`) dan OLE Drag-and-drop shelf. |
| **Fase 5** | **Optimasi Rilis & Packaging** | *Pending* | Profiling memori (<35 MB), kompilasi rilis (`lto = true`, `opt-level = "z"`), dan icon tray. |
