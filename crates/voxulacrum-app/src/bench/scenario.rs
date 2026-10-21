//! Benchmark scenarios: what a `--bench` run does, read from a RON file.
//!
//! A scenario is a list of steps run in order. Exactly one of them is a
//! `Measure`, which holds the steps whose frames are recorded; the steps
//! before it set the run up and are not measured. Durations are wall-clock
//! seconds: the camera and the frame clock are presentation, and a benchmark
//! measures frames.

use std::path::Path;

use serde::Deserialize;

use crate::input::GameAction;

/// A benchmark scenario, as read from its file.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Scenario {
    /// Short name the results are filed under, such as `S1`.
    pub name: String,
    /// Id of the package whose world the steps were written against. A run on
    /// another world is refused, since the camera positions mean nothing there.
    pub package: String,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

/// One thing a scenario does. Each step finishes before the next starts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub enum Step {
    /// Move the camera's target to `target`, in world units, and set its zoom
    /// (the orthographic half-height), both at once.
    Camera { target: (f32, f32, f32), zoom: f32 },
    /// Move the camera's target in a straight line from wherever it is to `to`,
    /// at a constant speed, arriving after `seconds`. The zoom stays as it is.
    Pan { to: (f32, f32, f32), seconds: f32 },
    /// Put the mouse cursor at a fraction of the window, `x` from the left and
    /// `y` from the top. It stays there: a benchmark ignores the real mouse.
    Cursor { x: f32, y: f32 },
    /// Press an input action once, as its key would.
    Press(GameAction),
    /// Hold the screen-relative movement intent `direction` (x right, y
    /// forward, at most length 1, as the keys give it) for `seconds`, pressing
    /// the next action of `presses` in turn every `every` seconds. It walks the
    /// player in Play mode and pans the camera in Authoring.
    Move { direction: (f32, f32), seconds: f32, every: f32, presses: Vec<GameAction> },
    /// Wait until the engine is at rest: nothing missing from the streamed set,
    /// and no generation or meshing outstanding. The run fails if that takes
    /// longer than `timeout` seconds.
    Settle { timeout: f32 },
    /// Wait this many seconds.
    Wait(f32),
    /// Record every frame while these steps run.
    Measure(Vec<Step>),
}

impl Scenario {
    /// Parse and validate the text of a scenario file.
    pub fn from_ron_str(text: &str) -> Result<Self, String> {
        let scenario: Self = ron::from_str(text).map_err(|e| e.to_string())?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// The scenario in the file at `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_ron_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Refuse a scenario that cannot run, or whose measured window is not
    /// exactly one top-level `Measure`.
    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name is empty".into());
        }
        if self.package.trim().is_empty() {
            return Err("package is empty".into());
        }
        let measures = self.steps.iter().filter(|s| matches!(s, Step::Measure(_))).count();
        if measures != 1 {
            return Err(format!("a scenario has exactly one Measure step, and this one has {measures}"));
        }
        for (i, step) in self.steps.iter().enumerate() {
            step.validate(&format!("steps[{i}]"), false)?;
        }
        Ok(())
    }
}

impl Step {
    /// Refuse a step that cannot run. `at` names the step in the error, and
    /// `measuring` is true inside a `Measure`, which may not hold another.
    fn validate(&self, at: &str, measuring: bool) -> Result<(), String> {
        match self {
            Step::Camera { target, zoom } => {
                finite(at, "Camera target", *target)?;
                positive(at, "Camera zoom", *zoom)
            }
            Step::Pan { to, seconds } => {
                finite(at, "Pan to", *to)?;
                positive(at, "Pan seconds", *seconds)
            }
            Step::Cursor { x, y } => {
                fraction(at, "Cursor x", *x)?;
                fraction(at, "Cursor y", *y)
            }
            Step::Press(_) => Ok(()),
            Step::Move { direction, seconds, every, .. } => {
                let (x, y) = *direction;
                if !(x.is_finite() && y.is_finite() && x * x + y * y <= 1.0 + 1e-6) {
                    return Err(format!(
                        "{at}: Move direction must be finite and at most length 1, got {direction:?}"
                    ));
                }
                positive(at, "Move seconds", *seconds)?;
                positive(at, "Move every", *every)
            }
            Step::Settle { timeout } => positive(at, "Settle timeout", *timeout),
            Step::Wait(seconds) => positive(at, "Wait", *seconds),
            Step::Measure(steps) => {
                if measuring {
                    return Err(format!("{at}: a Measure cannot hold another Measure"));
                }
                if steps.is_empty() {
                    return Err(format!("{at}: Measure is empty, so it would record nothing"));
                }
                for (i, step) in steps.iter().enumerate() {
                    step.validate(&format!("{at}.Measure[{i}]"), true)?;
                }
                Ok(())
            }
        }
    }
}

fn finite(at: &str, what: &str, point: (f32, f32, f32)) -> Result<(), String> {
    let (x, y, z) = point;
    if x.is_finite() && y.is_finite() && z.is_finite() {
        Ok(())
    } else {
        Err(format!("{at}: {what} must be finite, got {point:?}"))
    }
}

fn fraction(at: &str, what: &str, value: f32) -> Result<(), String> {
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(format!("{at}: {what} must be between 0 and 1, got {value}"))
    }
}

fn positive(at: &str, what: &str, value: f32) -> Result<(), String> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(format!("{at}: {what} must be a positive number, got {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scenario named `S1` for the sandbox with `steps` as its step list.
    fn scenario(steps: &str) -> String {
        format!(r#"( name: "S1", package: "sandbox", steps: [ {steps} ] )"#)
    }

    #[test]
    fn a_rest_scenario_parses() {
        let text = scenario(
            "Camera(target: (64.0, 32.0, 64.0), zoom: 40.0), Settle(timeout: 60.0), Wait(15.0), \
             Measure([ Wait(60.0) ])",
        );
        let parsed = Scenario::from_ron_str(&text).unwrap();
        assert_eq!(parsed.name, "S1");
        assert_eq!(parsed.package, "sandbox");
        assert_eq!(
            parsed.steps,
            vec![
                Step::Camera { target: (64.0, 32.0, 64.0), zoom: 40.0 },
                Step::Settle { timeout: 60.0 },
                Step::Wait(15.0),
                Step::Measure(vec![Step::Wait(60.0)]),
            ],
        );
    }

    #[test]
    fn the_shipped_scenarios_parse() {
        for (text, name) in [
            (include_str!("../../../../bench/s1-rest-launch.ron"), "S1"),
            (include_str!("../../../../bench/s2-rest-r14.ron"), "S2"),
            (include_str!("../../../../bench/s3-pan-r14.ron"), "S3"),
            (include_str!("../../../../bench/s4-play.ron"), "S4"),
        ] {
            let parsed = Scenario::from_ron_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(parsed.name, name);
            assert_eq!(parsed.package, "sandbox");
        }
    }

    #[test]
    fn whole_seconds_may_be_written_without_a_decimal_point() {
        let parsed = Scenario::from_ron_str(&scenario("Measure([ Wait(60) ])")).unwrap();
        assert_eq!(parsed.steps, vec![Step::Measure(vec![Step::Wait(60.0)])]);
    }

    #[test]
    fn a_pan_parses_and_refuses_a_duration_that_is_not_positive() {
        let parsed = Scenario::from_ron_str(&scenario("Measure([ Pan(to: (364.0, 32.0, 64.0), seconds: 25.0) ])"))
            .unwrap();
        assert_eq!(
            parsed.steps,
            vec![Step::Measure(vec![Step::Pan { to: (364.0, 32.0, 64.0), seconds: 25.0 }])],
        );
        let err = Scenario::from_ron_str(&scenario("Measure([ Pan(to: (0.0, 0.0, 0.0), seconds: 0.0) ])"))
            .unwrap_err();
        assert!(err.contains("Pan seconds"), "{err}");
    }

    #[test]
    fn play_steps_parse() {
        let text = scenario(
            "Cursor(x: 0.5, y: 0.5), Measure([ Press(TogglePlayMode), \
             Move(direction: (1.0, 0.0), seconds: 2.0, every: 0.5, presses: [PrimaryAction, CycleTool(1)]) ])",
        );
        let parsed = Scenario::from_ron_str(&text).unwrap();
        assert_eq!(
            parsed.steps,
            vec![
                Step::Cursor { x: 0.5, y: 0.5 },
                Step::Measure(vec![
                    Step::Press(GameAction::TogglePlayMode),
                    Step::Move {
                        direction: (1.0, 0.0),
                        seconds: 2.0,
                        every: 0.5,
                        presses: vec![GameAction::PrimaryAction, GameAction::CycleTool(1)],
                    },
                ]),
            ],
        );
    }

    #[test]
    fn a_cursor_outside_the_window_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Cursor(x: 1.5, y: 0.5), Measure([ Wait(1.0) ])")).unwrap_err();
        assert!(err.contains("Cursor x"), "{err}");
    }

    #[test]
    fn a_move_faster_than_the_keys_is_refused() {
        let text = scenario("Measure([ Move(direction: (1.0, 1.0), seconds: 1.0, every: 0.5, presses: []) ])");
        let err = Scenario::from_ron_str(&text).unwrap_err();
        assert!(err.contains("at most length 1"), "{err}");
    }

    #[test]
    fn a_scenario_without_a_measure_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Wait(15.0)")).unwrap_err();
        assert!(err.contains("has 0"), "{err}");
    }

    #[test]
    fn a_scenario_with_two_measures_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Measure([ Wait(1.0) ]), Measure([ Wait(1.0) ])")).unwrap_err();
        assert!(err.contains("has 2"), "{err}");
    }

    #[test]
    fn a_measure_inside_a_measure_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Measure([ Measure([ Wait(1.0) ]) ])")).unwrap_err();
        assert!(err.contains("steps[0].Measure[0]"), "{err}");
        assert!(err.contains("cannot hold another"), "{err}");
    }

    #[test]
    fn an_empty_measure_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Measure([])")).unwrap_err();
        assert!(err.contains("record nothing"), "{err}");
    }

    #[test]
    fn a_duration_that_is_not_positive_is_refused_with_its_place() {
        let err = Scenario::from_ron_str(&scenario("Wait(15.0), Measure([ Wait(0.0) ])")).unwrap_err();
        assert!(err.contains("steps[1].Measure[0]: Wait"), "{err}");
    }

    #[test]
    fn a_missing_field_is_refused() {
        let err = Scenario::from_ron_str(&scenario("Camera(target: (0.0, 0.0, 0.0)), Measure([ Wait(1.0) ])"))
            .unwrap_err();
        assert!(err.contains("zoom"), "{err}");
    }

    #[test]
    fn a_scenario_for_no_package_is_refused() {
        let text = r#"( name: "S1", package: " ", steps: [ Measure([ Wait(1.0) ]) ] )"#;
        assert_eq!(Scenario::from_ron_str(text).unwrap_err(), "package is empty");
    }
}
