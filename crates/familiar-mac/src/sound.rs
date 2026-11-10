//! Synthesized feedback sounds: a harsh buzz for mistakes, a soft click for hits.

use rodio::buffer::SamplesBuffer;
use rodio::{OutputStream, OutputStreamBuilder};

const SAMPLE_RATE: u32 = 44_100;

/// The audio output and the two prepared sounds. Without an output device the
/// app simply stays silent.
pub struct Sound {
    stream: Option<OutputStream>,
    punishment: Vec<f32>,
    click: Vec<f32>,
}

impl Sound {
    pub fn new() -> Self {
        let stream = match OutputStreamBuilder::open_default_stream() {
            Ok(mut stream) => {
                stream.log_on_drop(false);
                Some(stream)
            }
            Err(error) => {
                eprintln!("familiar: no audio output, sounds are off: {error}");
                None
            }
        };
        Self {
            stream,
            punishment: punishment_samples(),
            click: click_samples(),
        }
    }

    /// The harsh sound of a mistake.
    pub fn punish(&self) {
        self.play(&self.punishment);
    }

    /// The quiet click of a hit.
    pub fn click(&self) {
        self.play(&self.click);
    }

    fn play(&self, samples: &[f32]) {
        if let Some(stream) = &self.stream {
            stream
                .mixer()
                .add(SamplesBuffer::new(1, SAMPLE_RATE, samples.to_vec()));
        }
    }
}

fn sample_count(milliseconds: u32) -> usize {
    (SAMPLE_RATE * milliseconds / 1000) as usize
}

/// About 250 ms of white noise mixed with a sawtooth sweeping from 90 Hz down
/// to 50 Hz, pushed hard into a soft clipper.
pub fn punishment_samples() -> Vec<f32> {
    let total = sample_count(250);
    let mut noise_state = 0x2545_f491u32;
    let mut phase = 0.0f32;
    (0..total)
        .map(|i| {
            let progress = i as f32 / total as f32;
            // xorshift32: cheap, deterministic white noise.
            noise_state ^= noise_state << 13;
            noise_state ^= noise_state >> 17;
            noise_state ^= noise_state << 5;
            let noise = noise_state as f32 / u32::MAX as f32 * 2.0 - 1.0;

            let frequency = 90.0 - 40.0 * progress;
            phase = (phase + frequency / SAMPLE_RATE as f32).fract();
            let saw = phase * 2.0 - 1.0;

            // 4 ms attack, 40 ms release, to avoid clicks at the edges.
            let attack = (progress * 250.0 / 4.0).min(1.0);
            let release = ((1.0 - progress) * 250.0 / 40.0).min(1.0);
            let overdriven = ((0.45 * noise + 0.9 * saw) * 4.0).tanh();
            0.8 * overdriven * attack * release
        })
        .collect()
}

/// A 25 ms decaying 1.8 kHz tick, quiet on purpose.
pub fn click_samples() -> Vec<f32> {
    let total = sample_count(25);
    (0..total)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            let decay = (-t * 220.0).exp();
            0.16 * decay * (std::f32::consts::TAU * 1800.0 * t).sin()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |max, s| max.max(s.abs()))
    }

    #[test]
    fn punishment_is_a_quarter_second_loud_and_never_clips() {
        let samples = punishment_samples();
        assert_eq!(samples.len(), 11_025);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(peak(&samples) > 0.7);
        // It starts and ends near silence.
        assert!(samples[0].abs() < 0.05 && samples[samples.len() - 1].abs() < 0.05);
        assert_eq!(samples, punishment_samples(), "the sound is deterministic");
    }

    #[test]
    fn click_is_short_and_much_quieter_than_the_punishment() {
        let click = click_samples();
        assert_eq!(click.len(), 1102);
        assert!(peak(&click) > 0.05);
        assert!(peak(&click) < peak(&punishment_samples()) / 4.0);
        assert!(click[click.len() - 1].abs() < 0.001);
    }
}
