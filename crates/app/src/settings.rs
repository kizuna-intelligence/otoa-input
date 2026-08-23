use otoa_input_core::Settings as CoreSettings;
use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::ops::{Deref, DerefMut};

/// 保存される設定。
///
/// `core` はこのアプリが解釈する設定、`product` は接続先の実装だけが解釈する
/// 設定である。公開側は `product` の中身を一切解釈しない。ここに型を持たせると、
/// 接続先を差し替えるたびに公開側の型を変えることになる。
#[derive(Clone, Default)]
pub struct Settings {
    pub core: CoreSettings,
    /// 設定ファイルのうち `core` が使わないキー。
    /// そのまま保存に書き戻し、`ConnectionProvider` へ渡す。
    pub product: Value,
}

impl Deref for Settings {
    type Target = CoreSettings;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl DerefMut for Settings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

impl Settings {
    pub fn product_settings_value(&self) -> Option<Value> {
        self.product.is_object().then(|| self.product.clone())
    }
}

impl Serialize for Settings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let core = serde_json::to_value(&self.core).map_err(serde::ser::Error::custom)?;
        // 製品側のキーをベースにして core で上書きする。逆にすると、
        // 設定画面で変更した core の値が保存時に古い値へ戻る。
        let merged = match (self.product.clone(), core) {
            (Value::Object(mut product), Value::Object(core)) => {
                product.extend(core);
                Value::Object(product)
            }
            (_, core) => core,
        };
        merged.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Ok(Self {
            core: serde_json::from_value(value.clone()).map_err(D::Error::custom)?,
            product: value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn product_keys_survive_a_round_trip() {
        // 公開側が知らないキーを落とすと、製品版の設定が保存のたびに消える。
        let json = r#"{"input_gain":2.0,"product_only_field":"keep me"}"#;
        let settings = serde_json::from_str::<Settings>(json).expect("settings should parse");
        assert_eq!(settings.core.input_gain, 2.0);

        let saved = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(saved["product_only_field"], "keep me");
    }

    #[test]
    fn core_wins_over_the_stored_copy() {
        let json = r#"{"input_gain":2.0,"product_only_field":"keep me"}"#;
        let mut settings = serde_json::from_str::<Settings>(json).expect("settings should parse");
        settings.core.input_gain = 3.0;

        let saved = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(saved["input_gain"], 3.0);
        assert_eq!(saved["product_only_field"], "keep me");
    }

    #[test]
    fn overlay_keys_survive_settings_json_round_trip() {
        let json = r#"{"overlay_position":"top","overlay_transparent":"on","reduce_motion":true}"#;
        let settings = serde_json::from_str::<Settings>(json).expect("settings should parse");
        let saved = serde_json::to_value(&settings).expect("settings should serialize");
        assert_eq!(saved["overlay_position"], "top");
        assert_eq!(saved["overlay_transparent"], "on");
        assert_eq!(saved["reduce_motion"], true);
    }
}
