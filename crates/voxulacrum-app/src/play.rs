//! Play settings: what a package's Play mode is, read from its `play.ron`.
//!
//! The player controller and reach targeting are engine mechanisms; the numbers
//! they run on are the game's, and come from the package the world starts from.
//! A package that ships no `play.ron` has no Play mode.

use std::path::Path;

use bevy_ecs::prelude::Resource;
use serde::Deserialize;

/// File name of a package's play settings, at the package root.
pub const PLAY_FILE: &str = "play.ron";

/// The player's body, movement and reach, in voxels and seconds.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct PlayerTuning {
    /// Half the body's width and depth; the collision box is square.
    pub half_width: f32,
    /// Body height, feet to top.
    pub height: f32,
    /// Feet to eye: where reach and the enclosed-space check measure from.
    pub eye_height: f32,
    /// Downward acceleration.
    pub gravity: f32,
    /// Horizontal speed at full input.
    pub move_speed: f32,
    /// Fastest the player falls.
    pub max_fall_speed: f32,
    /// Tallest rise auto-step climbs.
    pub step_height: f32,
    /// Farthest a voxel may be from the eye for the player to break or place it.
    pub reach: f32,
}

impl PlayerTuning {
    /// Refuse values the controller cannot run on.
    fn validate(&self) -> Result<(), String> {
        let positive = [
            ("half_width", self.half_width),
            ("height", self.height),
            ("gravity", self.gravity),
            ("move_speed", self.move_speed),
            ("max_fall_speed", self.max_fall_speed),
            ("reach", self.reach),
        ];
        for (field, value) in positive {
            if !(value.is_finite() && value > 0.0) {
                return Err(format!("player.{field} must be a positive number, got {value}"));
            }
        }
        for (field, value) in [("eye_height", self.eye_height), ("step_height", self.step_height)] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(format!("player.{field} must be zero or more, got {value}"));
            }
        }
        Ok(())
    }
}

/// A package's play settings, held as a resource while it runs.
#[derive(Clone, Debug, PartialEq, Deserialize, Resource)]
pub struct PlaySettings {
    /// The player's body, movement and reach.
    pub player: PlayerTuning,
}

impl PlaySettings {
    /// Parse and validate the text of a `play.ron`.
    pub fn from_ron_str(text: &str) -> Result<Self, String> {
        let settings: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        settings.player.validate()?;
        Ok(settings)
    }

    /// The play settings of the package at `root`, or `None` if it ships no
    /// `play.ron`.
    pub fn load(root: &Path) -> Result<Option<Self>, String> {
        let path = root.join(PLAY_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_player(body: &str) -> String {
        format!("( player: ( {body} ) )")
    }

    const SANDBOX_PLAYER: &str = "half_width: 0.3, height: 1.5, eye_height: 1.6, gravity: 24.0, \
        move_speed: 4.5, max_fall_speed: 50.0, step_height: 1.0, reach: 4.5";

    #[test]
    fn the_shipped_play_settings_parse() {
        let sandbox = PlaySettings::from_ron_str(include_str!("../../../packages/sandbox/play.ron")).unwrap();
        assert_eq!(sandbox.player.reach, 4.5);
        assert_eq!(sandbox.player.eye_height, 1.6);
        PlaySettings::from_ron_str(include_str!("../../../crates/testbed/content/play.ron")).unwrap();
    }

    #[test]
    fn a_missing_field_is_refused() {
        let text = with_player("half_width: 0.3, height: 1.5");
        assert!(PlaySettings::from_ron_str(&text).is_err());
    }

    #[test]
    fn a_value_the_controller_cannot_run_on_is_refused() {
        let text = with_player(&SANDBOX_PLAYER.replace("move_speed: 4.5", "move_speed: -1.0"));
        let err = PlaySettings::from_ron_str(&text).unwrap_err();
        assert!(err.contains("player.move_speed"), "{err}");
    }

    #[test]
    fn a_package_without_play_settings_has_none() {
        let dir = std::env::temp_dir().join(format!("voxulacrum-no-play-{}", std::process::id()));
        assert_eq!(PlaySettings::load(&dir), Ok(None));
    }
}
