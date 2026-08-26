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
    Closing,
    /// 失敗。ユーザー操作か再試行で戻る。
    Failed,
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
    disable_after_close: bool,
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: SessionState::Disabled,
            disable_after_close: false,
        }
    }

    pub fn state(&self) -> SessionState {
        self.state
    }

    /// 遷移する。許可されない入力は状態を変えず `false` を返す。
    pub fn apply(&mut self, input: SessionInput) -> bool {
        if input == SessionInput::Enable {
            self.disable_after_close = false;
        }

        let next = match (self.state, input) {
            (SessionState::Disabled, SessionInput::Enable) => SessionState::Listening,
            (SessionState::Listening, SessionInput::SpeechStarted) => SessionState::Connecting,
            (SessionState::Listening, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Listening, SessionInput::Disable) => SessionState::Disabled,
            (SessionState::Disabled, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Connecting, SessionInput::Connected) => SessionState::Streaming,
            (SessionState::Connecting, SessionInput::Disable) => SessionState::Closing,
            (SessionState::Connecting, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Streaming, SessionInput::IdleTimeout) => SessionState::Closing,
            (SessionState::Streaming, SessionInput::Disable) => SessionState::Closing,
            (SessionState::Streaming, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Closing, SessionInput::Disable) => {
                self.disable_after_close = true;
                SessionState::Closing
            }
            (SessionState::Closing, SessionInput::Enable) => SessionState::Closing,
            (SessionState::Closing, SessionInput::Finished) => {
                let next = if self.disable_after_close {
                    SessionState::Disabled
                } else {
                    SessionState::Listening
                };
                self.disable_after_close = false;
                next
            }
            (SessionState::Closing, SessionInput::Failed) => SessionState::Failed,
            (SessionState::Connecting, SessionInput::Timeout)
            | (SessionState::Closing, SessionInput::Timeout) => {
                self.disable_after_close = false;
                SessionState::Listening
            }
            (SessionState::Connecting, SessionInput::Aborted)
            | (SessionState::Streaming, SessionInput::Aborted)
            | (SessionState::Closing, SessionInput::Aborted) => {
                self.disable_after_close = false;
                SessionState::Listening
            }
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
        self.state != SessionState::Disabled
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

    fn session_in_closing() -> Session {
        let mut session = Session::new();
        assert!(session.apply(SessionInput::Enable));
        assert!(session.apply(SessionInput::SpeechStarted));
        assert!(session.apply(SessionInput::Connected));
        assert!(session.apply(SessionInput::Disable));
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
    fn disable_during_closing_ends_disabled() {
        let mut session = session_in_closing();
        assert!(session.apply(SessionInput::Disable));
        assert_eq!(session.state(), SessionState::Closing);
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Disabled);
    }

    #[test]
    fn closing_without_disable_returns_listening() {
        let mut session = session_in_closing();
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn enable_clears_pending_disable() {
        let mut session = session_in_closing();
        assert!(session.apply(SessionInput::Disable));
        assert!(session.apply(SessionInput::Enable));
        assert_eq!(session.state(), SessionState::Closing);
        assert!(session.apply(SessionInput::Finished));
        assert_eq!(session.state(), SessionState::Listening);
    }

    #[test]
    fn closing_timeout_returns_to_listening_and_accepts_new_speech() {
        let mut session = session_in_closing();
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
                assert!(session.apply(SessionInput::Disable));
            }

            assert!(session.apply(SessionInput::Aborted));
            assert_eq!(session.state(), SessionState::Listening);
        }
    }
}
