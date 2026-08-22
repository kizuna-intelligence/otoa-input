/// 途中結果を安定させる。
///
/// 認識器は `partial_interval_ms` ごとに、それまでの音声を**丸ごと**再デコードする。
/// この方式しか採れない: Attention Encoder-Decoder は全 encoder フレームへ
/// cross-attend するので、音声が伸びると過去の self-KV も作り直しになる。
///
/// 結果は毎回書き換わるため、そのまま出すと画面の文字が踊る。
/// 隣り合う 2 回の仮説が一致した接頭辞から、さらに末尾 `tail_margin_chars` 文字を
/// 引いた分だけを表示する。
///
/// **一度出した文字も訂正してよい。** 訂正を禁じると、モデルが確定済みの範囲を
/// 言い直した時点で表示が二度と伸びなくなる（長い発話で実際に起きた）。
/// 途中結果はプロトコル上「置き換え」であり、訂正されてよい。
///
/// 余白を大きくすると表示が落ち着く代わりに、最初の文字が出るまでが遅くなる。
/// 4 発話での実測（間隔 125ms）: 余白 0 で 0.44 秒・訂正 13 回、
/// 余白 2 で 1.47 秒・訂正 5 回。既定は速さを採って 0 にしてある。
pub struct Agreement {
    tail_margin_chars: usize,
    previous: String,
    confirmed: String,
}

impl Agreement {
    pub fn new(tail_margin_chars: usize) -> Self {
        Self {
            tail_margin_chars,
            previous: String::new(),
            confirmed: String::new(),
        }
    }

    /// 新しい仮説を入れ、**表示してよいテキスト**を返す。
    /// 返るのは確定済みの接頭辞だけで、未確定の末尾は含めない。
    pub fn observe(&mut self, hypothesis: &str) -> String {
        let agreed = self
            .previous
            .chars()
            .zip(hypothesis.chars())
            .take_while(|(previous, current)| previous == current)
            .count();
        let candidate = hypothesis
            .chars()
            .take(agreed.saturating_sub(self.tail_margin_chars))
            .collect::<String>();

        let candidate_chars = candidate.chars().count();
        let confirmed_chars = self.confirmed.chars().count();
        let candidate_is_true_prefix =
            candidate_chars < confirmed_chars && self.confirmed.starts_with(&candidate);

        if !candidate_is_true_prefix {
            if candidate != self.confirmed && !candidate.starts_with(&self.confirmed) {
                tracing::debug!(
                    confirmed = %self.confirmed,
                    candidate,
                    "モデルが確定済みの接頭辞を書き換えたため、途中結果を訂正します"
                );
            }
            self.confirmed = candidate;
        } else {
            tracing::debug!(
                confirmed = %self.confirmed,
                candidate,
                "仮説が一時的に短くなったため、途中結果を維持します"
            );
        }

        self.previous.clear();
        self.previous.push_str(hypothesis);
        self.confirmed.clone()
    }

    /// 発話が切り替わったら呼ぶ。
    pub fn reset(&mut self) {
        self.previous.clear();
        self.confirmed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Agreement;

    #[test]
    fn measured_hypotheses_never_confirm_the_bad_cursor_text() {
        // stream_result.json の demo_speech.wav に記録された列をそのまま使う。
        let hypotheses = [
            "おん",
            "ニュー",
            "入力のデモ",
            "入力のデモです",
            "入力のデモです",
            "入力のデモです",
            "入力のデモです",
            "入力のデモです",
            "話した内容がそのまま",
            "話した内容がそのままカーソル・イン・アン・アン・アン",
            "話した内容がそのままカーソル位置に貼り込みます",
            "話した内容がそのままカーソル位置に貼り付けられます",
            "話した内容がそのままカーソル位置に貼り付けられます",
            "話した内容がそのままカーソル位置に貼り付けられます",
        ];
        let mut agreement = Agreement::new(8);
        let mut previous_length = 0;

        for hypothesis in hypotheses {
            let confirmed = agreement.observe(hypothesis);
            assert!(!confirmed.contains("カーソル・イン・アン・アン・アン"));
            assert!(confirmed.chars().count() >= previous_length);
            previous_length = confirmed.chars().count();
        }
    }

    #[test]
    fn unicode_inputs_do_not_split_characters() {
        let mut agreement = Agreement::new(1);
        assert_eq!(agreement.observe("▁日本語🙂末尾"), "");
        assert_eq!(agreement.observe("▁日本語🙂末端"), "▁日本語🙂");
    }

    #[test]
    fn corrected_prefix_recovers_and_then_keeps_growing() {
        let mut agreement = Agreement::new(0);
        assert_eq!(agreement.observe("確定する接頭辞"), "");
        assert_eq!(agreement.observe("確定する接頭辞です"), "確定する接頭辞");

        // 最初の発散では共通接頭辞が短いため、一時的な仮説として表示を維持する。
        assert_eq!(
            agreement.observe("訂正した内容に書き換えます"),
            "確定する接頭辞"
        );
        // 訂正された仮説が続けば確定済み範囲も置き換え、その後も伸長できる。
        assert_eq!(
            agreement.observe("訂正した内容に書き換えます"),
            "訂正した内容に書き換えます"
        );
        assert_eq!(
            agreement.observe("訂正した内容に書き換えます。その後も伸びます"),
            "訂正した内容に書き換えます"
        );
    }

    #[test]
    fn reset_removes_the_previous_utterance() {
        let mut agreement = Agreement::new(0);
        agreement.observe("最初の発話");
        assert_eq!(agreement.observe("最初の発話です"), "最初の発話");

        agreement.reset();

        assert_eq!(agreement.observe("次の発話"), "");
        assert_eq!(agreement.observe("次の発話です"), "次の発話");
    }
}
