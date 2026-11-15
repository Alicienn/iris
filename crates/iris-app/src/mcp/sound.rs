//! Sound: what plays, and playing or stopping it — through `crate::sound`, the same
//! state as the panel (never around it).

use super::Ctx;
use iris_mcp::{Args, Tool, ToolError, ToolResult};
use serde_json::{json, Value};

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

pub(super) fn tools() -> Vec<Tool> {
    vec![
        Tool::read(
            "sound_status",
            "Sound now",
            "What Iris's Sound plays: whether it plays, the theme (focus, deep, mask, relax, break, \
             sleep, asmr), the sound, the volume and the pad (0–100), the minutes left of a session, \
             and every theme with its sounds.",
            obj(json!({}), &[]),
        ),
        Tool::write(
            "sound_play",
            "Play sound",
            "Plays a generative soundscape in Iris, changing first what is given. theme: focus \
             (everyday work), deep (reading, writing; Silence is one of its sounds), mask (cover the \
             voices around), relax, break (a few minutes, then a bell), sleep (stops after a while), \
             asmr. sound: a sound's name within the theme (see sound_status). volume, x (fluid → \
             structured), y (easy → intense): 0–100. minutes: a session's length, for relax, break, \
             sleep and asmr (0 = no end for sleep and asmr).",
            obj(
                json!({
                    "theme": {"type": "string", "enum": ["focus", "deep", "mask", "relax", "break", "sleep", "asmr"]},
                    "sound": {"type": "string"},
                    "volume": {"type": "integer", "minimum": 0, "maximum": 100},
                    "x": {"type": "integer", "minimum": 0, "maximum": 100},
                    "y": {"type": "integer", "minimum": 0, "maximum": 100},
                    "minutes": {"type": "integer", "minimum": 0}
                }),
                &[],
            ),
        ),
        Tool::write("sound_stop", "Stop sound", "Pauses Iris's Sound (it fades out).", obj(json!({}), &[])),
    ]
}

pub(super) fn call(_ctx: &Ctx, name: &str, a: &Args) -> ToolResult {
    match name {
        "sound_status" => Ok(status()),
        "sound_play" => play(a),
        "sound_stop" => {
            crate::sound::stop();
            Ok(status())
        }
        _ => Err(ToolError::new(format!("unknown tool: {name}"))),
    }
}

fn status() -> Value {
    let s = crate::sound::status();
    let themes: Vec<Value> = crate::sound::THEMES
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "sounds": t.sounds.iter().map(|i| crate::sound::SOUNDS[*i as usize].0).collect::<Vec<_>>(),
                "minutes": crate::sound::session_choices(t.id),
            })
        })
        .collect();
    json!({
        "playing": s.playing,
        "theme": s.theme,
        "sound": s.sound,
        "volume": s.volume,
        "x": s.x,
        "y": s.y,
        "minutes_left": s.minutes_left,
        "problem": s.error,
        "themes": themes,
    })
}

fn percent(a: &Args, key: &str) -> Result<Option<u32>, ToolError> {
    match a.opt_i64(key)? {
        None => Ok(None),
        Some(v) if (0..=100).contains(&v) => Ok(Some(v as u32)),
        Some(v) => Err(ToolError::new(format!("{key} is 0 to 100, not {v}"))),
    }
}

fn play(a: &Args) -> ToolResult {
    let theme = a.opt_str("theme")?;
    let sound = a.opt_str("sound")?;
    let minutes = match a.opt_i64("minutes")? {
        Some(m) if m < 0 => return Err(ToolError::new("minutes cannot be negative")),
        m => m.map(|m| m as u32),
    };
    crate::sound::play(crate::sound::Play {
        theme: theme.as_deref(),
        sound: sound.as_deref(),
        volume: percent(a, "volume")?,
        x: percent(a, "x")?,
        y: percent(a, "y")?,
        minutes,
    })
    .map_err(ToolError::new)?;
    Ok(status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_answered_without_playing() {
        let (ctx, _dossier, _runtime) = super::super::test_ctx();
        // sound_play would open the computer's sound output: only its refusals are
        // tried here.
        for t in tools().iter().filter(|t| t.name != "sound_play") {
            assert!(
                call(&ctx, t.name, &Args::default()).is_ok(),
                "{} failed",
                t.name
            );
        }
        let wrong = call(&ctx, "sound_play", &Args::new(json!({"theme": "disco"})));
        assert!(wrong.is_err(), "an unknown theme is refused");
        let loud = call(&ctx, "sound_play", &Args::new(json!({"volume": 140})));
        assert!(loud.is_err(), "a volume above 100 is refused");
        let status = call(&ctx, "sound_status", &Args::default()).unwrap();
        assert_eq!(status["themes"].as_array().map(|t| t.len()), Some(7));
        assert_eq!(status["playing"], json!(false));
    }
}
