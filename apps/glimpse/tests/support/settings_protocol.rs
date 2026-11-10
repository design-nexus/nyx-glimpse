// Nyx Settings' extension protocol (nyx-settings apps/settings/src/ext/protocol.rs, 6b8b3ea): Glimpse's settings page must deserialize as Settings reads it.
//! What extensions print. Everything is JSON on stdout:
//!
//! - `<exec> pages` → `[PageInfo]` (empty: no matching hardware)
//! - `<exec> describe PAGE` → `PageDesc`
//! - `<exec> set PAGE KEY VALUE` → nothing, or `SetReply` (`toast`, `refresh`, `reload`);
//!   a non-zero exit shows stderr
//! - `<exec> theme-changed` → re-apply anything the new Omarchy theme reset
//!
//! Values are passed to `set` as plain text: `true`/`false` for switches, a number for
//! sliders, an option id for choices, `#rrggbb` for colours, `true,false,…` for chips,
//! `temp:percent,…` (°C) for one series of a curve (the key is `KEY/SERIES`), and
//! nothing for buttons.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PageInfo {
    pub id: String,
    pub title: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keywords: String,
}

fn default_icon() -> String {
    "application-x-addon-symbolic".into()
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PageDesc {
    /// A dim line under the page description (markup).
    pub subtitle: String,
    pub banners: Vec<Banner>,
    /// Ask again every this many seconds while the page is showing (live values).
    pub poll: u32,
    pub groups: Vec<GroupDesc>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Banner {
    pub text: String,
    pub warning: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct GroupDesc {
    pub collapsible: bool,
    pub open_by_default: bool,
    pub title: String,
    pub note: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Row {
    pub kind: Kind,
    pub key: String,
    pub title: String,
    pub desc: String,
    pub keywords: String,
    /// A small pill after the title, e.g. "Restart needed".
    pub tag: String,
    pub tooltip: String,
    /// Ask for the whole page again after this row changes.
    pub refresh: bool,
    /// Disable a control without hiding its current value.
    pub disabled: bool,

    pub value: serde_json::Value,
    /// choice, segmented, buttons: `[id, label]` pairs.
    pub options: Vec<(String, String)>,

    // slider
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub digits: u32,
    pub unit: String,
    /// Values are °C; shown in the user's temperature unit.
    pub temperature: bool,
    /// Send only when the slider is let go, not while it moves.
    pub on_release: bool,
    /// Quick-pick chips under the slider.
    pub marks: Vec<f64>,
    /// The firmware default, offered as a reset button.
    pub reset: Option<f64>,

    // entry
    pub placeholder: String,

    // button
    pub label: String,
    /// Needs a second click; this is the text shown while armed.
    pub confirm: String,
    pub destructive: bool,

    // colour
    pub theme: Option<ThemeSwatch>,
    /// A plain colour button instead of the swatch picker.
    pub compact: bool,

    // chips
    pub labels: Vec<String>,

    // camera: a live preview of this video device (`/dev/video0`)
    pub device: String,
    // meter: a live level graph of this PipeWire source (its node name)
    pub source: String,

    // curve
    pub series: Vec<Series>,
    pub presets: bool,
    pub hint: String,

    /// switch: shown while it's on. disclosure: shown when opened.
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Info,
    Switch,
    Slider,
    Choice,
    Segmented,
    Buttons,
    Entry,
    Button,
    Colour,
    Chips,
    Curve,
    Disclosure,
    Camera,
    Meter,
    /// A kind newer than this Settings; shown as a note instead of failing the page.
    #[serde(other)]
    Unknown,
}

/// Every kind this Settings can draw, passed to helpers as `SETTINGS_KINDS`.
pub const KINDS: &str = "info switch slider choice segmented buttons entry button colour chips curve disclosure camera meter";

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ThemeSwatch {
    pub label: String,
    pub colour: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Series {
    pub id: String,
    pub label: String,
    /// (°C, percent)
    pub points: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SetReply {
    pub toast: String,
    pub refresh: bool,
    /// Ask every extension for its pages again (devices appeared or went away).
    pub reload: bool,
}

impl Row {
    pub fn bool_value(&self) -> bool {
        self.value.as_bool().unwrap_or(false)
    }

    pub fn f64_value(&self) -> f64 {
        self.value.as_f64().or_else(|| self.value.as_str().and_then(|s| s.parse().ok())).unwrap_or(self.min)
    }

    pub fn str_value(&self) -> String {
        match &self.value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            v => v.to_string(),
        }
    }

    pub fn bools(&self) -> Vec<bool> {
        let given: Vec<bool> =
            self.value.as_array().map(|a| a.iter().map(|v| v.as_bool().unwrap_or(false)).collect()).unwrap_or_default();
        (0..self.labels.len()).map(|i| given.get(i).copied().unwrap_or(false)).collect()
    }
}

/// `true,false,…` for chips.
pub fn encode_bools(v: &[bool]) -> String {
    v.iter().map(|b| b.to_string()).collect::<Vec<_>>().join(",")
}

/// `temp:percent,…` for one curve series.
pub fn encode_points(p: &[(u32, u32)]) -> String {
    p.iter().map(|(t, s)| format!("{t}:{s}")).collect::<Vec<_>>().join(",")
}

pub fn parse_pages(text: &str) -> anyhow::Result<Vec<PageInfo>> {
    Ok(serde_json::from_str(text.trim())?)
}

pub fn parse_page(text: &str) -> anyhow::Result<PageDesc> {
    Ok(serde_json::from_str(text.trim())?)
}

/// Empty output is a plain success.
pub fn parse_reply(text: &str) -> SetReply {
    serde_json::from_str(text.trim()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_fill_in_defaults() {
        let p = parse_pages(r#"[{"id":"headset","title":"Headset"}]"#).unwrap();
        assert_eq!(p[0].icon, "application-x-addon-symbolic");
        assert!(p[0].description.is_empty());
        assert!(parse_pages("[]").unwrap().is_empty());
        assert!(parse_pages("nope").is_err());
    }

    #[test]
    fn describes_rows() {
        let d = parse_page(
            r#"{"subtitle":"<b>G16</b>","poll":5,"groups":[{"title":"Battery","rows":[
                {"kind":"slider","key":"limit","title":"Charge limit","value":80,"min":20,"max":100,"step":5,"unit":"%","marks":[60,80,100],"on_release":true},
                {"kind":"switch","key":"custom","title":"Custom curves","value":true,"rows":[
                    {"kind":"curve","key":"fan","series":[{"id":"cpu","label":"CPU fan","points":[[30,10],[60,50]]}],"presets":true}
                ]},
                {"kind":"choice","key":"mode","value":"b","options":[["a","A"],["b","B"]],"refresh":true},
                {"kind":"chips","key":"when","labels":["Boot","Sleep"],"value":[true]},
                {"kind":"slider","key":"tgp","temperature":true,"value":"87","min":75,"max":90,"reset":87}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!((d.poll, d.subtitle.as_str()), (5, "<b>G16</b>"));
        let rows = &d.groups[0].rows;
        assert_eq!(rows[0].kind, Kind::Slider);
        assert_eq!(rows[0].f64_value(), 80.0);
        assert_eq!(rows[0].marks, [60.0, 80.0, 100.0]);
        assert!(rows[0].on_release);
        assert!(rows[1].bool_value());
        assert_eq!(rows[1].rows[0].series[0].points, [(30, 10), (60, 50)]);
        assert_eq!(rows[2].options[1], ("b".to_string(), "B".to_string()));
        assert_eq!(rows[2].str_value(), "b");
        assert!(rows[2].refresh);
        // Missing chip states are off.
        assert_eq!(rows[3].bools(), [true, false]);
        // Numbers given as text still read.
        assert_eq!(rows[4].f64_value(), 87.0);
        assert_eq!(rows[4].reset, Some(87.0));
    }

    #[test]
    fn unknown_kinds_still_parse() {
        let d = parse_page(r#"{"groups":[{"rows":[{"kind":"hologram","title":"Hologram"},{"kind":"camera","device":"/dev/video0"},{"kind":"meter","source":"alsa_input.x"}]}]}"#).unwrap();
        let rows = &d.groups[0].rows;
        assert_eq!(rows[0].kind, Kind::Unknown);
        assert_eq!((rows[1].kind, rows[1].device.as_str()), (Kind::Camera, "/dev/video0"));
        assert_eq!((rows[2].kind, rows[2].source.as_str()), (Kind::Meter, "alsa_input.x"));
    }

    #[test]
    fn kinds_list_names_every_kind() {
        for k in ["camera", "meter", "curve", "info"] {
            assert!(KINDS.split(' ').any(|x| x == k));
        }
    }

    #[test]
    fn replies() {
        assert_eq!(parse_reply(""), SetReply::default());
        let r = parse_reply(r#"{"toast":"Restart to apply","refresh":true}"#);
        assert!(r.refresh && r.toast == "Restart to apply" && !r.reload);
        assert!(parse_reply(r#"{"reload":true}"#).reload);
    }

    #[test]
    fn encodes_values() {
        assert_eq!(encode_bools(&[true, false, true]), "true,false,true");
        assert_eq!(encode_points(&[(30, 10), (60, 55)]), "30:10,60:55");
    }
}
