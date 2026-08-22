#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("websocket connect failed: {0}")]
    Connect(String),
    #[error("websocket io error: {0}")]
    Io(String),
    #[error("invalid response json: {0}")]
    Decode(String),
    /// server が error frame を返した。
    #[error("ASR error {error_type} ({code}): {message}")]
    Server {
        code: u32,
        error_type: String,
        message: String,
        request_id: Option<String>,
    },
    #[error("connection closed by server before finished")]
    ClosedEarly,
}
