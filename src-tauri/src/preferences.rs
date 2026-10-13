use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TransferPreferences {
    /// 0 preserves the existing automatic choice.
    pub connections: usize,
    pub max_height: u32,
    pub container: String,
    pub audio_language: Option<String>,
}
impl Default for TransferPreferences {
    fn default() -> Self {
        Self {
            connections: 0,
            max_height: 0,
            container: "mp4".into(),
            audio_language: None,
        }
    }
}
impl TransferPreferences {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(language) = &self.audio_language {
            if language.len() > 35
                || language.is_empty()
                || !language
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            {
                return Err("Invalid audio language".into());
            }
        }
        if self.connections > 16 {
            return Err("Connections must be between 0 (automatic) and 16".into());
        }
        if ![0, 360, 480, 720, 1080, 1440, 2160, 4320].contains(&self.max_height) {
            return Err("Unsupported video resolution".into());
        }
        if !["mp4", "mkv", "webm", "mp3", "m4a"].contains(&self.container.as_str()) {
            return Err("Unsupported output format".into());
        }
        Ok(())
    }
    pub fn ytdlp_args(&self, custom: Option<&str>) -> Vec<String> {
        let mut args = self.ytdlp_format_args(custom);
        if self.connections > 0 {
            args.extend([
                "--concurrent-fragments".into(),
                self.connections.to_string(),
            ]);
        }
        args
    }
    fn ytdlp_format_args(&self, custom: Option<&str>) -> Vec<String> {
        let language = self
            .audio_language
            .as_ref()
            // YouTube reports "und" when the player does not identify a track.
            // It is not a real language to constrain yt-dlp to.
            .filter(|language| language.as_str() != "und")
            .map(|l| format!("[language^={l}]"))
            .unwrap_or_default();
        if matches!(self.container.as_str(), "mp3" | "m4a") {
            return vec![
                "-f".into(),
                format!("ba{language}/b{language}"),
                "-x".into(),
                "--audio-format".into(),
                self.container.clone(),
                "--audio-quality".into(),
                "0".into(),
            ];
        }
        let h = if self.max_height == 0 {
            String::new()
        } else {
            format!("[height<={}]", self.max_height)
        };
        let format = if language.is_empty() {
            custom
                .map(str::to_owned)
                .unwrap_or_else(|| format!("bv*{h}+ba/b{h}"))
        } else {
            // Do not silently fall back to another language or a combined English track.
            let selected_height = custom.map(|f| {
                f.split("[height<=")
                    .nth(1)
                    .and_then(|s| s.split(']').next())
                    .and_then(|s| s.parse::<u32>().ok())
            });
            let height = match selected_height {
                Some(Some(n)) => format!("[height<={n}]"),
                Some(None) => String::new(),
                None => h,
            };
            format!("bv{height}+ba{language}/b{height}{language}")
        };
        vec![
            "-f".into(),
            format,
            "--merge-output-format".into(),
            self.container.clone(),
            "--remux-video".into(),
            self.container.clone(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct FeatureSettings {
    pub transfer: TransferPreferences,
    pub show_detail_on_start: bool,
    pub auto_close_detail: bool,
    pub show_video_quality_menu: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_validate_and_audio_is_extracted() {
        let mut p = TransferPreferences {
            connections: 17,
            ..Default::default()
        };
        assert!(p.validate().is_err());
        p.connections = 8;
        p.container = "mp3".into();
        assert!(p.validate().is_ok());
        let a = p.ytdlp_args(Some("bv*+ba/b"));
        assert!(a.contains(&"-x".into()));
        assert!(a.contains(&"mp3".into()));
        p.container = "mkv".into();
        p.max_height = 720;
        assert!(
            p.ytdlp_args(None)
                .contains(&"bv*[height<=720]+ba/b[height<=720]".into())
        );
    }
    #[test]
    fn media_parallelism_applies_to_audio_and_video() {
        for container in ["mp4", "mp3"] {
            let p = TransferPreferences {
                connections: 10,
                container: container.into(),
                ..Default::default()
            };
            let args = p.ytdlp_args(None);
            assert!(
                args.windows(2)
                    .any(|a| a == ["--concurrent-fragments", "10"])
            );
        }
        assert!(
            !TransferPreferences::default()
                .ytdlp_args(None)
                .contains(&"--concurrent-fragments".into())
        );
    }
    #[test]
    fn selected_audio_language_has_no_other_language_fallback() {
        let mut p = TransferPreferences {
            audio_language: Some("tr".into()),
            max_height: 1080,
            ..Default::default()
        };
        assert_eq!(
            p.ytdlp_args(None)[1],
            "bv[height<=1080]+ba[language^=tr]/b[height<=1080][language^=tr]"
        );
        assert_eq!(
            p.ytdlp_args(Some("bv*[height<=720]+ba/b"))[1],
            "bv[height<=720]+ba[language^=tr]/b[height<=720][language^=tr]"
        );
        p.container = "mp3".into();
        assert_eq!(p.ytdlp_args(None)[1], "ba[language^=tr]/b[language^=tr]");
    }
    #[test]
    fn unspecified_audio_language_uses_normal_format_selection() {
        let p = TransferPreferences {
            audio_language: Some("und".into()),
            ..Default::default()
        };
        assert_eq!(p.ytdlp_args(None)[1], "bv*+ba/b");
    }
    #[test]
    fn legacy_preferences_keep_existing_defaults() {
        let s: FeatureSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.transfer.connections, 0);
        assert_eq!(s.transfer.container, "mp4");
    }
}
