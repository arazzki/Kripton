# 📐 Rangkuman Dimensi & Sensor Dynamic Island

Dokumen acuan ukuran visual (*UI layout*) dan area sensor (*hit-test buffer*) untuk implementasi Dynamic Island / Living Notch.

---

## 📊 Tabel Spesifikasi Dimensi

| Mode / Status | Ukuran Visual (W × H) | Posisi Y | Area Sensor (*Hit-Test*) | Catatan & Perilaku |
| :--- | :---: | :---: | :---: | :--- |
| **1. Stealth Mode** *(Hidden)* | **`184 × 5 px`** | `y = 0` | **`200 × 6 px`** *(Strict)* | Garis mikro di tepi paling atas. Sensor dibuat +16px lebih lebar agar mudah disentuh mouse dari tepi layar tanpa mengganggu tab browser. |
| **2. Compact Pill** *(Default)* | **`336 × 34 px`** | `y = 0` / `6` | **`360 × 46 px`** | Ukuran standar saat idle atau memutar media dan jam. Menggunakan buffer sensor `HIT_MARGIN = 12 px`. |
| **— Compact** *(Hovered)* | **`360 × 36 px`** | `y = 0` / `6` | **`384 × 48 px`** | Melebar halus saat kursor mouse menyentuh area kapsul. |
| **3. Expanded HUD** | **`580 × 192 px`** | `y = 0` / `6` | **`604 × 204 px`** | Kartu terbuka penuh (kontrol media, cover art, seekbar, file shelf). |

---

## 🔍 Detail Perhitungan Sensor (*Hit-Test Buffer*)

### 1. Stealth Mode (Sensor Pasif / Wake Strip)
- **Dimensi Sensor:** `200 × 6 px` (tepat di tengah atas layar).
- **Logika:** Di luar koordinat `200 × 6 px` ini, seluruh jendela bersifat tembus pandang (*click-through 100%* via `WS_EX_TRANSPARENT`), sehingga tab browser di kiri dan kanan tetap dapat diklik secara normal.

### 2. Mode Terbuka (Compact & Expanded)
Ketika pulau dalam kondisi aktif (*awake*), sistem menambahkan toleransi bantalan (**`HIT_MARGIN = 12 px`**) di sekeliling kapsul:
- **Sisi Kiri:** `X - 12 px`
- **Sisi Kanan:** `X + Width + 12 px`
- **Sisi Bawah:** `Y + Height + 12 px`

> **Fungsi Buffer 12px:** Memberikan ruang toleransi agar pulau tidak langsung menutup mendadak jika kursor mouse pengguna bergeser keluar sedikit secara tidak sengaja saat ingin mengklik tombol.
