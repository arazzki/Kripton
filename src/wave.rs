//! WASAPI loopback capture and the live five-band spectrum.
//!
//! A dedicated MTA thread owns the capture client, keeps a 512-sample ring of
//! mono audio, and every ~33 ms runs a Hann-windowed radix-2 FFT. The five
//! log-spaced bands are smoothed with a fast attack / slow release and
//! published through atomics, so the UI never blocks on audio.

use std::{
    f32::consts::TAU,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use windows::{
    core::PWSTR,
    Win32::{
        Media::Audio::{
            eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
            MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
        },
    },
};

pub const BAND_COUNT: usize = 5;
pub const FFT_SIZE: usize = 512;

const READ_INTERVAL: Duration = Duration::from_millis(10);
const ANALYSIS_INTERVAL: Duration = Duration::from_millis(33);
const DEVICE_CHECK_INTERVAL: Duration = Duration::from_secs(2);
const RECONNECT_DELAY: Duration = Duration::from_millis(700);
const MIN_FREQ: f32 = 40.0;
const MAX_FREQ: f32 = 16_000.0;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// Per-band makeup gain: music rolls off toward the top, so the upper bars
/// would otherwise barely move on an unweighted average.
const BAND_GAIN: [f32; BAND_COUNT] = [1.35, 1.25, 1.18, 1.25, 1.45];

#[derive(Default)]
struct Shared {
    bands: [AtomicU32; BAND_COUNT],
    peak: AtomicU32,
}

pub struct AudioWave {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
}

impl AudioWave {
    pub fn start() -> Self {
        let shared = Arc::new(Shared::default());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_shared = shared.clone();
        let worker_stop = stop.clone();

        thread::Builder::new()
            .name("kripton-audio".into())
            .spawn(move || capture_loop(worker_shared, worker_stop))
            .expect("failed to spawn audio worker");

        Self { shared, stop }
    }

    /// Latest smoothed band levels, each 0.0 - 1.0.
    pub fn bands(&self) -> [f32; BAND_COUNT] {
        std::array::from_fn(|index| {
            f32::from_bits(self.shared.bands[index].load(Ordering::Relaxed)).clamp(0.0, 1.0)
        })
    }

    /// Latest peak amplitude (0.0 - 1.0) with a short decay.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.shared.peak.load(Ordering::Relaxed)).clamp(0.0, 1.0)
    }
}

impl Drop for AudioWave {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn capture_loop(shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    let mut last_log = Instant::now() - Duration::from_secs(30);
    while !stop.load(Ordering::Relaxed) {
        match unsafe { Capture::open() } {
            Ok(mut capture) => {
                run_capture(&mut capture, &shared, &stop);
                publish(&shared, [0.0; BAND_COUNT], 0.0);
            }
            Err(error) => {
                if last_log.elapsed() >= Duration::from_secs(10) {
                    crate::log_line(&format!("audio loopback unavailable: {error}"));
                    last_log = Instant::now();
                }
                publish(&shared, [0.0; BAND_COUNT], 0.0);
            }
        }

        let wake_at = Instant::now() + RECONNECT_DELAY;
        while Instant::now() < wake_at && !stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn run_capture(capture: &mut Capture, shared: &Shared, stop: &AtomicBool) {
    if let Err(error) = unsafe { capture.client.Start() } {
        crate::log_line(&format!("audio loopback start failed: {error}"));
        return;
    }

    let debug = std::env::var_os("KRIPTON_AUDIO_DEBUG").is_some();
    let mut debug_at = Instant::now();
    let mut frames_seen = 0_u64;

    let mut analyzer = Analyzer::new(capture.sample_rate);
    let mut last_analysis = Instant::now();
    let mut last_device_check = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        match unsafe { capture.read_packet() } {
            Ok(Some(samples)) => {
                frames_seen += samples.len() as u64;
                analyzer.push(&samples);
            }
            Ok(None) => {}
            Err(error) => {
                crate::log_line(&format!("audio loopback read failed: {error}"));
                break;
            }
        }

        if last_analysis.elapsed() >= ANALYSIS_INTERVAL {
            let dt = last_analysis.elapsed().as_secs_f32().clamp(0.001, 0.2);
            last_analysis = Instant::now();
            let bands = analyzer.spectrum(dt);
            let peak = analyzer.take_peak();
            publish(shared, bands, peak);

            if debug && debug_at.elapsed() >= Duration::from_secs(2) {
                debug_at = Instant::now();
                crate::log_line(&format!(
                    "audio frames={frames_seen} rate={} peak={peak:.3} bands={bands:?}",
                    capture.sample_rate as u32
                ));
            }
        }

        if last_device_check.elapsed() >= DEVICE_CHECK_INTERVAL {
            last_device_check = Instant::now();
            if !capture.is_default_device() {
                break;
            }
        }

        thread::sleep(READ_INTERVAL);
    }

    unsafe {
        let _ = capture.client.Stop();
    }
}

fn publish(shared: &Shared, bands: [f32; BAND_COUNT], peak: f32) {
    for (slot, value) in shared.bands.iter().zip(bands) {
        slot.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    let previous = f32::from_bits(shared.peak.load(Ordering::Relaxed));
    let decayed = (previous * 0.8).max(peak.clamp(0.0, 1.0));
    shared.peak.store(decayed.to_bits(), Ordering::Relaxed);
}

struct Capture {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    format: SampleFormat,
    sample_rate: f32,
    device_id: String,
}

impl Capture {
    unsafe fn open() -> windows::core::Result<Self> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let device_id = device.GetId().map(|id| take_pwstr(id)).unwrap_or_default();
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let mix = client.GetMixFormat()?;
        // Read through the raw CoTaskMem buffer: WAVEFORMATEXTENSIBLE is
        // larger than WAVEFORMATEX, so copying the base struct would cut off
        // the SubFormat GUID we need to tell float from PCM.
        let tag = (*mix).wFormatTag;
        let bits = (*mix).wBitsPerSample;
        let sample_rate = (*mix).nSamplesPerSec as f32;
        let sample_format = SampleFormat::from_wave_format(mix);
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            mix,
            None,
        );
        CoTaskMemFree(Some(mix as *const std::ffi::c_void));
        init?;

        let sample_format = sample_format.ok_or_else(|| {
            windows::core::Error::new(
                windows::Win32::Foundation::E_NOTIMPL,
                format!("unsupported audio format: tag={tag} bits={bits}"),
            )
        })?;
        let capture = client.GetService::<IAudioCaptureClient>()?;

        Ok(Self {
            client,
            capture,
            format: sample_format,
            sample_rate,
            device_id,
        })
    }

    /// Reads one WASAPI packet and mixes it down to mono.
    unsafe fn read_packet(&mut self) -> windows::core::Result<Option<Vec<f32>>> {
        let mut frames = self.capture.GetNextPacketSize()?;
        if frames == 0 {
            return Ok(None);
        }

        let mut mono = Vec::with_capacity(frames as usize);
        while frames > 0 {
            let mut data: *mut u8 = ptr::null_mut();
            let mut count = 0_u32;
            let mut flags = 0_u32;
            self.capture
                .GetBuffer(&mut data, &mut count, &mut flags, None, None)?;

            if count > 0 {
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    mono.resize(mono.len() + count as usize, 0.0);
                } else if !data.is_null() {
                    self.format.push_mono(data, count, &mut mono);
                }
            }

            self.capture.ReleaseBuffer(count)?;
            frames = self.capture.GetNextPacketSize()?;
        }

        Ok(Some(mono))
    }

    fn is_default_device(&self) -> bool {
        default_device_id()
            .map(|id| id == self.device_id)
            .unwrap_or(false)
    }
}

fn default_device_id() -> Option<String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        device.GetId().ok().map(|id| take_pwstr(id))
    }
}

/// Converts a `CoTaskMemAlloc` string (as returned by `IMMDevice::GetId`)
/// into a `String` and releases the buffer.
unsafe fn take_pwstr(value: PWSTR) -> String {
    let text = value.to_string().unwrap_or_default();
    CoTaskMemFree(Some(value.0 as *const std::ffi::c_void));
    text
}

#[derive(Clone, Copy)]
enum SampleKind {
    F32,
    I16,
    I32,
    U8,
}

struct SampleFormat {
    channels: usize,
    bytes_per_sample: usize,
    kind: SampleKind,
}

impl SampleFormat {
    /// Reads the mix format straight from the CoTaskMem buffer. The pointer
    /// must still be valid (see `Capture::open`).
    unsafe fn from_wave_format(format: *const WAVEFORMATEX) -> Option<Self> {
        let tag = (*format).wFormatTag;
        let bits = (*format).wBitsPerSample;
        let kind = match tag {
            WAVE_FORMAT_IEEE_FLOAT => SampleKind::F32,
            WAVE_FORMAT_PCM => match bits {
                16 => SampleKind::I16,
                32 => SampleKind::I32,
                8 => SampleKind::U8,
                _ => return None,
            },
            WAVE_FORMAT_EXTENSIBLE => {
                let extended = format as *const WAVEFORMATEXTENSIBLE;
                let subformat = ptr::addr_of!((*extended).SubFormat).read_unaligned();
                match subformat.data1 {
                    3 => SampleKind::F32,
                    1 => match bits {
                        16 => SampleKind::I16,
                        32 => SampleKind::I32,
                        8 => SampleKind::U8,
                        _ => return None,
                    },
                    _ => return None,
                }
            }
            _ => return None,
        };

        Some(Self {
            channels: ((*format).nChannels as usize).max(1),
            bytes_per_sample: (bits as usize / 8).max(1),
            kind,
        })
    }

    fn push_mono(&self, data: *const u8, frames: u32, out: &mut Vec<f32>) {
        let frame_bytes = self.channels * self.bytes_per_sample;
        let length = frames as usize * frame_bytes;
        let bytes = unsafe { std::slice::from_raw_parts(data, length) };

        for frame in bytes.chunks_exact(frame_bytes) {
            let mut sum = 0.0;
            for channel in 0..self.channels {
                let offset = channel * self.bytes_per_sample;
                let sample = &frame[offset..offset + self.bytes_per_sample];
                sum += match self.kind {
                    SampleKind::F32 => {
                        f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]])
                    }
                    SampleKind::I16 => i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0,
                    SampleKind::I32 => {
                        i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                            / 2_147_483_648.0
                    }
                    SampleKind::U8 => (sample[0] as f32 - 128.0) / 128.0,
                };
            }
            out.push(sum / self.channels as f32);
        }
    }
}

struct Analyzer {
    samples: [f32; FFT_SIZE],
    write: usize,
    peak: f32,
    sample_rate: f32,
    bands: [f32; BAND_COUNT],
    smoothed: [f32; BAND_COUNT],
}

impl Analyzer {
    fn new(sample_rate: f32) -> Self {
        Self {
            samples: [0.0; FFT_SIZE],
            write: 0,
            peak: 0.0,
            sample_rate,
            bands: [0.0; BAND_COUNT],
            smoothed: [0.0; BAND_COUNT],
        }
    }

    fn push(&mut self, mono: &[f32]) {
        for &sample in mono {
            self.samples[self.write] = sample;
            self.write = (self.write + 1) % FFT_SIZE;
            self.peak = self.peak.max(sample.abs());
        }
    }

    fn take_peak(&mut self) -> f32 {
        std::mem::take(&mut self.peak)
    }

    /// Runs one Hann-windowed FFT and returns the smoothed band levels.
    fn spectrum(&mut self, dt: f32) -> [f32; BAND_COUNT] {
        let mut real = [0.0_f32; FFT_SIZE];
        let mut imag = [0.0_f32; FFT_SIZE];

        for (index, slot) in real.iter_mut().enumerate() {
            let window = 0.5 - 0.5 * (TAU * index as f32 / (FFT_SIZE - 1) as f32).cos();
            *slot = self.samples[(self.write + index) % FFT_SIZE] * window;
        }

        fft(&mut real, &mut imag);
        self.bands = band_levels(&real, &imag, self.sample_rate);

        // Fast attack, slow release: the bars jump on transients but settle
        // gently instead of flickering on every frame.
        let attack = 1.0 - (-dt * 30.0).exp();
        let release = 1.0 - (-dt * 8.0).exp();
        for index in 0..BAND_COUNT {
            let target = self.bands[index];
            let rate = if target > self.smoothed[index] {
                attack
            } else {
                release
            };
            self.smoothed[index] += (target - self.smoothed[index]) * rate;
        }
        self.smoothed
    }
}

fn band_levels(real: &[f32], imag: &[f32], sample_rate: f32) -> [f32; BAND_COUNT] {
    let nyquist = FFT_SIZE / 2;
    let max_freq = MAX_FREQ.min(sample_rate * 0.5);
    let ratio = (max_freq / MIN_FREQ).powf(1.0 / BAND_COUNT as f32);
    let mut levels = [0.0; BAND_COUNT];

    for (band, level) in levels.iter_mut().enumerate() {
        let low = MIN_FREQ * ratio.powi(band as i32);
        let high = MIN_FREQ * ratio.powi(band as i32 + 1);
        let low_bin = ((low / sample_rate * FFT_SIZE as f32).floor() as usize).max(1);
        let high_bin = ((high / sample_rate * FFT_SIZE as f32).ceil() as usize)
            .min(nyquist)
            .max(low_bin + 1);

        // Peak bin, not the average: the upper bands cover many more bins, so
        // averaging would drown their few loud bins in silence and leave the
        // right-hand bars almost still.
        let mut peak = 0.0_f32;
        for bin in low_bin..high_bin {
            peak = peak.max((real[bin] * real[bin] + imag[bin] * imag[bin]).sqrt());
        }

        // A full-scale sine peaks at N/4 after the Hann window (-12 dBFS).
        let magnitude = peak / FFT_SIZE as f32;
        let decibels = 20.0 * (magnitude + 1e-9).log10();
        let normalized = ((decibels + 66.0) / 54.0).clamp(0.0, 1.0).powf(1.25);
        *level = (normalized * BAND_GAIN[band]).clamp(0.0, 1.0);
    }

    levels
}

/// In-place iterative radix-2 FFT. `real` and `imag` must have the same
/// power-of-two length.
fn fft(real: &mut [f32], imag: &mut [f32]) {
    let length = real.len();
    debug_assert_eq!(length, imag.len());
    debug_assert!(length.is_power_of_two());

    let mut reversed = 0;
    for index in 1..length {
        let mut bit = length >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed |= bit;
        if index < reversed {
            real.swap(index, reversed);
            imag.swap(index, reversed);
        }
    }

    let mut size = 2;
    while size <= length {
        let angle = -TAU / size as f32;
        let half = size / 2;

        for start in (0..length).step_by(size) {
            for offset in 0..half {
                let even = start + offset;
                let odd = even + half;

                // Recomputing the twiddle from the angle keeps f32 phase error
                // from accumulating across the 512-point transform.
                let phase = angle * offset as f32;
                let (twiddle_real, twiddle_imag) = (phase.cos(), phase.sin());

                let odd_real = real[odd] * twiddle_real - imag[odd] * twiddle_imag;
                let odd_imag = real[odd] * twiddle_imag + imag[odd] * twiddle_real;

                real[odd] = real[even] - odd_real;
                imag[odd] = imag[even] - odd_imag;
                real[even] += odd_real;
                imag[even] += odd_imag;
            }
        }

        size <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, sample_rate: f32) -> [f32; FFT_SIZE] {
        let mut samples = [0.0; FFT_SIZE];
        for (index, sample) in samples.iter_mut().enumerate() {
            *sample = (TAU * frequency * index as f32 / sample_rate).sin();
        }
        samples
    }

    fn spectrum_of(samples: &[f32; FFT_SIZE], sample_rate: f32) -> [f32; BAND_COUNT] {
        let mut analyzer = Analyzer::new(sample_rate);
        analyzer.push(samples);
        // Two passes so the attack smoothing settles close to the target.
        analyzer.spectrum(0.033);
        analyzer.spectrum(0.033)
    }

    #[test]
    fn fft_peaks_on_the_sine_bin() {
        let mut real = [0.0; FFT_SIZE];
        let mut imag = [0.0; FFT_SIZE];
        for (index, value) in real.iter_mut().enumerate() {
            *value = (TAU * 32.0 * index as f32 / FFT_SIZE as f32).sin();
        }
        fft(&mut real, &mut imag);
        let magnitudes: Vec<f32> = (0..FFT_SIZE / 2)
            .map(|bin| (real[bin] * real[bin] + imag[bin] * imag[bin]).sqrt())
            .collect();
        let peak = magnitudes
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, 32);
    }

    #[test]
    fn low_sine_lights_the_low_band() {
        let levels = spectrum_of(&sine(250.0, 48_000.0), 48_000.0);
        let peak = levels
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, 1, "250 Hz should sit in the second band: {levels:?}");
    }

    #[test]
    fn high_sine_lights_the_high_band() {
        let levels = spectrum_of(&sine(8_000.0, 48_000.0), 48_000.0);
        let peak = levels
            .iter()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(right.1))
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(peak, 4, "8 kHz should sit in the top band: {levels:?}");
    }

    #[test]
    fn louder_signals_raise_the_bars() {
        let quiet = spectrum_of(&sine(250.0, 48_000.0).map(|sample| sample * 0.08), 48_000.0);
        let loud = spectrum_of(&sine(250.0, 48_000.0), 48_000.0);
        assert!(loud[1] > quiet[1] + 0.2, "loud={loud:?} quiet={quiet:?}");
    }

    #[test]
    fn silence_keeps_the_bars_dark() {
        let levels = spectrum_of(&[0.0; FFT_SIZE], 48_000.0);
        assert!(levels.iter().all(|level| *level < 0.01));
    }

    #[test]
    fn peak_tracks_the_loudest_sample() {
        let mut analyzer = Analyzer::new(48_000.0);
        analyzer.push(&[0.1, -0.6, 0.3]);
        assert!((analyzer.take_peak() - 0.6).abs() < 1e-6);
        assert_eq!(analyzer.take_peak(), 0.0);
    }

    #[test]
    fn band_edges_stay_inside_the_spectrum() {
        let levels = band_levels(&[0.0; FFT_SIZE], &[0.0; FFT_SIZE], 44_100.0);
        assert_eq!(levels, [0.0; BAND_COUNT]);
    }
}
