#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateEvent {
    SpeechStarted,
    SpeechEnded,
}

pub struct SpeechGate {
    threshold: f32,
    min_speech_frames: usize,
    min_silence_frames: usize,
    speaking: bool,
    run: usize,
}

impl SpeechGate {
    pub fn new(threshold: f32, min_speech_frames: usize, min_silence_frames: usize) -> Self {
        Self {
            threshold,
            min_speech_frames,
            min_silence_frames: min_silence_frames.max(1),
            speaking: false,
            run: 0,
        }
    }

    /// 1 フレーム分の発話確率を渡す。状態が変わったときだけイベントを返す。
    pub fn push(&mut self, prob: f32) -> Option<GateEvent> {
        let over = prob >= self.threshold;
        if !self.speaking {
            if over {
                self.run += 1;
            } else {
                self.run = 0;
            }
            if self.run >= self.min_speech_frames {
                self.speaking = true;
                self.run = 0;
                return Some(GateEvent::SpeechStarted);
            }
            return None;
        }

        // 発話中。無音が続いたら「次の発話を検知できる状態」へ戻す。
        // この SpeechEnded は ASR セッションの終了には使わず、次の発話の検知にだけ使う。
        // 発話の区切りは ASR サーバーからの `<end>` で確定する。
        // ここを消すと speaking が true のままになり、二度と SpeechStarted が出ない。
        if over {
            self.run = 0;
        } else {
            self.run += 1;
            if self.run >= self.min_silence_frames {
                self.speaking = false;
                self.run = 0;
                return Some(GateEvent::SpeechEnded);
            }
        }
        None
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    pub fn reset(&mut self) {
        self.speaking = false;
        self.run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{GateEvent, SpeechGate};

    #[test]
    fn no_event_below_min_speech() {
        let mut gate = SpeechGate::new(0.5, 3, 3);
        assert_eq!(gate.push(0.5), None);
        assert_eq!(gate.push(0.6), None);
        assert!(!gate.is_speaking());
    }

    #[test]
    fn emits_started_at_min_speech() {
        let mut gate = SpeechGate::new(0.5, 3, 3);
        assert_eq!(gate.push(0.5), None);
        assert_eq!(gate.push(0.7), None);
        assert_eq!(gate.push(0.9), Some(GateEvent::SpeechStarted));
    }

    #[test]
    fn run_resets_on_gap() {
        let mut gate = SpeechGate::new(0.5, 3, 3);
        assert_eq!(gate.push(0.8), None);
        assert_eq!(gate.push(0.8), None);
        assert_eq!(gate.push(0.2), None);
        assert_eq!(gate.push(0.8), None);
        assert!(!gate.is_speaking());
    }

    #[test]
    fn silence_does_not_emit_end() {
        let mut gate = SpeechGate::new(0.5, 1, 3);
        assert_eq!(gate.push(0.8), Some(GateEvent::SpeechStarted));
        assert_eq!(gate.push(0.2), None);
        assert_eq!(gate.push(0.2), None);
        assert!(gate.is_speaking());
    }

    #[test]
    fn threshold_is_inclusive() {
        let mut gate = SpeechGate::new(0.5, 1, 3);
        assert_eq!(gate.push(0.5), Some(GateEvent::SpeechStarted));
    }

    #[test]
    fn no_duplicate_started() {
        let mut gate = SpeechGate::new(0.5, 1, 3);
        assert_eq!(gate.push(0.8), Some(GateEvent::SpeechStarted));
        assert_eq!(gate.push(0.8), None);
        assert_eq!(gate.push(0.8), None);
    }

    #[test]
    fn rearms_after_silence_so_second_utterance_is_detected() {
        // 回帰テスト: 一度発話を検知したあと無音が続けば、
        // 次の発話でも SpeechStarted が出ること。
        // これが壊れると、起動後に発話を 1 回しか検知できなくなる。
        let mut gate = SpeechGate::new(0.5, 2, 3);
        assert_eq!(gate.push(0.9), None);
        assert_eq!(gate.push(0.9), Some(GateEvent::SpeechStarted));
        assert!(gate.is_speaking());

        assert_eq!(gate.push(0.0), None);
        assert_eq!(gate.push(0.0), None);
        assert_eq!(gate.push(0.0), Some(GateEvent::SpeechEnded));
        assert!(!gate.is_speaking());

        assert_eq!(gate.push(0.9), None);
        assert_eq!(gate.push(0.9), Some(GateEvent::SpeechStarted));
    }

    #[test]
    fn brief_dip_does_not_end_speech() {
        let mut gate = SpeechGate::new(0.5, 1, 3);
        assert_eq!(gate.push(0.9), Some(GateEvent::SpeechStarted));
        assert_eq!(gate.push(0.0), None);
        assert_eq!(gate.push(0.9), None);
        assert_eq!(gate.push(0.0), None);
        assert_eq!(gate.push(0.0), None);
        assert_eq!(gate.push(0.0), Some(GateEvent::SpeechEnded));
    }
}
