use anyhow::{anyhow, bail, Result};
use std::{collections::HashMap, env, fmt, path::PathBuf};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8770;
const DEFAULT_PATH: &str = "/asr/v1";
const DEFAULT_ASR_THREADS: usize = 2;
const DEFAULT_MAX_UTTERANCE_MS: u32 = 25_000;
const DEFAULT_PARTIAL_INTERVAL_MS: u32 = 125;
const DEFAULT_PARTIAL_TAIL_MARGIN_CHARS: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AsrEngine {
    K2,
    Kodama,
}

impl std::str::FromStr for AsrEngine {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "reazonspeech" => Ok(Self::K2),
            "kodama" => Ok(Self::Kodama),
            _ => Err(format!(
                "認識エンジン {value:?} は使えません。reazonspeech か kodama を指定してください。"
            )),
        }
    }
}

/// Configuration for the Otoa ASR server.
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub path: String,

    pub asr_engine: AsrEngine,
    pub asr_model_dir: PathBuf,

    pub asr_threads: usize,

    /// クライアントが `finalize` を送らないまま話し続けた場合の安全弁。
    /// ASR は一度に 30 秒程度までしか扱えない。
    pub max_utterance_ms: u32,

    /// 途中経過を出すか。kodama は途中の音声でも使える結果を返すので既定で有効、
    /// reazonspeech は非ストリーミングのモデルなので既定で無効。
    pub pseudo_stream: bool,
    /// 途中経過のために再デコードする間隔。短いほど早く表示できる。
    /// 1 回の再デコードがこの間隔より長くかかる場合は、終わるまで次を投げない。
    pub partial_interval_ms: u32,
    /// 途中結果として**表示しない**末尾の文字数。
    /// 隣り合う 2 回の仮説が一致した部分から、さらにこの分だけ削って表示する。
    /// 大きいほど表示が落ち着くが、最初の文字が出るまでが遅くなる。
    pub partial_tail_margin_chars: usize,

    pub auth_token: Option<String>,

    /// 指定すると、認識に渡した音声をこのディレクトリへ WAV で書き出す。
    /// 「先頭が欠ける」といった不具合の切り分け用で、既定では書かない。
    pub dump_dir: Option<PathBuf>,
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field("asr_engine", &self.asr_engine)
            .field("asr_model_dir", &self.asr_model_dir)
            .field("asr_threads", &self.asr_threads)
            .field("max_utterance_ms", &self.max_utterance_ms)
            .field("pseudo_stream", &self.pseudo_stream)
            .field("partial_interval_ms", &self.partial_interval_ms)
            .field("partial_tail_margin_chars", &self.partial_tail_margin_chars)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "***"))
            .field("dump_dir", &self.dump_dir)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            path: DEFAULT_PATH.to_string(),
            asr_engine: AsrEngine::K2,
            asr_model_dir: PathBuf::from("models/reazonspeech-k2-v2"),
            asr_threads: DEFAULT_ASR_THREADS,
            max_utterance_ms: DEFAULT_MAX_UTTERANCE_MS,
            pseudo_stream: false,
            partial_interval_ms: DEFAULT_PARTIAL_INTERVAL_MS,
            partial_tail_margin_chars: DEFAULT_PARTIAL_TAIL_MARGIN_CHARS,
            auth_token: None,
            dump_dir: None,
        }
    }
}

impl Config {
    /// Resolve arguments over `OTOA_ASR_*` environment variables and defaults.
    pub fn from_sources(args: &[String], environment: &HashMap<String, String>) -> Result<Self> {
        let arguments = parse_arguments(args)?;
        let mut config = Self::default();

        config.host = string_value("host", &arguments, environment, config.host)?;
        config.port = parse_value("port", &arguments, environment, config.port)?;
        config.path = string_value("path", &arguments, environment, config.path)?;
        config.asr_engine = parse_value("asr_engine", &arguments, environment, config.asr_engine)?;
        let default_model_dir = match config.asr_engine {
            AsrEngine::K2 => config.asr_model_dir.clone(),
            AsrEngine::Kodama => PathBuf::from("models/kodama-ja-streaming-small"),
        };
        config.asr_model_dir =
            path_value("asr_model_dir", &arguments, environment, default_model_dir)?;
        config.asr_threads =
            parse_value("asr_threads", &arguments, environment, config.asr_threads)?;
        config.max_utterance_ms = parse_value(
            "max_utterance_ms",
            &arguments,
            environment,
            config.max_utterance_ms,
        )?;
        let default_pseudo_stream = config.asr_engine == AsrEngine::Kodama;
        config.pseudo_stream = parse_bool_value(
            "pseudo_stream",
            &arguments,
            environment,
            default_pseudo_stream,
        )?;
        config.partial_interval_ms = parse_value(
            "partial_interval_ms",
            &arguments,
            environment,
            config.partial_interval_ms,
        )?;
        config.partial_tail_margin_chars = parse_value(
            "partial_tail_margin_chars",
            &arguments,
            environment,
            config.partial_tail_margin_chars,
        )?;

        config.dump_dir =
            optional_string_value("dump_dir", &arguments, environment)?.map(PathBuf::from);

        let auth_token = optional_string_value("auth_token", &arguments, environment)?;
        if auth_token.is_some() || arguments.contains_key("auth_token") {
            config.auth_token = auth_token;
        }

        config.validate()?;
        Ok(config)
    }

    pub fn from_process_args() -> Result<Self> {
        let args = env::args().skip(1).collect::<Vec<_>>();
        let environment = env::vars().collect::<HashMap<_, _>>();
        Self::from_sources(&args, &environment)
    }

    /// `--help` / `-h` が渡されたか。最初に打たれるコマンドなので、
    /// 未知のオプション扱いでエラーにしない。
    pub fn help_requested(args: &[String]) -> bool {
        args.iter()
            .any(|argument| argument == "--help" || argument == "-h")
    }

    fn validate(&self) -> Result<()> {
        if self.host.is_empty() {
            bail!("host must not be empty");
        }
        if self.port == 0 {
            bail!("port must be between 1 and 65535");
        }
        if !self.path.starts_with('/') {
            bail!("path must start with '/'");
        }
        if self.asr_threads == 0 {
            bail!("asr_threads must be positive");
        }
        if self.max_utterance_ms == 0 {
            bail!("max_utterance_ms must be positive");
        }
        if self.partial_interval_ms == 0 {
            bail!("partial_interval_ms must be positive");
        }
        Ok(())
    }
}

/// `--help` の本文。オプションを増やしたらここも足す。
pub const USAGE: &str = "\
otoa-asr-server — Otoa ASR Protocol v1 のサーバー

使い方:
  otoa-asr-server --asr-model-dir=<dir> [オプション]

オプション:
  --host=<addr>               待ち受けアドレス (既定: 127.0.0.1)
  --port=<port>               待ち受けポート (既定: 8770)
  --path=<path>               WebSocket のパス (既定: /asr/v1)
  --asr-engine=<reazonspeech|kodama>
                              認識エンジン (既定: reazonspeech)
  --asr-model-dir=<dir>       認識モデルのファイルを置いたディレクトリ
  --asr-threads=<n>           認識スレッド数 (既定: 2)
  --max-utterance-ms=<ms>     finalize が来ないまま話し続けた場合の上限
                              (既定: 25000)
  --partial-interval-ms=<ms>  途中経過を出す間隔 (既定: 125)
  --partial-tail-margin-chars=<n>
                              途中結果の未確定末尾として残す文字数 (既定: 0)
  --pseudo-stream[=<bool>]    途中経過の再デコードを有効にする
                              (既定: kodama は true、reazonspeech は false)
  --auth-token=<token>        Authorization: Bearer に要求するトークン
  --dump-dir=<dir>            認識へ渡した音声を WAV で書き出す (調査用)
  -h, --help                  この使い方を表示する

すべてのオプションは環境変数でも指定できる。--asr-model-dir なら
OTOA_ASR_ASR_MODEL_DIR。指定の優先順位はコマンド引数、環境変数、既定値。

発話の区切りはクライアントが finalize で決める。このサーバーは終話を
判定しないので、設定 JSON の endpoint_mode は client のみ受け付ける。
";

fn parse_arguments(args: &[String]) -> Result<HashMap<String, String>> {
    let known = [
        "host",
        "port",
        "path",
        "asr_engine",
        "asr_model_dir",
        "asr_threads",
        "max_utterance_ms",
        "pseudo_stream",
        "partial_interval_ms",
        "partial_tail_margin_chars",
        "auth_token",
        "dump_dir",
    ];
    let mut values = HashMap::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        let Some(option) = argument.strip_prefix("--") else {
            bail!("unexpected argument: {argument}");
        };
        let (name, inline_value) = option
            .split_once('=')
            .map_or((option, None), |(name, value)| (name, Some(value)));
        let name = name.replace('-', "_");
        if !known.contains(&name.as_str()) {
            bail!("unknown option: --{}", name.replace('_', "-"));
        }

        let value = if let Some(value) = inline_value {
            value.to_string()
        } else if name == "pseudo_stream" {
            if args
                .get(index + 1)
                .is_some_and(|next| !next.starts_with('-'))
            {
                index += 1;
                args[index].clone()
            } else {
                "true".to_string()
            }
        } else {
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| anyhow!("option --{} requires a value", name.replace('_', "-")))?
        };
        values.insert(name, value);
        index += 1;
    }
    Ok(values)
}

fn resolved_value(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
) -> Option<String> {
    arguments.get(name).cloned().or_else(|| {
        environment
            .get(&format!("OTOA_ASR_{}", name.to_ascii_uppercase()))
            .cloned()
    })
}

fn string_value(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
    default: String,
) -> Result<String> {
    Ok(resolved_value(name, arguments, environment).unwrap_or(default))
}

fn path_value(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
    default: PathBuf,
) -> Result<PathBuf> {
    Ok(resolved_value(name, arguments, environment).map_or(default, PathBuf::from))
}

fn parse_value<T>(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
    default: T,
) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    let Some(value) = resolved_value(name, arguments, environment) else {
        return Ok(default);
    };
    value.parse::<T>().map_err(|error| {
        anyhow!(
            "OTOA_ASR_{} has an invalid value {:?}: {}",
            name.to_ascii_uppercase(),
            value,
            error
        )
    })
}

fn parse_bool_value(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
    default: bool,
) -> Result<bool> {
    let Some(value) = resolved_value(name, arguments, environment) else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!(
            "OTOA_ASR_{} has an invalid boolean value {:?}",
            name.to_ascii_uppercase(),
            value
        ),
    }
}

fn optional_string_value(
    name: &str,
    arguments: &HashMap<String, String>,
    environment: &HashMap<String, String>,
) -> Result<Option<String>> {
    Ok(
        resolved_value(name, arguments, environment).and_then(|value| {
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{AsrEngine, Config};
    use std::collections::HashMap;

    #[test]
    fn defaults_keep_design_values() {
        let config = Config::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8770);
        assert_eq!(config.path, "/asr/v1");
        assert_eq!(config.asr_engine, AsrEngine::K2);
        assert_eq!(
            config.asr_model_dir,
            std::path::PathBuf::from("models/reazonspeech-k2-v2")
        );
        assert_eq!(config.asr_threads, 2);
        assert_eq!(config.max_utterance_ms, 25_000);
        assert_eq!(config.partial_interval_ms, 125);
        assert_eq!(config.partial_tail_margin_chars, 0);
        assert!(!config.pseudo_stream);
    }

    #[test]
    fn kodama_changes_engine_specific_defaults() {
        let args = vec!["--asr-engine=kodama".to_string()];
        let config = Config::from_sources(&args, &HashMap::new()).expect("config should parse");
        assert_eq!(config.asr_engine, AsrEngine::Kodama);
        assert_eq!(
            config.asr_model_dir,
            std::path::PathBuf::from("models/kodama-ja-streaming-small")
        );
        assert!(config.pseudo_stream);

        let args = vec![
            "--asr-engine=kodama".to_string(),
            "--asr-model-dir=/tmp/custom".to_string(),
        ];
        let config = Config::from_sources(&args, &HashMap::new()).expect("config should parse");
        assert_eq!(
            config.asr_model_dir,
            std::path::PathBuf::from("/tmp/custom")
        );

        let args = vec![
            "--asr-engine=kodama".to_string(),
            "--pseudo-stream=false".to_string(),
        ];
        let config = Config::from_sources(&args, &HashMap::new()).expect("config should parse");
        assert!(!config.pseudo_stream);
    }

    #[test]
    fn tail_margin_accepts_cli_environment_and_zero() {
        let args = vec!["--partial-tail-margin-chars=0".to_string()];
        let config = Config::from_sources(&args, &HashMap::new()).expect("zero should be valid");
        assert_eq!(config.partial_tail_margin_chars, 0);

        let environment = HashMap::from([(
            "OTOA_ASR_PARTIAL_TAIL_MARGIN_CHARS".to_string(),
            "12".to_string(),
        )]);
        let config = Config::from_sources(&[], &environment).expect("config should parse");
        assert_eq!(config.partial_tail_margin_chars, 12);
    }

    #[test]
    fn engine_names_are_shared_by_cli_and_bundled_server() {
        assert_eq!("reazonspeech".parse(), Ok(AsrEngine::K2));
        assert_eq!("kodama".parse(), Ok(AsrEngine::Kodama));

        let error = "whisper"
            .parse::<AsrEngine>()
            .expect_err("unknown engine should fail");
        assert!(error.contains("reazonspeech か kodama"));
    }

    #[test]
    fn arguments_override_environment() {
        let mut environment = HashMap::new();
        environment.insert("OTOA_ASR_PORT".to_string(), "9000".to_string());
        environment.insert("OTOA_ASR_MAX_UTTERANCE_MS".to_string(), "20000".to_string());
        environment.insert("OTOA_ASR_ASR_THREADS".to_string(), "4".to_string());
        environment.insert("OTOA_ASR_PSEUDO_STREAM".to_string(), "true".to_string());
        let environment_config =
            Config::from_sources(&[], &environment).expect("config should parse");
        assert_eq!(environment_config.port, 9000);
        assert_eq!(environment_config.max_utterance_ms, 20_000);
        assert_eq!(environment_config.asr_threads, 4);

        let args = vec![
            "--port".to_string(),
            "9001".to_string(),
            "--max-utterance-ms".to_string(),
            "18000".to_string(),
            "--asr-threads=3".to_string(),
        ];
        let config = Config::from_sources(&args, &environment).expect("config should parse");
        assert_eq!(config.port, 9001);
        assert_eq!(config.max_utterance_ms, 18_000);
        assert_eq!(config.asr_threads, 3);
        assert!(config.pseudo_stream);
    }
}
