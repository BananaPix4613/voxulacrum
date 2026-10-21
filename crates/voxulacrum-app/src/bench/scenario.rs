//! Benchmark scenarios: what a `--bench` run does, read from a RON file.
//!
//! A scenario is a list of steps run in order. Exactly one of them is a
//! `Measure`, which holds the steps whose frames are recorded; the steps
//! before it set the run up and are not measured. Durations are wall-clock
//! seconds: the camera and the frame clock are presentation, and a benchmark
//! measures frames.

use std::path::Path;

use serde::Deserialize;

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
                let (x, y, z) = *target;
                if !(x.is_finite() && y.is_finite() && z.is_finite()) {
                    return Err(format!("{at}: Camera target must be finite, got {target:?}"));
                }
                positive(at, "Camera zoom", *zoom)
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
    fn whole_seconds_may_be_written_without_a_decimal_point() {
        let parsed = Scenario::from_ron_str(&scenario("Measure([ Wait(60) ])")).unwrap();
        assert_eq!(parsed.steps, vec![Step::Measure(vec![Step::Wait(60.0)])]);
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
