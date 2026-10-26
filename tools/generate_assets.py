#!/usr/bin/env python3
"""Generate Kripton's visual and audio assets.

The mascot is rendered procedurally from the PRD art direction:
a pitch-black squircle containing a single bioluminescent liquid plasma orb
with icy-cyan, emerald and deep-violet ionized gas layers.

Run from the repository root:
    python tools/generate_assets.py
"""

from __future__ import annotations

import math
import struct
import wave
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "assets"
ASSETS.mkdir(parents=True, exist_ok=True)

CYAN = np.array([0.0, 240.0, 255.0], dtype=np.float32)
EMERALD = np.array([16.0, 185.0, 129.0], dtype=np.float32)
VIOLET = np.array([124.0, 58.0, 237.0], dtype=np.float32)
INDIGO = np.array([79.0, 70.0, 229.0], dtype=np.float32)
WHITE = np.array([255.0, 255.0, 255.0], dtype=np.float32)


def _rgba(rgb: np.ndarray, alpha: np.ndarray) -> Image.Image:
    """Convert float RGB (0..255) and alpha (0..1) arrays into a PIL image."""
    a = np.clip(alpha * 255.0, 0, 255).astype(np.uint8)
    rgb = np.asarray(rgb, dtype=np.float32)
    if rgb.ndim == 1:
        rgb = np.broadcast_to(rgb, a.shape + (3,))
    rgb = np.clip(rgb, 0, 255).astype(np.uint8)
    return Image.fromarray(np.dstack([rgb, a]), mode="RGBA")


def _lerp_stops(t: np.ndarray, stops: list[tuple[float, np.ndarray]]) -> np.ndarray:
    positions = np.array([s[0] for s in stops], dtype=np.float32)
    out = np.empty(t.shape + (3,), dtype=np.float32)
    for channel in range(3):
        values = np.array([s[1][channel] for s in stops], dtype=np.float32)
        out[..., channel] = np.interp(t, positions, values)
    return out


def render_orb(size: int = 1024) -> Image.Image:
    """Render the Ionized Plasma Core with a soft outer aura."""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    center = (size - 1) / 2.0
    radius = size * 0.295

    x = (xx - center) / radius
    y = (yy - center) / radius
    r = np.sqrt(x * x + y * y)
    theta = np.arctan2(y, x)

    # Liquid ferrofluid turbulence: travelling waves deform the surface, while the
    # inner core stays round so it reads as a plasma orb rather than a snowflake.
    core_stability = np.clip((r - 0.08) / 0.55, 0.0, 1.0)
    turbulence = (
        0.034 * np.sin(5.0 * theta + 2.2 * r)
        + 0.021 * np.cos(9.0 * theta - 3.0 * r)
        + 0.012 * np.sin(17.0 * r + 1.4)
    ) * core_stability
    r_t = r + turbulence

    stops = [
        (0.00, WHITE),
        (0.16, np.array([215.0, 255.0, 255.0], dtype=np.float32)),
        (0.33, CYAN),
        (0.52, EMERALD),
        (0.72, INDIGO),
        (0.88, VIOLET),
        (1.00, np.array([24.0, 8.0, 52.0], dtype=np.float32)),
    ]
    orb_rgb = _lerp_stops(np.clip(r_t, 0.0, 1.0), stops)

    # Soft glass highlight and a thin ionized rim.
    specular = np.exp(-(((x + 0.28) / 0.27) ** 2 + ((y + 0.30) / 0.31) ** 2))
    orb_rgb += specular[..., None] * WHITE * 0.62
    rim = np.exp(-(((r_t - 0.945) / 0.028) ** 2))
    orb_rgb += rim[..., None] * CYAN * 0.55

    # Fine grain keeps the render from looking like a flat CSS gradient.
    rng = np.random.default_rng(7)
    orb_rgb += rng.normal(0.0, 3.2, (size, size, 1))
    orb_alpha = np.clip((1.0 - r_t) / 0.055, 0.0, 1.0) ** 0.72

    # Outer aura: cyan close to the orb, violet further out.
    aura_alpha = np.clip(np.exp(-(((r - 0.92) / 0.40) ** 2)) * 0.62, 0.0, 1.0)
    aura_alpha *= np.clip((2.35 - r) / 0.55, 0.0, 1.0)
    cyan_weight = np.exp(-(((r - 0.80) / 0.23) ** 2))
    aura_rgb = VIOLET * (1.0 - cyan_weight[..., None]) + CYAN * cyan_weight[..., None]

    aura = _rgba(aura_rgb, aura_alpha).filter(ImageFilter.GaussianBlur(size * 0.035))
    orb = _rgba(orb_rgb, orb_alpha).filter(ImageFilter.GaussianBlur(size * 0.0035))
    highlight = _rgba(WHITE, specular * 0.75).filter(ImageFilter.GaussianBlur(size * 0.012))

    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.alpha_composite(aura)
    out.alpha_composite(orb)
    out.alpha_composite(highlight)
    return out


def render_corona(size: int = 1024) -> Image.Image:
    """Render the rotating emerald corona that orbits the plasma core.

    Kept on its own transparent canvas so Slint can rotate it continuously
    without re-rendering the core or the outer aura.
    """
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    center = (size - 1) / 2.0
    x = (xx - center) / (size * 0.5)
    y = (yy - center) / (size * 0.5)
    r = np.sqrt(x * x + y * y)
    theta = np.arctan2(y, x)

    ring_radius = 0.36
    ring = np.exp(-(((r - ring_radius) / 0.075) ** 2))

    # Three slow lobes plus fine filaments give the ring a liquid, ionized edge.
    lobes = 0.5 + 0.5 * np.sin(3.0 * theta + 0.7)
    filaments = 0.5 + 0.5 * np.sin(11.0 * theta - 2.1)
    wisp = 0.35 + 0.65 * (0.65 * lobes + 0.35 * filaments)

    hotspots = np.zeros_like(r)
    for angle, width, strength in ((2.1, 0.20, 0.85), (4.4, 0.16, 0.55), (5.6, 0.24, 0.65)):
        delta = np.angle(np.exp(1j * (theta - angle)))
        hotspots += strength * np.exp(-((delta / width) ** 2)) * np.exp(
            -(((r - ring_radius) / 0.11) ** 2)
        )

    bloom = np.exp(-(((r - 0.43) / 0.14) ** 2)) * 0.20
    alpha = np.clip(ring * (0.30 + 0.70 * wisp) + hotspots * 0.70 + bloom, 0.0, 1.0) * 0.88

    hot = np.clip(hotspots, 0.0, 1.0)
    rgb = EMERALD * (1.0 - hot[..., None]) + CYAN * hot[..., None]
    return _rgba(rgb, alpha).filter(ImageFilter.GaussianBlur(size * 0.006))


def render_aura(size: int = 1024) -> Image.Image:
    """Render the soft violet halo that sits behind the core."""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    center = (size - 1) / 2.0
    r = np.sqrt(((xx - center) / (size * 0.5)) ** 2 + ((yy - center) / (size * 0.5)) ** 2)

    alpha = np.exp(-((r / 0.48) ** 2)) * 0.58
    alpha *= np.clip((1.16 - r) / 0.34, 0.0, 1.0)
    cyan_weight = np.exp(-(((r - 0.30) / 0.22) ** 2))
    rgb = VIOLET * (1.0 - cyan_weight[..., None]) + CYAN * cyan_weight[..., None]
    return _rgba(rgb, alpha).filter(ImageFilter.GaussianBlur(size * 0.030))


def render_icon(orb_master: Image.Image) -> Image.Image:
    size = 1024
    icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))

    # Pitch-black squircle with a barely visible ionized rim.
    squircle = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(squircle)
    draw.rounded_rectangle((20, 20, size - 21, size - 21), radius=224, fill=(0, 0, 0, 255))
    draw.rounded_rectangle(
        (21, 21, size - 22, size - 22), radius=223, outline=(31, 32, 38, 255), width=3
    )
    icon.alpha_composite(squircle)

    # Ambient bottom glow, clipped to the squircle.
    glow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gd.ellipse((130, 720, 894, 1140), fill=(88, 42, 210, 120))
    gd.ellipse((300, 810, 724, 1080), fill=(0, 190, 220, 95))
    glow = glow.filter(ImageFilter.GaussianBlur(95))
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((20, 20, size - 21, size - 21), radius=224, fill=255)
    icon.alpha_composite(Image.composite(glow, Image.new("RGBA", (size, size), (0, 0, 0, 0)), mask))

    # Mascot orb, slightly above the optical center.
    orb = orb_master.resize((720, 720), Image.Resampling.LANCZOS)
    icon.alpha_composite(orb, (152, 132))

    return icon


def render_concept(orb_master: Image.Image) -> Image.Image:
    width, height = 1920, 1080
    canvas = Image.new("RGBA", (width, height), (5, 5, 7, 255))

    # Soft violet/cyan atmosphere behind the notch.
    yy, xx = np.mgrid[0:height, 0:width].astype(np.float32)
    cx, cy = width / 2.0, 100.0
    dist = np.sqrt(((xx - cx) / 760.0) ** 2 + ((yy - cy) / 420.0) ** 2)
    haze_alpha = np.clip(np.exp(-(dist**2) * 2.2) * 0.30, 0.0, 1.0)
    haze_rgb = np.zeros((height, width, 3), dtype=np.float32)
    haze_rgb[:] = VIOLET * 0.65 + CYAN * 0.35
    canvas.alpha_composite(_rgba(haze_rgb, haze_alpha))

    # Floating top-attached notch.
    notch_w, notch_h, notch_radius = 900, 214, 72
    left = (width - notch_w) // 2
    top = -notch_radius
    shadow = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    sd.rounded_rectangle(
        (left + 10, top + 26, left + notch_w - 10, top + notch_h + 26),
        radius=notch_radius,
        fill=(0, 0, 0, 190),
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(42))
    canvas.alpha_composite(shadow)

    notch = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    nd = ImageDraw.Draw(notch)
    nd.rounded_rectangle(
        (left, top, left + notch_w, top + notch_h), radius=notch_radius, fill=(0, 0, 0, 255)
    )
    canvas.alpha_composite(notch)

    # Inner bottom ambient glow.
    glow = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    gd = ImageDraw.Draw(glow)
    gd.rounded_rectangle(
        (left + 96, top + notch_h - 46, left + notch_w - 96, top + notch_h - 18),
        radius=14,
        fill=(0, 225, 255, 90),
    )
    gd.rounded_rectangle(
        (left + 220, top + notch_h - 46, left + notch_w - 220, top + notch_h - 18),
        radius=14,
        fill=(124, 58, 237, 140),
    )
    glow = glow.filter(ImageFilter.GaussianBlur(34))
    mask = Image.new("L", (width, height), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (left, top, left + notch_w, top + notch_h), radius=notch_radius, fill=255
    )
    canvas.alpha_composite(Image.composite(glow, Image.new("RGBA", (width, height), (0, 0, 0, 0)), mask))

    # Mascot + restrained wordmark.
    orb = orb_master.resize((196, 196), Image.Resampling.LANCZOS)
    canvas.alpha_composite(orb, ((width - 196) // 2, 58))

    draw = ImageDraw.Draw(canvas)
    try:
        font = ImageFont.truetype("C:/Windows/Fonts/segoeuib.ttf", 22)
    except OSError:
        font = ImageFont.load_default()
    word = "KRIPTON"
    char_w = 30
    start = (width - (len(word) - 1) * char_w) // 2
    for index, char in enumerate(word):
        draw.text(
            (start + index * char_w, 268),
            char,
            fill=(148, 163, 184, 230),
            font=font,
            anchor="mm",
        )

    # A faint horizontal light line under the notch to suggest a screen edge.
    edge = Image.new("RGBA", (width, height), (0, 0, 0, 0))
    ed = ImageDraw.Draw(edge)
    ed.line((0, 214, left - 24, 214), fill=(80, 80, 92, 70), width=1)
    ed.line((left + notch_w + 24, 214, width, 214), fill=(80, 80, 92, 70), width=1)
    canvas.alpha_composite(edge)

    return canvas.convert("RGB")


def render_chime() -> None:
    sample_rate = 44_100
    duration = 1.8
    samples = int(sample_rate * duration)
    t = np.arange(samples, dtype=np.float64) / sample_rate
    left = np.zeros(samples, dtype=np.float64)
    right = np.zeros(samples, dtype=np.float64)

    notes = [
        (0.00, 880.00, 0.22, -0.15),
        (0.13, 1174.66, 0.20, 0.10),
        (0.27, 1567.98, 0.17, 0.22),
        (0.42, 1760.00, 0.10, -0.05),
    ]

    for offset, frequency, amplitude, pan in notes:
        local_t = t - offset
        active = local_t >= 0.0
        attack = np.clip(local_t / 0.012, 0.0, 1.0)
        envelope = np.zeros_like(local_t)
        envelope[active] = (
            attack[active]
            * np.exp(-3.7 * local_t[active])
            * (1.0 - np.clip((local_t[active] - 1.35) / 0.45, 0.0, 1.0))
        )
        tone = np.sin(2.0 * np.pi * frequency * local_t)
        # A quiet octave partial makes the chime feel glassy rather than beepy.
        tone += 0.16 * np.sin(2.0 * np.pi * frequency * 2.0 * local_t)
        signal = tone * envelope * amplitude
        left += signal * (1.0 - max(pan, 0.0) * 0.8)
        right += signal * (1.0 + min(pan, 0.0) * 0.8)

    # Warm sub tone.
    sub_env = np.exp(-2.1 * t) * (1.0 - np.clip((t - 1.35) / 0.45, 0.0, 1.0))
    sub = 0.055 * np.sin(2.0 * np.pi * 440.0 * t) * sub_env
    left += sub
    right += sub

    # Gentle stereo width.
    delay = int(0.006 * sample_rate)
    right[delay:] += left[:-delay] * 0.10

    peak = max(np.max(np.abs(left)), np.max(np.abs(right)), 1e-9)
    scale = 0.72 / peak
    left = np.clip(left * scale, -1.0, 1.0)
    right = np.clip(right * scale, -1.0, 1.0)

    frames = bytearray()
    for l_sample, r_sample in zip(left, right):
        frames += struct.pack("<hh", int(l_sample * 32767), int(r_sample * 32767))

    with wave.open(str(ASSETS / "chime.wav"), "wb") as wav:
        wav.setnchannels(2)
        wav.setsampwidth(2)
        wav.setframerate(sample_rate)
        wav.writeframes(bytes(frames))


def main() -> None:
    print("Rendering Ionized Plasma Core…")
    orb = render_orb(1024)
    orb.resize((512, 512), Image.Resampling.LANCZOS).save(ASSETS / "plasma_core.png")

    print("Rendering rotating corona…")
    render_corona(1024).resize((512, 512), Image.Resampling.LANCZOS).save(
        ASSETS / "plasma_corona.png"
    )

    print("Rendering outer aura…")
    render_aura(1024).resize((512, 512), Image.Resampling.LANCZOS).save(
        ASSETS / "plasma_aura.png"
    )

    print("Rendering app icon…")
    icon = render_icon(orb)
    icon.save(ASSETS / "icon.png")
    icon.save(
        ASSETS / "icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    print("Rendering 16:9 concept art…")
    render_concept(orb).save(ASSETS / "plasma_core_concept.png")

    print("Rendering notification chime…")
    render_chime()

    print("Assets written to", ASSETS)


if __name__ == "__main__":
    main()
