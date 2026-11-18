# 📐 Rangkuman Dimensi & Sensor Dynamic Island

Dokumen acuan ukuran visual (*UI layout*) dan area sensor (*hit-test buffer*) untuk implementasi Dynamic Island / Living Notch.

---

## 📊 Tabel Spesifikasi Dimensi

| Mode / Status | Ukuran Visual (W × H) | Posisi Y | Area Sensor (*Hit-Test*) | Catatan & Perilaku |
| :--- | :---: | :---: | :---: | :--- |
| **1. Stealth Mode** *(Hidden)* | **`128 × 4 px`** | `y = 0` | **`148 × 6 px`** *(Strict)* | Garis mikro di tepi paling atas saat idle tanpa media maupun timer fokus aktif (*Smart Adaptive*). Sensor dibuat +20px lebih lebar (`WAKE_STRIP_W = 148 px`, `WAKE_STRIP_H = 6 px`) agar mudah disentuh mouse dari tepi layar tanpa mengganggu tab browser. |
| **2. Closed Compact Pill** *(Smart Adaptive Active)* | **`260 × 28 px`** | `y = 0` | **`284 × 40 px`** | Aktif otomatis saat media sedang diputar atau timer fokus sedang aktif (`media_active \|\| focus_active`), dan kembali ke Stealth ketika keduanya berhenti. Menggunakan buffer sensor `HIT_MARGIN = 12 px`. |
| **— Compact** *(Hovered)* | **`276 × 30 px`** | `y = 0` | **`300 × 42 px`** | Melebar halus saat kursor mouse menyentuh kapsul atau *wake strip* (atau saat animasi selesai fokus berlangsung). |
| **3. Expanded HUD** *(Expanded & Lyrics)* | **`476 × 172 px`** | `y = 0` | **`500 × 184 px`** | Kartu terbuka penuh (Music/Lyrics, Focus, Water, Shelf, To-do) dengan navigasi dock. |
| **4. Device HUD** *(Headphone)* | **`268 × 54 px`** | `y = 0` | **`292 × 66 px`** | Notifikasi transien perangkat audio selama 3 detik sebelum kembali ke mode sebelumnya. |
| **5. Charging HUD** *(Power)* | **`256 × 50 px`** | `y = 0` | **`280 × 62 px`** | Notifikasi transien pengisian daya / baterai rendah selama 3 detik sebelum kembali ke mode sebelumnya. |

---

## 🔍 Detail Perhitungan Sensor (*Hit-Test Buffer*) & Smart Adaptive

### 1. Stealth Mode (Sensor Pasif / Wake Strip)
- **Dimensi Sensor:** `148 × 6 px` (`WAKE_STRIP_W = 148 px`, `WAKE_STRIP_H = 6 px`, tepat di tengah atas layar; aktif ketika `notch_height <= 10 px`).
- **Logika:** Di luar koordinat `148 × 6 px` ini, seluruh jendela bersifat tembus pandang (*click-through 100%* via `WS_EX_TRANSPARENT`), sehingga tab browser di kiri dan kanan tetap dapat diklik secara normal.

### 2. Perilaku *Smart Adaptive* (Mode Tertutup Otomatis)
Saat tidak diperluas (*collapsed*) dan kursor berada di luar area sensor:
- Jika **`media_active || focus_active`** bernilai `true`, notch bertahan di **Closed Compact Pill (`260 × 28 px`)** untuk menampilkan indikator media/visualizer dan sisa waktu fokus.
- Jika keduanya `false`, notch otomatis menyusut kembali ke **Stealth Mode (`128 × 4 px`)** agar tetap minimalis di tepi layar.

### 3. Mode Terbuka (Compact, Expanded, & Hardware HUD)
Ketika pulau dalam kondisi aktif (*awake*, yaitu `notch_height > 10 px`), sistem menambahkan toleransi bantalan (**`HIT_MARGIN = 12 px`**) di sekeliling kapsul:
- **Sisi Kiri:** `X - 12 px`
- **Sisi Kanan:** `X + Width + 12 px`
- **Sisi Bawah:** `Y + Height + 12 px`

> **Fungsi Buffer 12px:** Memberikan ruang toleransi agar pulau tidak langsung menutup mendadak jika kursor mouse pengguna bergeser keluar sedikit secara tidak sengaja saat ingin mengklik tombol.
