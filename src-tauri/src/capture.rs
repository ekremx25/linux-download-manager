use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const EXTENSIONS: &str = "7z zip rar tar gz tgz bz2 xz zst torrent mp4 mkv webm avi mov m4v m3u8 mpd mp3 m4a flac wav ogg opus aac pdf epub csv doc docx xls xlsx ppt pptx odt apk appimage bin deb dmg exe msi pkg rpm iso img";

#[derive(Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Rules {
    enabled: bool,
    extensions: Vec<String>,
    excluded_sites: Vec<String>,
}
impl Default for Rules {
    fn default() -> Self {
        Self {
            enabled: true,
            extensions: EXTENSIONS.split_whitespace().map(str::to_owned).collect(),
            excluded_sites: Vec::new(),
        }
    }
}
impl Rules {
    fn validate(&mut self) -> Result<(), String> {
        if self.extensions.len() > 300 || self.excluded_sites.len() > 200 {
            return Err("Too many capture rules".into());
        }
        for extension in &mut self.extensions {
            *extension = extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if extension.is_empty()
                || extension.len() > 20
                || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return Err("Use extensions such as zip, mp4, pdf".into());
            }
        }
        self.extensions.sort();
        self.extensions.dedup();
        for site in &mut self.excluded_sites {
            *site = site.trim().to_ascii_lowercase();
            if site.is_empty() || site.len() > 2048 || site.chars().any(char::is_whitespace) {
                return Err("Invalid excluded site or address".into());
            }
        }
        Ok(())
    }
}
// Native host reads rules without starting the desktop app for unrelated downloads.
pub fn request(input: &Value) -> Result<Value, String> {
    let defaults = Rules::default();
    let dir = crate::resolve_app_data_dir()?;
    let path = dir.join("downloads.sqlite3");
    if !path.exists() && input.get("settings").is_none() {
        return Ok(
            json!({"ok":true,"configured":false,"settings":defaults,"defaults":defaults.extensions}),
        );
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let db = Connection::open(path).map_err(|e| e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| e.to_string())?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY,value TEXT NOT NULL)",
    )
    .map_err(|e| e.to_string())?;
    if let Some(value) = input.get("settings") {
        let mut rules: Rules = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        rules.validate()?;
        let sql = if input["initializeOnly"].as_bool() == Some(true) {
            "INSERT OR IGNORE INTO app_settings(key,value) VALUES('capture_rules_v1',?1)"
        } else {
            "INSERT OR REPLACE INTO app_settings(key,value) VALUES('capture_rules_v1',?1)"
        };
        db.execute(
            sql,
            params![serde_json::to_string(&rules).map_err(|e| e.to_string())?],
        )
        .map_err(|e| e.to_string())?;
    }
    let saved: Option<String> = db
        .query_row(
            "SELECT value FROM app_settings WHERE key='capture_rules_v1'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let rules: Rules = match &saved {
        Some(value) => serde_json::from_str(value).map_err(|e| e.to_string())?,
        None => Rules::default(),
    };
    Ok(
        json!({"ok":true,"configured":saved.is_some(),"settings":rules,"defaults":defaults.extensions}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editable_types_validate_and_empty_list_stays_empty() {
        let mut rules: Rules = serde_json::from_value(
            json!({"extensions":[".ZIP","zip","CBZ"],"excludedSites":["*.example.org"]}),
        )
        .unwrap();
        rules.validate().unwrap();
        assert_eq!(rules.extensions, vec!["cbz", "zip"]);
        assert!(rules.enabled);
        rules.extensions = Vec::new();
        rules.validate().unwrap();
        assert!(rules.extensions.is_empty());
        rules.extensions = vec!["../zip".into()];
        assert!(rules.validate().is_err());
        assert!(Rules::default().extensions.contains(&"mp4".into()));
    }
}
