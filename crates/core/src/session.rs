#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// 待受を止めている。マイクも VAD も動かさない。
    Disabled,
    /// マイクと VAD は動いているが、ASR 接続先へは繋いでいない。
    Listening,
    /// 発話を検知して接続中。音声はバッファする。
    Connecting,
    /// ASR 接続先へ音声を送っている。
    Streaming,
    /// 停止要求済み。ASR セッションの完了通知（`finished`）待ち。
    /// 完了後は待受へ戻る。
    Closing,
    /// 利用者が待受を止め、ASR セッションの完了通知（`finished`）待ち。
    /// 完了後は停止する。
    Stopping,
    /// 失敗。ユーザー操作か再試行で戻る。
    Failed,
}

impl SessionState {
    /// 利用者が待受を有効にしている状態か。
    ///
    /// `Stopping` は接続の終了待ちでも、希望状態は停止である。
    pub fn listening_enabled(self) -> bool {
        !matches!(self, Self::Disabled | Self::Stopping)
    }

    /// ASR 接続の終了を待っている状態か。
    pub fn is_closing(self) -> bool {
        matches!(self, Self::Closing | Self::Stopping)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionInput {
    Enable,
    Disable,
    SpeechStarted,
    Connected,
    /// 最後の発話区切り通知から長く経過したので閉じる。
    IdleTimeout,
    Finished,
    /// 接続または終了待ちを諦めて待受へ戻す。
    Timeout,
    /// ASR セッションが切断されたので、失敗扱いにせず待受へ戻す。
    Aborted,
    Failed,
    /// Failed からの復帰。
    Retry,
}

#[derive(Debug)]
pub struct Session {
    state: SessionState,
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: SessionState::Disabled,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// 遷移する。許可されない入力は状態を変えず `false` を返す。
    pub fn apply(&mut self, input: SessionInput) -> bool {
        let next = match (self.state, input) {
            (SessionState::Disabled, SessionInput::Enable) => SessionState::Listening,
            (SessionState::Listening, SessionInput::SpeechStarted) => SessionState::Connecting,
            (SessionState::Listening, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Listening, SessionInput::Disable) => SessionState::Disabled,
            (SessionState::Disabled, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Connecting, SessionInput::Connected) => SessionState::Streaming,
            (SessionState::Connecting, SessionInput::Disable) => SessionState::Stopping,
            (SessionState::Connecting, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Streaming, SessionInput::IdleTimeout) => SessionState::Closing,
            (SessionState::Streaming, SessionInput::Disable) => SessionState::Stopping,
            (SessionState::Streaming, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Closing, SessionInput::Disable) => SessionState::Stopping,
            (SessionState::Closing, SessionInput::Enable) => SessionState::Closing,
            (SessionState::Stopping, SessionInput::Disable) => SessionState::Stopping,
            (SessionState::Stopping, SessionInput::Enable) => SessionState::Closing,
            (SessionState::Closing, SessionInput::Finished) => SessionState::Listening,
            (SessionState::Stopping, SessionInput::Finished) => SessionState::Disabled,
            (SessionState::Closing, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Stopping, SessionInput::Failed) => SessionState::Disabled,
            (SessionState::Connecting, SessionInput::Timeout)
            | (SessionState::Closing, SessionInput::Timeout) => SessionState::Listening,
            (SessionState::Stopping, SessionInput::Timeout) => SessionState::Disabled,
            (SessionState::Connecting, SessionInput::Aborted)
            | (SessionState::Streaming, SessionInput::Aborted)
            | (SessionState::Closing, SessionInput::Aborted) => SessionState::Listening,
            (SessionState::Stopping, SessionInput::Aborted) => SessionState::Disabled,
            (SessionState::Failed, SessionInput::Retry) => SessionState::Listening,
            (SessionState::Failed, SessionInput::Disable) => SessionState::Disabled,
            _ => return false,
        };

        self.state = next;
        true
    }

    /// この状態で ASR 接続先へ音声を送ってよいか。
    pub fn accepts_audio(&self) -> bool {
        matches!(
            self.state,
            SessionState::Connecting | SessionState::Streaming
        )
    }

    /// この状態でマイクと VAD を回すか。
    pub fn is_listening(&self) -> bool {
        self.state.listening_enabled()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SessionInput, SessionState};

    fn session_closing_after_idle() -> Session {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert!(session.apply(SessionInput::IdleTimeout));
        assert_eq!(session.state(), SessionState::Closing);
        session
    }

    #[test]
    fn listening_to_streaming_path() {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert_eq!(session.state(), SessionState::Streaming);
    }

    #[test]
    fn streaming_idle_timeout_closes() {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert!(session.apply(SessionInput::IdleTimeout));
        assert_eq!(session.state(), SessionState::Closing);
        assert!(!session.accepts_audio());
    }

    #[test]
    fn disabled_stops_listening() {
        let session = Session::new();
        assert_eq!(session.state(), SessionState::Disabled);
        assert!(!session.is_listening());
    }

    #[test]
    fn invalid_transition_is_rejected() {
        let mut session = Session::new();
        assert!(!session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Disabled);
    }

    #[test]
    fn listening_can_fail() {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::Failed));
        assert_eq!(session.state(), SessionState::Failed);
    }

    #[test]
    fn disabling_a_streaming_session_ends_disabled() {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert!(session.apply(SessionInput::Disable));
        assert_eq!(session.state(), SessionState::Stopping);
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Disabled);
    }

    #[test]
    fn idle_close_returns_listening() {
        let mut session = session_closing_after_idle();
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn enabling_while_stopping_returns_listening() {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert!(session.apply(SessionInput::Disable));
        assert_eq!(session.state(), SessionState::Stopping);
        assert!(session.apply(SessionInput::Enable));
        assert_eq!(session.state(), SessionState::Closing);
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn closing_timeout_returns_to_listening_and_accepts_new_speech() {
        let mut session = session_closing_after_idle();
        assert!(session.apply(SessionInput::Timeout));
        assert_eq!(session.state(), SessionState::Listening);
        assert!(session.apply(SessionInput::SpeechStarted));
        assert_eq!(session.state(), SessionState::Connecting);
    }

    #[test]
    fn aborted_asr_session_returns_to_listening() {
        for state in [
            SessionState::Connecting,
            SessionState::Streaming,
            SessionState::Closing,
        ] {
            let mut session = Session::new();
            assert!(session.apply(SessionInput::Enable));
            assert!(session.apply(SessionInput::SpeechStarted));
            if state == SessionState::Streaming || state == SessionState::Closing {
                assert!(session.apply(SessionInput::Connected));
            }
            if state == SessionState::Closing {
                assert!(session.apply(SessionInput::IdleTimeout));
            }

            assert!(session.apply(SessionInput::Aborted));
            assert_eq!(session.state(), SessionState::Listening);
        }
    }

    #[test]
    fn stopping_completion_failures_stay_disabled() {
        for end in [
            SessionInput::Timeout,
            SessionInput::Aborted,
            SessionInput::Failed,
        ] {
            let mut session = Session::new();
            assert!(session.apply(SessionInput::Enable));
            assert!(session.apply(SessionInput::SpeechStarted));
            assert!(session.apply(SessionInput::Connected));
            assert!(session.apply(SessionInput::Disable));
            assert_eq!(session.state(), SessionState::Stopping);
            assert!(session.apply(end));
            assert_eq!(session.state(), SessionState::Disabled);
        }
    }
}
