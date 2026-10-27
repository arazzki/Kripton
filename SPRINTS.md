# Rencana Sprint — Kripton v2

Acuan: [`PRD.md`](PRD.md). Setiap sprint berakhir dengan `cargo fmt`, `cargo clippy`, build/smoke test, satu commit, lalu push ke `feat/prd-v2`. Merge ke `main` hanya setelah review pengguna.

| # | Sprint | Fokus | Status |
| --- | --- | --- | --- |
| 0 | Foundation | Shell native, state, geometri notch, transparansi, hover | ✅ Selesai |
| 1 | Idle + Closed | Plasma core hidup, closed bar, spring physics | ✅ Selesai |
| 2 | Music | WinRT SMTC, transport, seek, live wave WASAPI | ✅ Selesai |
| 3 | Lyrics | LRCLIB, lirik sinkron, mode lebar 580 px | ✅ Selesai |
| 4 | Shelf | Drag & drop OLE, carousel, preview file | ✅ Selesai |
| 5 | Productivity | Focus timer, Water, To-do | ⏳ Berikutnya |
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

## Sprint 1 — Idle + Closed ✅

**Tujuan:** menghidupkan maskot dan membuat notch tertutup benar-benar informatif.

**Selesai:**

1. **Plasma core tiga lapis** — `plasma_core.png` (inti), `plasma_corona.png` (cincin berputar 9°/s), `plasma_aura.png` (halo violet). Breathing 0.25 Hz dengan fase aura tertinggal 0.55 rad agar terasa cair, bukan tiga gambar yang berdenyut serempak.
2. **Ferrofluid pull** — `Δp = u · Lmax(1 − e^(−d/λ))` dengan `Lmax = 4.2 px`, `λ = 140 px`, fade halus di tepi 320 px. Posisi orb mengikuti kursor dengan exponential smoothing, tidak pernah keluar dari notch.
3. **Closed mode** — notch 336 × 34 px (Compact Pill): battery ring 18 px (`#34D399`/`#22C55E` di atas `#064E3B`), jam 24-jam 15 px bold, 5 pill bar 2.5 px. Desktop tanpa baterai menampilkan ikon AC.
4. **Battery reader** — `GetSystemPowerStatus`, polling 30 detik, arc dihitung di Rust (`battery_arc`) supaya geometri bisa diuji tanpa Win32.
5. **Spring** — `src/spring.rs`: semi-implicit Euler, `ζ = 0.72`, `ω = 17 rad/s`, settle < 400 ms, mendukung retarget mid-flight. Menggantikan `cubic-bezier` di Slint.
6. **Idle/Closed switching** — `IAudioMeterInformation::GetPeakValue` pada default render endpoint, hysteresis 0,25 detik aktif / 1,6 detik hening. Ambang 0,03 (≈ −30 dBFS) karena endpoint nyata punya noise floor sampai ~0,017. Mode tidak dipersistensi: setiap start selalu Idle sampai audio benar-benar terdeteksi.
7. **Runtime loop** — timer 16 ms untuk spring + hover; properti maskot diperbarui ~20 fps; sampling audio ~10 Hz. Shell expanded memakai layout tetap 580 × 192 px yang di-clip oleh notch agar teks tidak reflow saat morph.
8. **COM STA** diinisialisasi di `main.rs` sebagai persiapan SMTC (Sprint 2) dan OLE drag/drop (Sprint 4).

**Acceptance terverifikasi:**

- Orb bernapas halus; screenshot Idle, Closed (audio diputar), dan Expanded diambil di Windows 1920 × 1080 @150%.
- Kursor menarik plasma tanpa membuat orb keluar dari notch (unit test `ferrofluid_pull_is_clamped`).
- Battery ring mengikuti persentase Windows; arc 0/25/50/75/100% diuji sebagai unit test.
- Audio mulai → Closed; audio berhenti → Idle setelah hysteresis.
- Ukuran kembali ke compact setelah kursor pergi; morph bisa dibalik di tengah jalan (unit test `spring_retargets_mid_flight`).
- CPU idle ≈ **1,1% total** (≈13% satu core) pada mesin uji 12 core, build debug, animasi 20 fps. Masih di atas target PRD 0–0,3%; akan ditinjau lagi di Sprint 6 (release/LTO + animasi adaptif).

**Known limitations (disengaja):**

- Live wave masih diturunkan dari satu nilai peak + fase per bar; FFT WASAPI 5 pita menyusul di Sprint 2.
- Halaman fitur di dalam shell expanded masih placeholder.
- Battery ring pada 100% hanya berupa lingkaran penuh tanpa teks persentase (sesuai PRD Closed).
- Belum ada tray; keluar lewat klik kanan.

---

## Sprint 2 — Music + Live Sound Wave ✅

**Tujuan:** kontrol media native melalui WinRT SMTC dan visualizer audio nyata.

**Selesai:**

1. **SMTC** — `src/media.rs` menjalankan worker MTA khusus (`GlobalSystemMediaTransportControlsSessionManager`) dan memblokir `IAsyncOperation` dengan executor park/unpark kecil. UI thread hanya membaca snapshot biasa, jadi tidak pernah menunggu COM.
2. **Metadata + artwork** — judul, artis, album, status, AUMID, posisi/durasi, dan tombol yang tersedia. Thumbnail dibaca lewat `IRandomAccessStreamReference` → `IBufferByteAccess`, didekode dengan `image` ke RGBA 128 px, lalu dijadikan tekstur Slint di UI thread. Track berubah → generasi naik → artwork lama dibuang.
3. **Transport & seek** — `TryTogglePlayPauseAsync`, `TrySkipNextAsync`, `TrySkipPreviousAsync`, dan `TryChangePlaybackPositionAsync`. Posisi diinterpolasi 60 fps di antara polling 250 ms, jadi angka waktu berjalan mulus tanpa spam ke SMTC.
4. **Live wave** — `src/wave.rs`: WASAPI loopback pada default render endpoint (MTA), mixdown mono (F32/I16/I32/U8, termasuk `WAVEFORMATEXTENSIBLE`), ring 512 sampel, Hann window, FFT radix-2 in-house, 5 pita log 40 Hz–16 kHz. Attack cepat / release lambat, dipublikasikan lewat atomik.
5. **Header status** — mascot, judul halaman, battery ring 16 px, tanggal + jam, dan 5 bar live di kanan atas; semua ikut terlihat saat expanded maupun Closed.
6. **Music page** — album art 64 px, judul/subjudul, tombol buka aplikasi sumber, seek bar interaktif, waktu berjalan/negatif, serta kontrol lyrics/prev/play/next.
7. **Fallback** — tanpa sesi SMTC, halaman menampilkan "Nothing playing" dan notch kembali ke Idle setelah hening 1,6 detik. Audio non-SMTC (game, pemutar tanpa SMTC) tetap menyalakan Closed lewat peak loopback.
8. **Buka sumber** — `IApplicationActivationManager::ActivateApplication(aumid)` dengan fallback `explorer.exe shell:AppsFolder\<aumid>`.

**Acceptance terverifikasi:**

- Metadata dan artwork berganti < 500 ms setelah track berubah (Spotify: "Pompeii" → "Iris" → "Don't Look Back in Anger", artwork ikut berganti).
- Seek bekerja: klik di tengah timeline memindahkan posisi Spotify dari 2,3 detik ke 119 detik. Play/pause dan next juga diverifikasi lewat SMTC.
- Tombol `>` membawa Spotify ke foreground (foreground process berubah dari OpenCode ke Spotify).
- Bar bergerak hanya saat audio berbunyi. Dengan track hening, kelima bar turun ke nol; dengan sinyal uji 5 pita, bar 0–4 bergerak 0,3–0,8.
- Hysteresis Idle/Closed tetap benar: audio berhenti → Idle; SMTC `Playing` → Closed.
- Screenshot Idle, Closed (bar aktif), dan Music expanded diambil di Windows 1920 × 1080 @150%.
- CPU build debug pada mesin 12 core: **Idle 0,74%**, **Closed + bar aktif 1,29%**, **expanded statis 0,25%** total. Working set Debug ±115 MB (target rilis <35 MB tetap di Sprint 6).

**Known limitations (disengaja):**

- Perangkat audio dicabut belum diuji secara fisik; jalur pemulihan sudah ada (capture gagal/default device berubah → reconnect 700 ms) tetapi baru diverifikasi lewat pembacaan kode.
- Bila aplikasi memutar ke endpoint yang bukan default Windows, metadatanya benar tetapi bar mengikuti endpoint default (sesuai desain PRD).
- Lirik masih tombol on/off tanpa konten; sinkronisasi LRCLIB di Sprint 3.
- Expanded shell masih memakai kartu placeholder untuk Focus/Water/Shelf/To-do.
- Aplikasi sumber dibuka tanpa membawa posisi jendela ke monitor yang sama.
- Belum ada tray; keluar lewat klik kanan.

---

## Sprint 3 — Lyrics ✅

**Tujuan:** lirik tersinkron dalam notch melebar.

**Selesai:**

1. **Integrasi LRCLIB & HTTP Client Native** — `src/lyrics.rs`: integrasi Windows native WinRT `HttpClient` (`Web_Http`), query `/api/get` (judul, artis, durasi) dengan fallback `/api/search?q=`, worker thread background tanpa memblokir thread UI.
2. **Parser LRC & Time-Sync Engine** — parsing timestamp presisi `[mm:ss.xx]` dan `[mm:ss.xxx]`, binary search window waktu lirik, auto-scroll 3 baris (baris sebelumnya, baris aktif, baris berikutnya).
3. **Cache In-Memory & Disk** — cache RAM (`HashMap`) dan cache lokal persisten JSON di folder `%APPDATA%/Kripton/lyrics_cache/`, deduplikasi request in-flight agar tidak ada query ganda untuk track yang sama.
4. **Layout Widened Split-View (580 × 192 px)** — mode `NotchMode::Lyrics` (580 × 192 px per DIMENSIONS.md):
   - Kolom kiri: compact player (album art 56 px, judul, artis, slider progress, tombol kontrol media).
   - Divider: garis vertikal 1 px `#FFFFFF1F`.
   - Kolom kanan: viewport lirik tersinkron (baris aktif putih terang 15 px bold dengan visual bullet marker, baris lain `#64748B` 13 px semibold, fallback gracefully saat offline/lirik tidak tersedia).
5. **State Persistensi & Toggle** — toggle lirik on/off tersimpan ke `state.json` via `StateStore`, animasi spring transisi halus antar ukuran notch.
6. **Ambient Glow Adaptif** — ekstraksi warna dominan dari data RGBA album art (`compute_dominant_color`), diproyeksikan sebagai pendaran radial glow di bagian bawah notch.

**Acceptance terverifikasi:**

- Parser LRC dan window sinkronisasi diverifikasi via 4 unit test (`test_parse_lrc_standard`, `test_parse_lrc_multi_timestamp`, `test_current_window`, `test_url_encode`).
- 29 unit test di seluruh proyek lulus 100% (`cargo test`).
- Bebas error dan peringatan (`cargo clippy` & `cargo fmt` bersih).
- Notch 580 px pas berada di tengah kanvas 680 px tanpa terpotong (sensor 604 × 204 px).

---

## Sprint 4 — Shelf ✅

**Tujuan:** rak file drag & drop native.

**Selesai:**

1. **OLE `IDropTarget` native** — `src/shelf.rs` mengimplementasikan `IDropTarget` dengan `#[implement]` dan mendaftarkannya ke HWND notch lewat `RegisterDragDrop` pada STA UI. `DragEnter`/`DragOver`/`Drop` memakai `FORMATETC` `CF_HDROP` + `TYMED_HGLOBAL`; callback hanya menaruh path di antrean, jadi drag dari Explorer tidak memblokir render.
2. **Auto-open Shelf** — `DragEnter` menyalakan flag `open_requested`; runtime tick berikutnya membuka halaman Shelf dan mengekspansi notch, lalu `DragLeave`/`Drop` mematikan indikator drop.
3. **Ekstraksi metadata** — `DragQueryFileW` membaca semua path, `std::fs::metadata` mengisi nama, ukuran terformat (`512 B`, `2.4 MB`, `5.00 GB`), dan status folder. Path duplikat (case-insensitive) diabaikan; rak dibatasi 20 item dan item tertua keluar lebih dulu.
4. **Carousel 54 px** — model Slint `[ShelfView]` hanya berisi jendela 5 item yang terlihat; tombol `<`/`>` 28 px menggeser `shelf-start`, tombol `Clear` mengosongkan rak. Thumbnail gambar didekode di worker `kripton-shelf-thumbs` (108 px, RGBA) agar UI tidak menunggu disk.
5. **Ikon, empty state, animasi masuk** — thumbnail asli untuk PNG/JPEG/WebP, ikon folder kuning, ikon dokumen biru, empty state yang berubah warna saat drag aktif, dan item memudar masuk lewat properti `shelf-fade` (0 → 1, 160 ms ease-out) setiap model dibangun ulang.
6. **Interaksi** — double-click item membuka file/folder lewat `ShellExecuteW`; badge `×` menghapus satu item; path yang hilang ditandai `Missing` dan dibersihkan saat startup berikutnya.
7. **Persistensi** — `state.json` menyimpan `shelf_items` (nama, path, tipe, ukuran, `size_formatted`) dan langsung ditulis ulang setiap add/remove/clear.

**Acceptance terverifikasi:**

- Drop target terdaftar sukses di HWND notch (`shelf drop target registered` di `kripton.log`).
- 33 unit test lulus 100% (`cargo test`), termasuk format ukuran, clamp carousel, deteksi ekstensi gambar, dan perbandingan path case-insensitive.
- `cargo fmt --check` dan `cargo clippy --all-targets -- -D warnings` bersih.
- Empty state, carousel, Clear, dan badge hapus dirender pada notch 580 × 192 px (screenshot smoke test).

**Known limitations:**

- Drag-out (`IDropSource`) ke aplikasi lain belum diimplementasikan; Sprint 4 hanya mencakup drop-in sesuai deliverables. Drag-out tetap kandidat Sprint 6.
- Thumbnail untuk format selain PNG/JPEG/WebP memakai ikon dokumen generik (fitur `image` yang diaktifkan hanya tiga codec tersebut).
- Cabut/pindah file saat aplikasi berjalan baru terdeteksi sebagai `Missing`; pembersihan permanen terjadi saat restart.
- Animasi keluar per item belum ada: item yang dihapus langsung hilang, lalu item tersisa memudar masuk ulang.

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
