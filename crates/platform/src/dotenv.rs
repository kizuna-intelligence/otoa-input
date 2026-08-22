use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `.env` 形式のファイルを読み、キーと値の対応を返す。
/// プロセスの環境変数は変更しない。
pub fn parse(contents: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();

    for raw_line in contents.lines() {
        let mut line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(stripped) = line.strip_prefix("export ") {
            line = stripped;
        }

        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };

        let key = raw_key.trim();
        if key.is_empty() {
            continue;
        }

        let value = raw_value.trim();
        let (value, quoted) = match value.as_bytes().first().copied() {
            Some(b'"') if value.len() >= 2 && value.ends_with('"') => {
                (&value[1..value.len() - 1], true)
            }
            Some(b'\'') if value.len() >= 2 && value.ends_with('\'') => {
                (&value[1..value.len() - 1], true)
            }
            _ => (value, false),
        };

        let value = if quoted {
            value
        } else {
            value.split_once(" #").map_or(value, |(value, _)| value)
        };
        values.insert(key.to_string(), value.to_string());
    }

    values
}

/// 指定ディレクトリから上へ向かって `.env` を探し、最初に見つかったものを読む。
/// 見つからなければ空の map を返す。読めなければ warn を出して空を返す。
pub fn load_from_ancestors(start: &Path) -> HashMap<String, String> {
    let mut current = absolute_path(start);
    let home = dirs::home_dir().map(|path| canonical_or_original(&path));

    for _ in 0..8 {
        let env_path = current.join(".env");
        match std::fs::read_to_string(&env_path) {
            Ok(contents) => return parse(&contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    path = %env_path.display(),
                    error = %error,
                    "failed to read .env"
                );
                return HashMap::new();
            }
        }

        if home.as_ref().is_some_and(|home| current == *home) {
            break;
        }

        let Some(parent) = current.parent().map(Path::to_path_buf) else {
            break;
        };
        if parent == current {
            break;
        }

        if let Some(home) = &home {
            if current.starts_with(home) && !parent.starts_with(home) {
                break;
            }
        }
        current = parent;
    }

    HashMap::new()
}

fn absolute_path(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current_dir| current_dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    canonical_or_original(&path)
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_simple_pairs() {
        let values = parse("A=1\nB=2");
        assert_eq!(values.len(), 2);
        assert_eq!(values.get("A"), Some(&"1".to_string()));
        assert_eq!(values.get("B"), Some(&"2".to_string()));
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let values = parse("# c\n\nA=1\n  # another comment");
        assert_eq!(values.len(), 1);
        assert_eq!(values.get("A"), Some(&"1".to_string()));
    }

    #[test]
    fn strips_export_prefix() {
        let values = parse("export A=1");
        assert_eq!(values.get("A"), Some(&"1".to_string()));
    }

    #[test]
    fn strips_double_quotes() {
        let values = parse("A=\"x y\"");
        assert_eq!(values.get("A"), Some(&"x y".to_string()));
    }

    #[test]
    fn strips_single_quotes() {
        let values = parse("A='x y'");
        assert_eq!(values.get("A"), Some(&"x y".to_string()));
    }

    #[test]
    fn keeps_hash_inside_quotes() {
        let values = parse("A=\"a#b\"");
        assert_eq!(values.get("A"), Some(&"a#b".to_string()));
    }

    #[test]
    fn strips_trailing_comment_unquoted() {
        let values = parse("A=1 # c");
        assert_eq!(values.get("A"), Some(&"1".to_string()));
    }

    #[test]
    fn ignores_lines_without_equals() {
        let values = parse("justtext");
        assert!(values.is_empty());
    }

    #[test]
    fn later_key_wins() {
        let values = parse("A=first\nA=second");
        assert_eq!(values.get("A"), Some(&"second".to_string()));
    }
}
