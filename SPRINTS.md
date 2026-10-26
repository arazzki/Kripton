# Rencana Sprint — Kripton v2

Acuan: [`PRD.md`](PRD.md). Setiap sprint berakhir dengan `cargo fmt`, `cargo clippy`, build/smoke test, satu commit, lalu push ke `feat/prd-v2`. Merge ke `main` hanya setelah review pengguna.

| # | Sprint | Fokus | Status |
| --- | --- | --- | --- |
| 0 | Foundation | Shell native, state, geometri notch, transparansi, hover | ✅ Selesai |
| 1 | Idle + Closed | Plasma core hidup, closed bar, spring physics | ⏳ Berikutnya |
| 2 | Music | WinRT SMTC, transport, seek, live wave WASAPI | ⏳ |
| 3 | Lyrics | LRCLIB, lirik sinkron, mode lebar 560 px | ⏳ |
| 4 | Shelf | Drag & drop OLE, carousel, preview file | ⏳ |
| 5 | Productivity | Focus timer, Water, To-do | ⏳ |
| 6 | HUD + Release | Headphone/charging HUD, tray, autostart, installer | ⏳ |

---

## Sprint 0 — Foundation ✅

**Tujuan:** kerangka aplikasi native yang benar-benar berjalan di Windows, dengan state dan interaksi dasar.

**Selesai:**

- Branch `feat/prd-v2`, PRD v2.0 sebagai acuan tunggal.
- `Cargo.toml` + `build.rs` (Slint compile, ikon executable, metadata versi).
- `src/state.rs`: `Page`, `NotchMode`, skema persisted, normalisasi harian, penyimpanan JSON atomik, kuarantina state korup, unit test.
- `src/window_ext.rs`: attach HWND, top-center, always-on-top, `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, click-through adaptif, hover 60/250 ms, single-instance mutex, sudut DWM tidak dibulatkan.
- `src/main.rs`: bootstrap, clock timer, callback expand/page/quit, logging, single instance.
- `ui/appwindow.slint`: kanvas transparan 600 × 220, siluet notch dengan telinga cekung 12 px dan sudut bawah 18 px, idle orb, shell expanded, pagination 5 halaman.
- Aset prosedural: `plasma_core.png`, `icon.ico`, `icon.png`, `plasma_core_concept.png`, `chime.wav`.
- Verifikasi: transparansi native, posisi top-center, hover expand, klik dot + persistensi, click-through.
- Renderer dipindah ke FemtoVG karena Skia/D3D merender alpha sebagai hitam.

**Known limitations (disengaja):**

- Halaman fitur masih placeholder.
- Belum ada tray; keluar lewat klik kanan.
- Battery ring dan jam di dalam notch baru kerangka.

---

## Sprint 1 — Idle + Closed

**Tujuan:** menghidupkan maskot dan membuat notch tertutup benar-benar informatif.

**Deliverables:**

1. **Plasma core** — breathing 0.25 Hz multi-layer (core/corona/aura) dengan fase berbeda; aura bereaksi terhadap kursor (ferrofluid pull, `Δp = u · Lmax(1 − e^(−d/λ))`).
2. **Ferrofluid controller** — polling kursor 60 fps, jarak Euclidean ke pusat orb, redaman saat kursor menjauh.
3. **Closed mode** — notch 210 × 38 px: battery ring 18 px (`#34D399` di atas `#064E3B`), jam 24-jam 15 px bold, 5 bar live wave placeholder (data audio menyusul di Sprint 2).
4. **Battery reader** — Win32 `GetSystemPowerStatus`, update 30 detik dan saat status berubah.
5. **Spring** — ganti animasi `cubic-bezier` dengan spring teredam ζ≈0.72 untuk lebar/tinggi (target 60 fps, tanpa jitter).
6. **Idle/Closed switching** — idle saat tidak ada aktivitas, closed saat media/timer aktif; `state.json` mengingat mode terakhir.

**Acceptance:**

- Orb terlihat bernapas halus tanpa discontinuity.
- Kursor di dekat orb menarik plasma secara natural, tidak membuat orb keluar dari notch.
- Battery ring DBG akurat terhadap pengaturan Windows.
- Ukuran kembali ke closed 210 × 38 setelah kursor pergi.
- Tidak ada alokasi per-frame yang berlebihan; CPU idle < 2%.

---

## Sprint 2 — Music + Live Sound Wave

**Tujuan:** kontrol media native melalui WinRT SMTC dan visualizer audio nyata.

**Deliverables:**

1. Integrasi `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`.
2. Polling metadata (judul, artis, album, status), artwork → `slint::Image`.
3. Tombol prev/play-pause/next, seek slider interaktif, waktu berjalan/negatif.
4. Mode Closed: 5 bar WASAPI loopback FFT (Hann N=512, 5 pita log) + attack/release smoothing.
5. Fallback aman saat tidak ada sesi media (kembali ke Idle).
6. Tombol `>` membuka aplikasi sumber via `IApplicationActivationManager`/ShellExecute.

**Acceptance:**

- Metadata dan artwork berganti < 500 ms setelah track berubah.
- Seek dan play/pause bekerja pada Spotify/browser/Apple Music.
- Bar bergerak hanya saat audio benar-benar berbunyi (tidak memantul saat hening).
- Tidak crash saat perangkat audio dicabut.

---

## Sprint 3 — Lyrics

**Tujuan:** lirik tersinkron dalam notch melebar.

**Deliverables:**

1. Query LRCLIB (`/api/get` + fallback `/api/search`) memakai judul/artis/durasi, cache in-memory + disk.
2. Parser LRC, pencocokan waktu, auto-scroll 3 baris.
3. Layout 560 × 165 px: kolom kiri compact player (album 56 px), divider 1 px, kolom kanan lirik.
4. Baris aktif putih 16 px bold; baris lain `#64748B` 14 px.
5. Tombol lirik on/off tersimpan di state; degradasi anggun saat lirik tidak tersedia.
6. Background glow mengikuti warna dominan artwork.

**Acceptance:**

- Lirik berganti tepat waktu (±150 ms) dan tetap sinkron setelah pause/resume.
- Tidak ada request berulang untuk track yang sama.
- Mode 560 px tidak terpotong di kanvas 600 px.
- Offline: tampil pesan "Lyrics unavailable", fitur lain tetap jalan.

---

## Sprint 4 — Shelf

**Tujuan:** rak file drag & drop native.

**Deliverables:**

1. OLE `IDropTarget` pada HWND notch (bukan WebView), menerima `CF_HDROP`.
2. Ekstraksi path, nama, ukuran, tipe; ikon folder/file.
3. Carousel horizontal 54 px preview, tombol `<`/`>` 28 px, tombol `Clear`.
4. Aksi klik: buka file/folder via ShellExecute; aksi hapus item dari rak.
5. Persistensi `shelf_items` (maksimum 20 item; file yang hilang ditandai).
6. Empty state dan animasi masuk/keluar item.

**Acceptance:**

- Drag file dari Explorer ke notch menambah item tanpa memblokir UI.
- Rak bertahan setelah restart; path yang hilang dibersihkan.
- Clear mengosongkan rak + state.
- Tidak ada crash saat drag banyak file sekaligus.

---

## Sprint 5 — Productivity

**Tujuan:** Focus timer, Water tracker, dan To-do yang benar-benar bisa dipakai.

**Deliverables:**

1. **Focus** — Pomodoro 25/5, mulai/pause/reset, ring progres, chime `chime.wav`, hitung sesi harian.
2. **Water** — target 2000 ml, tombol +250 ml, visual gelombang cairan, progress harian, reset tengah malam, pengingat interval.
3. **To-do** — tambah lewat input inline, checklist, strikethrough `#6B7280`, hapus, reorder drag, ID stabil.
4. Persistensi dan normalisasi harian untuk semua counter.
5. Notifikasi Windows native (tanpa toast library eksternal) atau HUD in-notch bila notifikasi dimatikan.

**Acceptance:**

- Timer tetap akurat saat notch tertutup dan saat PC sleep (koreksi waktu monotonic).
- Water reset tepat saat tanggal lokal berubah.
- To-do bertahan setelah restart; input tidak kehilangan fokus saat hover.
- Chime hanya berbunyi sekali per sesi selesai.

---

## Sprint 6 — Hardware HUD + Tray + Release

**Tujuan:** reaksi perangkat, kontrol aplikasi, dan paket distribusi.

**Deliverables:**

1. **Headphone HUD** — deteksi endpoint audio default berubah (WASAPI `IMMNotificationClient`), pill 320 × 64 px, nama perangkat + `Connected` hijau, auto-hide 3 detik.
2. **Charging HUD** — `GetSystemPowerStatus` + `WM_POWERBROADCAST`, pill 300 × 58 px, tampil 3 detik.
3. Tray icon (Slint `SystemTrayIcon` / Win32 `Shell_NotifyIcon`): show/hide, autostart, quit.
4. Autostart lewat `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
5. Settings minimal: mulai dengan Windows, aktifkan lirik, target air.
6. Release: optimasi LTO, installer MSIX/Inno Setup, signing opsional, versi 1.0.0.
7. README final + screenshot/GIF, panduan troubleshooting.

**Acceptance:**

- HUD muncul < 300 ms setelah perangkat berubah dan hilang otomatis.
- Tray mengontrol show/hide/quit; tidak ada proses zombie.
- Autostart bisa dinyalakan/dimatikan dari tray.
- `cargo build --release` menghasilkan satu `kripton.exe` yang berjalan di mesin bersih.
- Merge `feat/prd-v2` → `main` setelah review.

---

## Definition of Done (setiap sprint)

1. `cargo fmt --all --check` bersih.
2. `cargo clippy --all-targets -- -D warnings` bersih.
3. `cargo test` lulus.
4. Smoke test manual di Windows: idle, hover, halaman terkait, tanpa panic di log.
5. Tidak ada regresi pada mode sebelumnya.
6. Satu commit deskriptif, push ke `feat/prd-v2`.
7. Catatan sprint diperbarui di file ini.
