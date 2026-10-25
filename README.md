# 🏝️ Kripton — Native Windows Dynamic Island in Rust

Kripton adalah aplikasi desktop *living notch / dynamic island* untuk Windows 10 & 11 yang dibangun **100% menggunakan Rust dan Slint UI**. Didesain dengan filosofi performa ultra-ringan (**< 35 MB RAM**, **0% CPU saat idle**, dan tanpa browser/webview).

---

## 🚀 Fitur Utama
1. **Morphing Dynamic Notch:** Transisi animasi native (Stealth Edge `5px` $\to$ Compact Pill $\to$ Expanded HUD).
2. **Universal Media Controller:** Terhubung ke Spotify, Apple Music, YouTube di browser lewat Windows Media Transport Controls (WinRT SMTC).
3. **Procedural Waveform:** Visualizer musik ritmis ala iOS tanpa kalkulasi audio yang berat.
4. **Minimalist System HUDs:** Indikator pop-up volume master, status baterai, dan tombol Caps Lock.
5. **Quick Screenshot & File Shelf:** Menampung tangkapan layar otomatis (`Win + Shift + S`) dan file drag-and-drop.

---

## 🛠️ Stack Teknologi
- **Bahasa:** Rust (Edition 2021)
- **UI Framework:** Slint v1.18 (GPU Direct Renderer via Direct3D/FemtoVG)
- **Windows APIs:** Crate `windows` (Win32 + WinRT)

---

## 🏃 Cara Menjalankan

Masuk ke folder `E:\Project\Kripton`:
```bash
cargo run
```

Untuk mengompilasi versi rilis yang sangat teroptimasi dan berukuran kecil:
```bash
cargo build --release
```
File executable rilis akan berada di `target/release/kripton.exe`.

---

## 📋 Dokumentasi Spesifikasi Lengkap
Lihat file [PRD.md](./PRD.md) untuk rincian arsitektur teknis dan roadmap milestone.
