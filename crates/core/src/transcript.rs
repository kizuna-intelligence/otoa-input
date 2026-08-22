/// 進行中の文字起こし。確定分と未確定分を別に持つ。
#[derive(Debug, Default)]
pub struct Transcript {
    /// 現在の発話区間で確定した文字列。
    segment: String,
    /// 未確定文字列。次の更新で丸ごと差し替わる。
    partial: String,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// 確定テキストを追記する。
    pub fn push_final(&mut self, text: &str) {
        self.segment.push_str(text);
    }

    /// 未確定テキストを差し替える。追記ではない。
    pub fn replace_partial(&mut self, text: &str) {
        self.partial.clear();
        self.partial.push_str(text);
    }

    /// 表示用の全文（確定 + 未確定）。
    pub fn display(&self) -> String {
        let mut display = String::with_capacity(self.segment.len() + self.partial.len());
        display.push_str(&self.segment);
        display.push_str(&self.partial);
        display
    }

    pub fn committed(&self) -> &str {
        &self.segment
    }

    pub fn partial(&self) -> &str {
        &self.partial
    }

    /// 発話が終わった。確定分を取り出し、内部を空にする。
    /// 未確定分は破棄する。
    pub fn take_segment(&mut self) -> Option<String> {
        let segment = std::mem::take(&mut self.segment);
        self.partial.clear();
        if segment.trim().is_empty() {
            None
        } else {
            Some(segment)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.segment.is_empty() && self.partial.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::Transcript;

    #[test]
    fn partial_is_replaced_not_appended() {
        let mut transcript = Transcript::new();
        transcript.replace_partial("こんにちは");
        transcript.replace_partial("こんばんは");
        assert_eq!(transcript.display(), "こんばんは");
    }

    #[test]
    fn final_is_appended() {
        let mut transcript = Transcript::new();
        transcript.push_final("こん");
        transcript.push_final("にちは");
        assert_eq!(transcript.display(), "こんにちは");
    }

    #[test]
    fn take_segment_clears_partial() {
        let mut transcript = Transcript::new();
        transcript.push_final("確定");
        transcript.replace_partial("未確定");
        assert_eq!(transcript.take_segment(), Some("確定".to_string()));
        assert!(transcript.display().is_empty());
    }

    #[test]
    fn take_segment_returns_none_for_blank() {
        let mut transcript = Transcript::new();
        transcript.push_final("  \n\t");
        assert_eq!(transcript.take_segment(), None);
        assert!(transcript.is_empty());
    }
}
