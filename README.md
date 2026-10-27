# Kripton

**Living notch untuk Windows.** Kripton adalah aplikasi desktop native yang menempel di tepi atas layar, tepat di tengah, dan berubah bentuk seperti *notch* macOS: orb plasma yang bernapas saat idle, kontrol musik, lirik sinkron, timer fokus, pelacak air, rak file, to-do, serta HUD headphone dan baterai.

Dibangun **100% dengan Rust + Slint**. Tidak ada Electron, WebView2, browser scraping, Discord RPC, skrip eksternal, atau database eksternal.

> Status: **Sprint 6 — Hardware HUD + Tray + Release selesai (v1.0.0)**. Shell notch, Idle/Closed, halaman **Music** (WinRT SMTC + live wave FFT WASAPI), **Lyrics** (LRCLIB, lirik sinkron 580 px), **Shelf** (OLE drop target, carousel file/folder), **Focus** (Pomodoro + chime), **Water** (gelombang + pengingat), **To-do** (checklist + drag reorder), **HUD headphone/charging**, dan **tray icon** sudah berjalan. Paket rilis memakai profil LTO dan script installer Inno Setup (lihat [`SPRINTS.md`](SPRINTS.md)).

![Kripton — concept art](assets/plasma_core_concept.png)

*Arah visual: notch hitam pekat dengan Ionized Plasma Core. Concept art dibuat secara prosedural dari prompt PRD.*

---

## Tampilan

| Idle / Stealth | Closed | Expanded |
| --- | --- | --- |
| Garis plasma 184 × 5 px, maskot 30 px saat wake, bernapas 0.25 Hz | Battery ring, jam 24-jam, dan 5 bar live wave di notch 336 × 34 px | Kartu 580 × 192 px: Music/Lyrics, Focus, Water, Shelf, To-do, header battery + jam + live wave |

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
- [x] **Focus** — Pomodoro 25/5, preset 15/25/45/60, ring progres, chime in-memory, sesi harian.
- [x] **Water** — target 2.000 ml, +150/+250/+500, Undo, gelombang cairan, pengingat 90 menit.
- [x] **To-do** — input inline, checklist, strikethrough, hapus, drag reorder, persistensi.
- [x] **HUD** — headphone connect/disconnect 320 × 64 px dan charging 300 × 58 px, auto-hide 3 detik.
- [x] **System** — tray icon (show/hide, autostart, lyrics, water goal, quit), single instance, installer 1.0.0.

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
├── focus.rs          # Pomodoro monotonic (GetTickCount64), fase 25/5, chime in-memory
├── water.rs          # tracker hidrasi harian, undo, format ribuan, nudge 90 menit
├── todo.rs           # checklist CRUD, reorder, ID stabil
├── hud.rs            # state machine HUD 3 detik + ambang charging
├── headphone.rs      # IMMNotificationClient MTA: endpoint default → HUD
├── tray.rs           # Shell_NotifyIconW, menu, antrean aksi ke UI thread
├── autostart.rs      # HKCU Run key read/write/remove
├── wave.rs           # WASAPI loopback + FFT radix-2 512 → 5 pita, atomik
└── window_ext.rs     # Win32 overlay, hover, runtime 60 fps, playback clock, callbacks

ui/
└── appwindow.slint   # shell notch, idle/closed/expanded, Music/Lyrics/Shelf/Focus/Water/To-do, HUD

installer/
└── kripton.iss       # Inno Setup 6, per-user, autostart task opsional

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
- **HUD hardware** memakai state machine murni (`src/hud.rs`) dengan batas 3 detik. Perubahan endpoint audio datang dari callback `IMMNotificationClient` di thread MTA terpisah dan hanya menulis snapshot; UI tick yang menampilkan HUD, sehingga audio stack tidak pernah memblokir render.
- **Tray** memakai hidden message window + `Shell_NotifyIconW` karena Slint 1.18 belum punya API tray. Menu hanya mengantre aksi; UI thread yang menerapkannya, jadi tidak ada panggilan Slint dari thread lain. Show/hide memakai `ShowWindow` pada HWND overlay agar keepalive event loop tetap hidup.
- **Charging** dipantau dengan `GetSystemPowerStatus` tiap 250 ms di tick runtime. Winit tidak mengekspos `WM_POWERBROADCAST` mentah, jadi polling ini menggantikan listener pesan power dengan latensi di bawah 300 ms.

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

Build rilis (LTO fat, `codegen-units = 1`, strip, `panic = "abort"`):

```powershell
cargo build --release
# hasil: target\release\kripton.exe (satu file, tanpa runtime tambahan)
```

Installer per-user (tanpa UAC) memakai Inno Setup 6:

```powershell
cargo build --release
& "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\kripton.iss
# hasil: dist\Kripton-1.0.0-setup.exe
```

Installer menyalin `kripton.exe`, `README.md`, dan `LICENSE` ke `%LOCALAPPDATA%\Programs\Kripton`, membuat shortcut Start Menu, dan menyediakan task opsional **Start Kripton with Windows** yang menulis nilai `Kripton` yang sama dengan toggle tray. Uninstall menghapus nilai Run tersebut, tetapi membiarkan `%APPDATA%\Kripton\state.json` agar data pengguna tidak hilang saat reinstall.

## Kontrol (Sprint 6)

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
| Focus: klik `Start`/`Pause` | Memulai atau menjeda countdown; chip waktu tetap tampil di pill compact |
| Focus: klik `Reset` | Mengembalikan fase aktif ke durasi penuh |
| Focus: klik preset `15/25/45/60` atau `±5` | Mengubah durasi sesi fokus dan menyimpannya sebagai default |
| Sesi fokus selesai | Chime berbunyi sekali, plasma pulse 3 detik, lalu pindah ke `Short Break` dalam keadaan pause |
| Water: klik `+150` / `+250` / `+500` | Menambah asupan dan mengaktifkan `Undo last glass` |
| Water: klik `Undo last glass` | Membatalkan gelas terakhir |
| Water: diam 90 menit di bawah target | Pesan `Time for a glass of water` menyala biru |
| To-do: ketik lalu `Enter` | Menambah task baru; input tetap fokus untuk task berikutnya |
| To-do: klik checkbox | Menandai selesai (strikethrough `#6B7280`) |
| To-do: drag baris | Memindahkan urutan task; urutan disimpan ke `state.json` |
| To-do: klik ikon hapus / `Clear done` | Menghapus satu task atau semua task selesai |
| Headphone dicolok/dilepas atau jadi endpoint default | HUD 320 × 64 px: nama perangkat + `Connected`/`Disconnected` hijau/amber, auto-hide 3 detik |
| Charger dicolok/dilepas, baterai ≤ 20%, atau penuh | HUD 300 × 58 px: status + persen, auto-hide 3 detik |
| Klik ikon tray (kiri, double-click) | Show/hide notch tanpa mematikan proses |
| Klik kanan ikon tray | Menu: Show/Hide, Start with Windows, Lyrics, Water goal, Quit |
| Tray → `Start with Windows` | Menulis/menghapus `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Kripton` |
| Tray → `Lyrics` | Menyalakan/mematikan lirik dan menyimpannya ke `state.json` |
| Tray → `Water goal` | Memilih target 1.500/2.000/2.500/3.000 ml dan menyimpannya |
| Tray → `Quit Kripton` | Menghapus ikon tray, menyimpan state, lalu keluar tanpa proses zombie |
| Jauhkan kursor (250 ms) | Notch mengecil kembali ke Idle/Closed |
| Klik notch | Toggle expand/collapse |
| Klik titik pagination | Pindah halaman (Music/Focus/Water/Shelf/To-do) |
| Klik kanan notch | Keluar dari aplikasi (shortcut cepat selain tray) |
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

State ditulis lewat file sementara lalu `rename`, dan state korup dikarantina menjadi `state.corrupt-YYYYMMDD-HHMMSS.json`. Tidak ada data yang dikirim ke mana pun saat ini. Metadata musik dibaca lokal lewat SMTC, audio dianalisis lokal lewat loopback, dan artwork tidak pernah diunggah. Timer fokus, asupan air, dan daftar to-do hanya tersimpan di `state.json` lokal. Lirik hanya memanggil LRCLIB secara langsung saat mode Lyrics aktif; tidak ada analytics.

Autostart bersifat opt-in dan hanya menulis satu nilai per-user:

```
HKCU\Software\Microsoft\Windows\CurrentVersion\Run
  Kripton = "<path>\kripton.exe"
```

Tidak ada scheduled task, service, atau perubahan machine-wide. Menghapus centang `Start with Windows` (atau uninstall) langsung menghapus nilai tersebut.

## Troubleshooting

| Gejala | Penyebab & solusi |
| --- | --- |
| Ikon tray tidak muncul | Explorer mungkin baru restart. Kripton mendengarkan pesan `TaskbarCreated` dan menambahkan ikon lagi otomatis; kalau tetap tidak ada, quit lewat klik kanan notch lalu jalankan ulang. |
| Tray → `Show Kripton` tidak memunculkan notch | Notch mungkin sudah terlihat tetapi berada di monitor utama. Kripton selalu menempel di tengah atas monitor primary; cek juga `kripton.log`. |
| HUD headphone tidak muncul | HUD hanya bereaksi pada endpoint yang terdeteksi headphone/headset (form factor 3/5 atau nama yang mengandung headphone/headset/earbud/AirPods/Buds). Speaker, HDMI, dan SPDIF sengaja diabaikan. Pastikan perangkat menjadi **default playback device**. |
| HUD charging tidak muncul | Transisi hanya dipicu saat status AC/charging berubah, baterai turun melewati 20%, atau mencapai penuh. Mesin tanpa baterai tidak menampilkan HUD charging. |
| Autostart tidak berjalan setelah login | Buka Task Manager → Startup dan pastikan `Kripton` tidak di-disable. Windows juga bisa menunda startup beberapa detik. |
| Aplikasi tidak mau start / langsung keluar | Hanya satu instance yang diizinkan. Cek Task Manager untuk `kripton.exe` lama, atau lihat `%APPDATA%\Kripton\kripton.log`. |
| Notch terlihat hitam di area transparan | Pastikan memakai build resmi (FemtoVG). Renderer lain tidak dipakai karena alpha Windows. |
| Build release gagal karena file terkunci | Hentikan `kripton.exe` yang sedang berjalan sebelum `cargo build --release`. |
| Installer tidak bisa dikompilasi | Script membutuhkan **Inno Setup 6** (`ISCC.exe`). Jalankan `cargo build --release` lebih dulu agar `target\release\kripton.exe` ada. |

Log dan state bisa dihapus kapan saja saat Kripton tidak berjalan; aplikasi akan membuatnya ulang dengan default.

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

Saat ini **60 unit test** mencakup state machine, parsing, geometri, HUD, autostart, dan tracker. CI Windows berjalan untuk fmt, clippy, dan test pada setiap push (`.github/workflows/ci.yml`).

## Lisensi

MIT © 2026 Kripton. Lihat [`LICENSE`](LICENSE).
