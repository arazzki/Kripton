# Kripton

**Living notch untuk Windows.** Kripton adalah aplikasi desktop native yang menempel di tepi atas layar, tepat di tengah, dan berubah bentuk seperti *notch* macOS: orb plasma yang bernapas saat idle, kontrol musik, lirik sinkron, timer fokus, pelacak air, rak file, to-do, serta HUD headphone dan baterai.

Dibangun **100% dengan Rust + Slint**. Tidak ada Electron, WebView2, browser scraping, Discord RPC, skrip eksternal, atau database eksternal.

> Status: **Sprint 4 — Shelf selesai**. Shell notch, Idle/Closed, halaman **Music** (WinRT SMTC + live wave FFT WASAPI), **Lyrics** (LRCLIB, lirik sinkron 580 px), dan **Shelf** (OLE drop target, carousel file/folder) sudah berjalan. Berikutnya Sprint 5: Focus, Water, dan To-do (lihat [`SPRINTS.md`](SPRINTS.md)).

![Kripton — concept art](assets/plasma_core_concept.png)

*Arah visual: notch hitam pekat dengan Ionized Plasma Core. Concept art dibuat secara prosedural dari prompt PRD.*

---

## Tampilan

| Idle / Stealth | Closed | Expanded |
| --- | --- | --- |
| Garis plasma 184 × 5 px, maskot 30 px saat wake, bernapas 0.25 Hz | Battery ring, jam 24-jam, dan 5 bar live wave di notch 336 × 34 px | Kartu 580 × 192 px: Music/Lyrics, Shelf, header battery + jam + live wave |

Notch dianimasikan di kanvas transparan 680 × 250 px; shell expanded memakai layout tetap 580 × 192 px yang di-clip oleh siluet notch.

Palet: hitam pekat `#000000`, icy cyan `#00F0FF`, emerald `#10B981`, krypton violet `#7C3AED`, dengan aksen indigo `#4F46E5`.

---

## Fitur yang direncanakan

- [x] **Foundation** — window transparan top-center, notch dengan *inverse concave fillets*, hover expand/collapse, state JSON.
- [x] **Idle** — plasma core tiga lapis bernapas 0.25 Hz, ferrofluid cursor pull, corona berputar.
- [x] **Closed** — battery ring 18 px, jam digital, live sound wave 5 batang dari FFT WASAPI loopback.
- [x] **Music** — WinRT SMTC, album art, transport, seek slider, tombol buka aplikasi sumber.
- [x] **Lyrics** — LRCLIB + lirik tersinkron, melebar 580 px.
- [x] **Shelf** — OLE drop target, carousel 5 item, thumbnail gambar, Clear, buka via ShellExecute.
- [ ] **Focus** — Pomodoro in-notch + chime.
- [ ] **Water** — target harian, gelombang cairan, pengingat.
- [ ] **To-do** — checklist cepat, strikethrough.
- [ ] **HUD** — headphone connect & charging pop-up 3 detik.
- [ ] **System** — tray icon, autostart, single instance, installer.

---

## Arsitektur

```
src/
├── main.rs           # bootstrap Slint, COM STA, timer jam/baterai, wiring callback
├── living.rs         # model maskot: breathing, ferrofluid, corona, wave, hysteresis media
├── spring.rs         # integrator pegas teredam untuk lebar/tinggi notch
├── state.rs          # Page/NotchMode, state.json atomik, normalisasi harian
├── system.rs         # GetSystemPowerStatus + geometri battery arc
├── media.rs          # worker SMTC MTA: metadata, artwork, transport, seek, buka sumber
├── lyrics.rs         # LRCLIB, parser LRC, cache RAM/disk, window sinkronisasi
├── shelf.rs          # OLE IDropTarget CF_HDROP, metadata file, thumbnail worker, persistensi
├── wave.rs           # WASAPI loopback + FFT radix-2 512 → 5 pita, atomik
└── window_ext.rs     # Win32 overlay, hover, runtime 60 fps, playback clock, callbacks

ui/
└── appwindow.slint   # shell notch, silhouette Path, idle/closed/expanded, Music/Lyrics/Shelf

assets/
├── icon.ico            # ikon executable
├── plasma_core.png     # inti Ionized Plasma Core
├── plasma_corona.png   # cincin korona yang berputar
├── plasma_aura.png     # halo violet di belakang inti
└── chime.wav           # chime timer fokus

tools/
└── generate_assets.py  # generator aset prosedural (Pillow + NumPy)
```

**Prinsip geometri**

- Kanvas native selalu **680 × 250 px** dan transparan; notch dianimasikan di dalamnya sehingga shadow/overshoot tidak terpotong.
- Siluet notch digambar sebagai satu `Path` dinamis dengan sudut bawah membulat 18 px; mode Stealth menyusut menjadi garis 184 × 5 px.
- Transisi lebar/tinggi memakai **spring teredam** (`ζ ≈ 0.72`, `ω ≈ 17 rad/s`, semi-implicit Euler). Target bisa dibalik di tengah animasi, jadi notch langsung berbalik saat kursor masuk/keluar tanpa menunggu animasi selesai.
- Window memakai `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`, selalu topmost, dan `WS_EX_TRANSPARENT` saat kursor di luar notch agar klik menembus ke aplikasi di bawahnya.
- Animasi maskot dibatasi **~20 fps** (bukan vsync 60 fps) supaya CPU idle tetap rendah; morph shell tetap berjalan tiap tick timer 16 ms.
- **SMTC** berjalan di worker MTA sendiri; UI thread tidak pernah memblokir COM. Polling 300 ms, artwork didekode di worker lalu dijadikan tekstur di UI thread.
- **Audio** dibaca dari loopback default endpoint pada worker MTA; FFT 512 titik dijalankan ~30 Hz dan hasilnya dipublikasikan lewat atomik, sehingga render tidak menunggu audio.

> Renderer: **FemtoVG**. Renderer Skia/Direct3D milik Slint saat ini merender area transparan sebagai hitam di Windows, sehingga FemtoVG dipakai untuk alpha yang benar.

---

## Kebutuhan

- Windows 10 22H2 atau Windows 11
- Rust **1.92+** dengan toolchain MSVC (`x86_64-pc-windows-msvc`)
- Visual Studio Build Tools (Windows SDK + C++ workload)
- Python 3.10+ (hanya jika ingin membuat ulang aset)

## Build & jalankan

```powershell
cargo build
cargo run
```

Build pertama mengompilasi Slint dan cukup berat. Jika RAM terbatas, batasi paralelisme:

```powershell
cargo build -j 1
```

Build rilis:

```powershell
cargo build --release
# hasil: target\release\kripton.exe
```

## Kontrol (Sprint 4)

| Aksi | Hasil |
| --- | --- |
| Media mulai diputar | Notch tertutup otomatis menampilkan battery ring, jam, dan live wave dari spectrum nyata |
| Media berhenti / dijeda (±1.6 detik) | Kembali ke Idle dengan plasma core |
| Arahkan kursor ke notch | Notch wake ke Compact Pill 360 × 36 px; klik untuk membuka shell 580 × 192 px |
| Drag file/folder dari Explorer ke notch | Halaman Shelf terbuka otomatis, item ditambahkan ke carousel |
| Double-click item Shelf | Membuka file/folder lewat `ShellExecute` |
| Klik `×` pada item | Menghapus item dari rak |
| Klik `Clear` | Mengosongkan rak dan `state.json` |
| Klik `<` / `>` | Menggeser carousel Shelf (maksimum 5 item terlihat) |
| Klik play/pause, prev, next | Mengirim perintah ke sesi SMTC aktif |
| Klik/drag seek bar | Memindahkan posisi pemutaran; waktu berjalan diinterpolasi 60 fps |
| Klik tombol `>` | Membuka aplikasi sumber (mis. Spotify) lewat AUMID |
| Jauhkan kursor (250 ms) | Notch mengecil kembali ke Idle/Closed |
| Klik notch | Toggle expand/collapse |
| Klik titik pagination | Pindah halaman (Music/Focus/Water/Shelf/To-do) |
| Klik kanan notch | Keluar dari aplikasi (sementara, sampai tray hadir) |
| Klik di luar notch | Diteruskan ke aplikasi di bawahnya (click-through) |

Hanya satu instance yang boleh berjalan; instance kedua langsung keluar. Set `KRIPTON_AUDIO_DEBUG=1` untuk menulis level FFT tiap 2 detik ke `kripton.log`.

## Data & privasi

State disimpan di:

```
%APPDATA%\Kripton\state.json
```

Log ringan (rotasi manual, tanpa telemetri):

```
%APPDATA%\Kripton\kripton.log
```

State ditulis lewat file sementara lalu `rename`, dan state korup dikarantina menjadi `state.corrupt-YYYYMMDD-HHMMSS.json`. Tidak ada data yang dikirim ke mana pun saat ini. Metadata musik dibaca lokal lewat SMTC, audio dianalisis lokal lewat loopback, dan artwork tidak pernah diunggah. Nanti, lirik hanya memanggil LRCLIB secara langsung saat mode Lyrics aktif; tidak ada analytics.

## Aset

Semua aset dibuat secara prosedural dari arah seni PRD:

```powershell
python tools/generate_assets.py
```

Menghasilkan `plasma_core.png`, `plasma_corona.png`, `plasma_aura.png`, `icon.ico`, `icon.png`, `plasma_core_concept.png`, dan `chime.wav`.

## Kualitas kode

```powershell
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

CI Windows berjalan untuk fmt, clippy, dan test pada setiap push (`.github/workflows/ci.yml`).

## Lisensi

MIT © 2026 Kripton. Lihat [`LICENSE`](LICENSE).
