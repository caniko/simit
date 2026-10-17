use std::{fs, path::Path, process::Command};

pub fn script(root: &Path, workflow: &serde_yaml::Value, job: &str, script: &str) -> String {
    // Execute the generated job-budget step, then emulate Actions' output
    // expansion. The caller and native runner retain their production code.
    let output_file = tempfile::NamedTempFile::new().unwrap();
    let budget = workflow["jobs"][job]["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "simit-gate-budget")
        .unwrap();
    let output = Command::new("bash")
        .current_dir(root)
        .args(["-euo", "pipefail", "-c", budget["run"].as_str().unwrap()])
        .env("GITHUB_OUTPUT", output_file.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let deadline = fs::read_to_string(output_file.path()).unwrap();
    let deadline = deadline.trim().strip_prefix("deadline=").unwrap();
    assert!(deadline.parse::<u64>().is_ok());
    script.replace("${{ steps.simit-gate-budget.outputs.deadline }}", deadline)
}
