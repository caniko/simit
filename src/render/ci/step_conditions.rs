//! Conditional steps the generator writes, declared once. The renderer emits
//! these values and the updater's GitHub adapter proves the push outcome of
//! the same values, so the two cannot drift apart as separate string lists.

use super::github_action_ref;

/// What a generated conditional step does on an original push run whose job
/// succeeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// It runs, so it must succeed.
    Runs,
    /// Its condition is false, so it must be skipped.
    Skipped,
    /// It runs on the repository's default branch and is skipped elsewhere.
    DefaultBranch,
}

/// An `if:` expression of a generated step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepCondition {
    Always,
    NotCancelled,
    Release,
    ReleaseOutsidePullRequests,
    DefaultBranchPush,
    DefaultBranchPushOrRelease,
    DependencyDownloadFailed,
}

impl StepCondition {
    const ALL: [Self; 7] = [
        Self::Always,
        Self::NotCancelled,
        Self::Release,
        Self::ReleaseOutsidePullRequests,
        Self::DefaultBranchPush,
        Self::DefaultBranchPushOrRelease,
        Self::DependencyDownloadFailed,
    ];

    /// The bare expression, as GitHub evaluates it.
    pub const fn expression(self) -> &'static str {
        match self {
            Self::Always => "always()",
            Self::NotCancelled => "!cancelled()",
            Self::Release => "inputs.release",
            Self::ReleaseOutsidePullRequests => {
                "inputs.release && github.event_name != 'pull_request'"
            }
            Self::DefaultBranchPush => {
                "github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)"
            }
            Self::DefaultBranchPushOrRelease => {
                "(github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)) || (inputs.release && github.event_name != 'pull_request')"
            }
            Self::DependencyDownloadFailed => {
                "failure() && steps.simit-dependency-download.outcome == 'failure'"
            }
        }
    }

    /// The expression in `${{ }}`, the spelling most generated steps use.
    pub fn wrapped(self) -> String {
        format!("${{{{ {} }}}}", self.expression())
    }

    /// `if:` wraps a bare expression in `${{ }}` itself, so both spellings
    /// are one condition.
    pub fn parse(condition: &str) -> Option<Self> {
        let expression = condition
            .strip_prefix("${{")
            .and_then(|inner| inner.strip_suffix("}}"))
            .map_or(condition, str::trim);
        Self::ALL
            .into_iter()
            .find(|known| known.expression() == expression)
    }

    /// Inputs are absent on a push, and a job that succeeded was neither
    /// cancelled nor failed.
    pub const fn push_outcome(self) -> PushOutcome {
        match self {
            Self::Always | Self::NotCancelled => PushOutcome::Runs,
            Self::Release | Self::ReleaseOutsidePullRequests | Self::DependencyDownloadFailed => {
                PushOutcome::Skipped
            }
            Self::DefaultBranchPush | Self::DefaultBranchPushOrRelease => {
                PushOutcome::DefaultBranch
            }
        }
    }

    /// Whether the step runs in every job that can succeed, whatever the
    /// event or branch: it then behaves like a step without a condition.
    pub const fn always_runs(self) -> bool {
        matches!(self, Self::Always | Self::NotCancelled)
    }
}

/// The pinned `actions/cache` sub-action (`restore` or `save`).
pub(crate) fn cache_action(action: &str) -> String {
    github_action_ref("actions/cache", "v6.1.0").replacen(
        "actions/cache@",
        &format!("actions/cache/{action}@"),
        1,
    )
}

/// Whether a step is optional: it may run or be skipped without hiding
/// qualification work. Only the generated cache save qualifies, because that
/// action can do nothing but store a cache entry, and its condition depends
/// on what earlier steps found at run time.
pub fn is_optional_step(step: &serde_yaml::Value) -> bool {
    // The rendered reference ends in a `# version` comment, which YAML drops.
    let save = cache_action("save");
    let pinned = save.split_once(" #").map_or(save.as_str(), |(pin, _)| pin);
    step.get("run").is_none()
        && step.get("uses").and_then(serde_yaml::Value::as_str) == Some(pinned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_parse_in_both_spellings_and_nothing_else() {
        for condition in StepCondition::ALL {
            assert_eq!(
                StepCondition::parse(condition.expression()),
                Some(condition)
            );
            assert_eq!(StepCondition::parse(&condition.wrapped()), Some(condition));
        }
        assert_eq!(StepCondition::parse("${{ vars.SKIP != '' }}"), None);
        assert_eq!(StepCondition::parse("always() && false"), None);
    }

    #[test]
    fn only_the_pinned_cache_save_is_optional() {
        let step = |source: &str| serde_yaml::from_str::<serde_yaml::Value>(source).unwrap();
        let save = cache_action("save");
        assert!(is_optional_step(&step(&format!(
            "if: ${{{{ steps.x.outputs.save == 'true' }}}}\nuses: {save}\n"
        ))));
        assert!(!is_optional_step(&step(&format!(
            "uses: {}\n",
            cache_action("restore")
        ))));
        assert!(!is_optional_step(&step("uses: actions/cache/save@v4\n")));
        assert!(!is_optional_step(&step("run: cargo test\n")));
    }
}
