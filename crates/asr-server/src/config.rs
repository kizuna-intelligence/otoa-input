use anyhow::{anyhow, bail, Result};
use std::{collections::HashMap, env, fmt, path::PathBuf};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8770;
const DEFAULT_PATH: &str = "/asr/v1";
const DEFAULT_ASR_THREADS: usize = 2;
const DEFAULT_MAX_UTTERANCE_MS: u32 = 25_000;
const DEFAULT_PARTIAL_INTERVAL_MS: u32 = 500;

/// Configuration for the Otoa ASR server.
#[derive(Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub path: String,

    pub asr_model_dir: PathBuf,

    pub asr_threads: usize,

    /// クライアントが `finalize` を送らないまま話し続けた場合の安全弁。
    /// ASR は一度に 30 秒程度までしか扱えない。
    pub max_utterance_ms: u32,

    pub pseudo_stream: bool,
    pub partial_interval_ms: u32,

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
            .field("asr_model_dir", &self.asr_model_dir)
            .field("asr_threads", &self.asr_threads)
            .field("max_utterance_ms", &self.max_utterance_ms)
            .field("pseudo_stream", &self.pseudo_stream)
            .field("partial_interval_ms", &self.partial_interval_ms)
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
            asr_model_dir: PathBuf::from("models/reazonspeech-k2-v2"),
            asr_threads: DEFAULT_ASR_THREADS,
            max_utterance_ms: DEFAULT_MAX_UTTERANCE_MS,
            pseudo_stream: false,
            partial_interval_ms: DEFAULT_PARTIAL_INTERVAL_MS,
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
        config.asr_model_dir = path_value(
            "asr_model_dir",
            &arguments,
            environment,
            config.asr_model_dir,
        )?;
        config.asr_threads =
            parse_value("asr_threads", &arguments, environment, config.asr_threads)?;
        config.max_utterance_ms = parse_value(
            "max_utterance_ms",
            &arguments,
            environment,
            config.max_utterance_ms,
        )?;
        config.pseudo_stream = parse_bool_value(
            "pseudo_stream",
            &arguments,
            environment,
            config.pseudo_stream,
        )?;
        config.partial_interval_ms = parse_value(
            "partial_interval_ms",
            &arguments,
            environment,
            config.partial_interval_ms,
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
  --asr-model-dir=<dir>       ReazonSpeech k2-v2 の ONNX を置いたディレクトリ
  --asr-threads=<n>           認識スレッド数 (既定: 2)
  --max-utterance-ms=<ms>     finalize が来ないまま話し続けた場合の上限
                              (既定: 25000)
  --partial-interval-ms=<ms>  途中経過を出す間隔 (既定: 500)
  --pseudo-stream[=<bool>]    途中経過の再デコードを有効にする (既定: false)
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
        "asr_model_dir",
        "asr_threads",
        "max_utterance_ms",
        "pseudo_stream",
        "partial_interval_ms",
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
    use super::Config;
    use std::collections::HashMap;

    #[test]
    fn defaults_keep_design_values() {
        let config = Config::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8770);
        assert_eq!(config.path, "/asr/v1");
        assert_eq!(config.asr_threads, 2);
        assert_eq!(config.max_utterance_ms, 25_000);
        assert_eq!(config.partial_interval_ms, 500);
        assert!(!config.pseudo_stream);
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
