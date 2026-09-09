//! Reading `Custom.*.binds`.
//!
//! The file is XML: one element per action, each with `<Primary>` and
//! `<Secondary>` children carrying `Device` and `Key`, and optional
//! `<Modifier>` children. Only `Device="Keyboard"` entries are usable from
//! software; a HOTAS binding on the primary with a keyboard chord on the
//! secondary is the common case and the one this handles.

use crate::keys::{scan_code, ScanCode};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One keyboard chord: the key plus its modifiers, as binds-file names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chord {
    pub key: String,
    pub modifiers: Vec<String>,
    /// Whether it came from the Primary or Secondary slot.
    pub slot: &'static str,
}

impl Chord {
    /// Scan codes for the modifiers then the key. `None` if any name is
    /// unknown -- an action we cannot press is reported, never approximated.
    pub fn scan_codes(&self) -> Option<(Vec<ScanCode>, ScanCode)> {
        let mods = self.modifiers.iter().map(|m| scan_code(m)).collect::<Option<Vec<_>>>()?;
        Some((mods, scan_code(&self.key)?))
    }

    pub fn human(&self) -> String {
        let mut parts: Vec<String> = self.modifiers.iter().map(|m| pretty(m)).collect();
        parts.push(pretty(&self.key));
        parts.join("+")
    }
}

fn pretty(k: &str) -> String {
    k.strip_prefix("Key_").unwrap_or(k).replace("Left", "L").replace("Right", "R").replace("Numpad_", "Num")
}

/// A binding on a device other than the keyboard (HOTAS, gamepad): the
/// game's device id and key name (`Joy_9`, `Joy_POV1Up`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceBind {
    pub device: String,
    pub key: String,
    pub slot: &'static str,
}

impl DeviceBind {
    /// Button number for a plain `Joy_N` binding; hats and axes are not buttons.
    pub fn button(&self) -> Option<u32> {
        self.key.strip_prefix("Joy_")?.parse().ok()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Binds {
    pub path: PathBuf,
    pub preset: String,
    pub layout: String,
    /// Action name -> keyboard chords (primary first if it is a keyboard bind).
    pub actions: HashMap<String, Vec<Chord>>,
    /// Actions that exist in the file with no keyboard binding at all.
    pub unbound: Vec<String>,
    /// Action name -> bindings on other devices (a HOTAS button, a hat).
    pub device_binds: HashMap<String, Vec<DeviceBind>>,
}

impl Binds {
    /// The game's bindings folder.
    pub fn default_dir() -> Option<PathBuf> {
        std::env::var_os("LOCALAPPDATA")
            .map(|l| PathBuf::from(l).join("Frontier Developments").join("Elite Dangerous").join("Options").join("Bindings"))
    }

    /// The most recently written `*.binds` in the folder.
    pub fn find_latest(dir: &Path) -> Option<PathBuf> {
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "binds") {
                let t = e.metadata().and_then(|m| m.modified()).ok()?;
                if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
                    best = Some((t, p));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut b = Self::parse(&text)?;
        b.path = path.to_path_buf();
        Ok(b)
    }

    pub fn parse(xml: &str) -> Result<Self> {
        let doc = roxmltree::Document::parse(xml).context("parsing binds XML")?;
        let root = doc.root_element();
        let preset = root.attribute("PresetName").unwrap_or("").to_string();
        let mut layout = String::new();
        let mut actions: HashMap<String, Vec<Chord>> = HashMap::new();
        let mut unbound = Vec::new();
        let mut device_binds: HashMap<String, Vec<DeviceBind>> = HashMap::new();

        for node in root.children().filter(|n| n.is_element()) {
            let name = node.tag_name().name();
            if name == "KeyboardLayout" {
                layout = node.text().unwrap_or("").trim().to_string();
                continue;
            }
            let slots: Vec<_> = node
                .children()
                .filter(|c| c.is_element() && (c.tag_name().name() == "Primary" || c.tag_name().name() == "Secondary"))
                .collect();
            if slots.is_empty() {
                continue; // a setting (MouseSensitivity etc.), not an action
            }
            let mut chords = Vec::new();
            for s in slots {
                if s.attribute("Device") != Some("Keyboard") {
                    let device = s.attribute("Device").unwrap_or("");
                    let key = s.attribute("Key").unwrap_or("");
                    if !device.is_empty() && device != "{NoDevice}" && !key.is_empty() {
                        device_binds.entry(name.to_string()).or_default().push(DeviceBind { device: device.to_string(), key: key.to_string(), slot: if s.tag_name().name() == "Primary" { "primary" } else { "secondary" } });
                    }
                    continue;
                }
                let key = s.attribute("Key").unwrap_or("").to_string();
                if key.is_empty() {
                    continue;
                }
                let modifiers = s
                    .children()
                    .filter(|m| m.is_element() && m.tag_name().name() == "Modifier" && m.attribute("Device") == Some("Keyboard"))
                    .filter_map(|m| m.attribute("Key").map(str::to_string))
                    .collect();
                chords.push(Chord {
                    key,
                    modifiers,
                    slot: if s.tag_name().name() == "Primary" { "primary" } else { "secondary" },
                });
            }
            if chords.is_empty() {
                unbound.push(name.to_string());
            } else {
                actions.insert(name.to_string(), chords);
            }
        }
        Ok(Binds { path: PathBuf::new(), preset, layout, actions, unbound, device_binds })
    }

    /// The first keyboard chord for an action.
    pub fn chord(&self, action: &str) -> Option<&Chord> {
        self.actions.get(action).and_then(|v| v.first())
    }

    /// Every non-keyboard binding of an action.
    pub fn device_binds(&self, action: &str) -> &[DeviceBind] {
        self.device_binds.get(action).map(Vec::as_slice).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<Root PresetName="Custom" MajorVersion="4" MinorVersion="2">
	<KeyboardLayout>en-US</KeyboardLayout>
	<MouseSensitivity Value="1.00000000" />
	<IncreaseSystemsPower>
		<Primary Device="231D3201" Key="Joy_9" />
		<Secondary Device="Keyboard" Key="Key_5">
			<Modifier Device="Keyboard" Key="Key_LeftControl" />
		</Secondary>
	</IncreaseSystemsPower>
	<GalaxyMapOpen>
		<Primary Device="231D3201" Key="Joy_13" />
		<Secondary Device="Keyboard" Key="Key_H">
			<Modifier Device="Keyboard" Key="Key_LeftShift" />
		</Secondary>
	</GalaxyMapOpen>
	<UI_Select>
		<Primary Device="Keyboard" Key="Key_Space" />
		<Secondary Device="{NoDevice}" Key="" />
	</UI_Select>
	<OnlyOnStick>
		<Primary Device="231D3201" Key="Joy_1" />
		<Secondary Device="{NoDevice}" Key="" />
	</OnlyOnStick>
</Root>"#;

    #[test]
    fn keyboard_chords_are_found_on_either_slot_and_stick_only_actions_are_unbound() {
        let b = Binds::parse(SAMPLE).unwrap();
        assert_eq!(b.layout, "en-US");
        let pips = b.chord("IncreaseSystemsPower").unwrap();
        assert_eq!((pips.key.as_str(), pips.slot), ("Key_5", "secondary"));
        assert_eq!(pips.modifiers, vec!["Key_LeftControl"]);
        assert_eq!(pips.human(), "LControl+5");
        assert_eq!(b.chord("UI_Select").unwrap().slot, "primary");
        assert!(b.chord("OnlyOnStick").is_none());
        assert!(b.unbound.contains(&"OnlyOnStick".to_string()));
        let (mods, key) = b.chord("GalaxyMapOpen").unwrap().scan_codes().unwrap();
        assert_eq!(mods.len(), 1);
        assert_eq!(key.code, 0x23);
    }

    #[test]
    fn device_bindings_are_kept_with_their_button_numbers() {
        let b = Binds::parse(SAMPLE).unwrap();
        let d = b.device_binds("IncreaseSystemsPower");
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].device.as_str(), d[0].key.as_str(), d[0].button()), ("231D3201", "Joy_9", Some(9)));
        assert_eq!(b.device_binds("OnlyOnStick")[0].button(), Some(1));
        assert!(b.device_binds("UI_Select").is_empty(), "{{NoDevice}} is not a binding");
    }

    #[test]
    fn the_commanders_real_file_parses_when_present() {
        let Some(dir) = Binds::default_dir() else { return };
        let Some(path) = Binds::find_latest(&dir) else { return };
        let b = Binds::load(&path).unwrap();
        assert!(b.actions.len() > 100, "{} actions", b.actions.len());
        for a in ["GalaxyMapOpen", "IncreaseSystemsPower", "UI_Up", "UI_Select"] {
            assert!(b.chord(a).is_some(), "{a} should have a keyboard chord");
            assert!(b.chord(a).unwrap().scan_codes().is_some(), "{a} chord must be pressable");
        }
    }
}
