/// 線形補間で任意レートから 16 kHz へ落とす。状態を持つので発話をまたいで使い回す。
pub struct Resampler {
    src_rate: u32,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(src_rate: u32) -> Self {
        Self {
            src_rate: src_rate.max(1),
            pos: 1.0,
            last: 0.0,
        }
    }

    /// f32 サンプル列を 16 kHz の i16 列へ変換して push する。
    pub fn push(&mut self, input: &[f32], out: &mut Vec<i16>) {
        if input.is_empty() {
            return;
        }

        if self.src_rate == 16_000 {
            out.extend(input.iter().copied().map(sample_to_i16));
            self.last = *input.last().unwrap_or(&self.last);
            self.pos = 0.0;
            return;
        }

        let step = self.src_rate as f64 / 16_000.0;
        let mut position = self.pos;
        let mut previous = self.last;

        for &current in input {
            while position < 1.0 {
                let fraction = position as f32;
                let sample = previous + (current - previous) * fraction;
                out.push(sample_to_i16(sample));
                position += step;
            }
            position -= 1.0;
            previous = current;
        }

        self.pos = position;
        self.last = previous;
    }
}

fn sample_to_i16(sample: f32) -> i16 {
    if sample <= -1.0 {
        i16::MIN
    } else if sample >= 1.0 {
        i16::MAX
    } else {
        (sample * i16::MAX as f32).round() as i16
    }
}
