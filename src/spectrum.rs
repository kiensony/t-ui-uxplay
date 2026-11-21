use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{
    array,
    f32::consts::TAU,
    io::{self, Read},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const SAMPLE_RATE: usize = 24_000;
pub const BAND_COUNT: usize = 24;
const FFT_SIZE: usize = 1024;
const SILENCE_TIMEOUT: Duration = Duration::from_millis(250);
const PEAK_HOLD_FRAMES: u8 = 5;

#[derive(Clone, Copy, Default)]
pub struct Spectrum {
    pub levels: [f32; BAND_COUNT],
    pub peaks: [f32; BAND_COUNT],
    pub phase: f32,
}

pub struct Monitor {
    current: Arc<Mutex<Option<(Instant, Spectrum)>>>,
}

impl Monitor {
    pub fn spawn(mut pcm: io::PipeReader) -> io::Result<Self> {
        let current = Arc::new(Mutex::new(None));
        let output = current.clone();
        thread::Builder::new()
            .name("airplay-spectrum".into())
            .spawn(move || {
                let mut analyzer = Analyzer::new();
                let mut bytes = [0_u8; 2 * FFT_SIZE];
                while pcm.read_exact(&mut bytes).is_ok() {
                    let now = Instant::now();
                    let mut current = output.lock().unwrap();
                    if current.is_none_or(|(at, _)| now.duration_since(at) >= SILENCE_TIMEOUT) {
                        analyzer.spectrum = Spectrum::default();
                        analyzer.hold = [0; BAND_COUNT];
                    }
                    *current = Some((now, analyzer.update(&bytes)));
                }
            })?;
        Ok(Self { current })
    }

    pub fn latest(&self, now: Instant) -> Option<Spectrum> {
        self.current
            .lock()
            .unwrap()
            .filter(|(at, _)| now.saturating_duration_since(*at) < SILENCE_TIMEOUT)
            .map(|(_, spectrum)| spectrum)
    }

    pub fn clear(&self) {
        *self.current.lock().unwrap() = None;
    }
}

struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    window: [f32; FFT_SIZE],
    edges: [usize; BAND_COUNT + 1],
    spectrum: Spectrum,
    hold: [u8; BAND_COUNT],
}

impl Analyzer {
    fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let mut edges = [2; BAND_COUNT + 1];
        for band in 1..=BAND_COUNT {
            let ratio = band as f32 / BAND_COUNT as f32;
            let bin = 2.0 * (FFT_SIZE as f32 / 4.0).powf(ratio);
            edges[band] = (bin.round() as usize).max(edges[band - 1] + 1);
        }
        Self {
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            window: array::from_fn(|i| 0.5 - 0.5 * (TAU * i as f32 / FFT_SIZE as f32).cos()),
            edges,
            spectrum: Spectrum::default(),
            hold: [0; BAND_COUNT],
        }
    }

    fn update(&mut self, bytes: &[u8; 2 * FFT_SIZE]) -> Spectrum {
        let mut bins = array::from_fn::<_, FFT_SIZE, _>(|i| {
            let sample = i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
            Complex32::new(f32::from(sample) * self.window[i] / 32_768.0, 0.0)
        });
        self.fft.process_with_scratch(&mut bins, &mut self.scratch);

        for (index, edges) in self.edges.windows(2).enumerate() {
            let power = bins[edges[0]..edges[1]]
                .iter()
                .map(|bin| bin.norm_sqr())
                .fold(0.0_f32, f32::max);
            let magnitude = power.sqrt() * 4.0 / FFT_SIZE as f32;
            let decibels = 20.0 * magnitude.max(1e-6).log10();
            let target = ((decibels + 56.0) / 56.0).clamp(0.0, 1.0);

            let level = target.max(self.spectrum.levels[index] * 0.76);
            self.spectrum.levels[index] = level;
            let peak = &mut self.spectrum.peaks[index];
            if level >= *peak {
                *peak = level;
                self.hold[index] = PEAK_HOLD_FRAMES;
            } else if self.hold[index] > 0 {
                self.hold[index] -= 1;
            } else {
                *peak = (*peak - 0.06).max(level);
            }
        }
        let strength = self.spectrum.levels.iter().copied().fold(0.0, f32::max);
        let bass = self.spectrum.levels[..8]
            .iter()
            .copied()
            .fold(0.0, f32::max);
        self.spectrum.phase = (self.spectrum.phase + 0.035 + 0.06 * strength + 0.10 * bass) % TAU;
        self.spectrum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f32) -> [u8; 2 * FFT_SIZE] {
        let mut pcm = [0; 2 * FFT_SIZE];
        for (i, sample) in pcm.chunks_exact_mut(2).enumerate() {
            let value = (16_000.0 * (TAU * frequency * i as f32 / SAMPLE_RATE as f32).sin()) as i16;
            sample.copy_from_slice(&value.to_le_bytes());
        }
        pcm
    }

    #[test]
    fn audio_frequencies_move_different_bars_and_peaks_decay() {
        for frequency in [125.0, 4000.0] {
            let mut analyzer = Analyzer::new();
            let silence = analyzer.update(&[0; 2 * FFT_SIZE]);
            assert!(silence.levels.iter().all(|&level| level == 0.0));
            let spectrum = analyzer.update(&tone(frequency));
            assert!(spectrum.phase - silence.phase > silence.phase);
            let (band, &level) = spectrum
                .levels
                .iter()
                .enumerate()
                .max_by(|(_, left), (_, right)| left.total_cmp(right))
                .unwrap();
            let bin = (frequency * FFT_SIZE as f32 / SAMPLE_RATE as f32).round() as usize;
            assert!((analyzer.edges[band]..analyzer.edges[band + 1]).contains(&bin));
            assert!(level > 0.8);
            let quiet = analyzer.update(&[0; 2 * FFT_SIZE]);
            assert!(quiet.levels[band] < level);
            assert_eq!(quiet.peaks[band], level);
            for _ in 0..PEAK_HOLD_FRAMES {
                analyzer.update(&[0; 2 * FFT_SIZE]);
            }
            assert!(analyzer.spectrum.peaks[band] < level);
        }
    }
}
