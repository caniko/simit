//! Trusted default-branch observer; producer sources and artifacts remain data.
use super::*;
use crate::config::custody::Config;

pub const PATH: &str = ".github/workflows/producer-custody.yaml";

pub fn validate_backend(
    config: &crate::config::CiConfig,
    provider: CiProvider,
    platform: Platform,
) -> Result<()> {
    if config.custody.is_some() && (provider != CiProvider::Actions || platform != Platform::Github)
    {
        bail!("[ci.custody] requires the effective GitHub Actions backend");
    }
    Ok(())
}

pub fn file(config: &Config) -> Result<GeneratedFile> {
    config.validate()?;
    let runner = ResolvedRunner::literal("ubuntu-24.04")?;
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    workflow.push_str(
        "name: Retain completed producer custody\n\non:\n  workflow_run:\n    workflows:\n",
    );
    for name in config.producers.keys() {
        workflow.push_str(&format!("      - \"{}\"\n", yaml_double_quote(name)));
    }
    workflow.push_str("    types: [completed]\n\npermissions:\n  contents: read\n  actions: read\n  statuses: write\n\nconcurrency:\n  group: custody-${{ github.event.workflow_run.head_repository.full_name }}-${{ github.event.workflow_run.head_sha }}-${{ github.event.workflow_run.event }}\n  cancel-in-progress: false\n\njobs:\n  collect:\n    runs-on: ubuntu-24.04\n    timeout-minutes: 10\n    env:\n      GH_HOST: github.com\n      GH_TOKEN: ${{ github.token }}\n      SIMIT_CUSTODY_ROOT: ${{ runner.temp }}/simit-producer-custody\n");
    workflow.push_str(&format!(
        "      SIMIT_CUSTODY_CONFIG: \"{}\"\n    steps:\n",
        yaml_double_quote(&serde_json::to_string(config)?)
    ));
    for phase in ["capture", "report"] {
        if phase == "report" {
            workflow.push_str("      - name: Upload completed-producer custody\n        id: custody-upload\n        if: always()\n");
            push_action_uses(
                &mut workflow,
                Platform::Github,
                Some(&runner),
                "upload-artifact",
                "v7.0.2",
                "v4.6.2",
            );
            workflow.push_str("\n        with:\n          name: producer-custody-${{ github.event.workflow_run.head_sha }}-${{ github.event.workflow_run.event }}-${{ github.run_id }}-${{ github.run_attempt }}\n          retention-days: 32\n          if-no-files-found: error\n          path: ${{ runner.temp }}/simit-producer-custody\n");
        }
        let name = if phase == "capture" {
            "Capture completed original producers"
        } else {
            "Report custody on producer source head"
        };
        workflow.push_str(&format!("      - name: {name}\n        if: always()\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: {phase}\n"));
        if phase == "report" {
            workflow.push_str("          SIMIT_CUSTODY_UPLOAD_OUTCOME: ${{ steps.custody-upload.outcome }}\n          SIMIT_CUSTODY_ARTIFACT_ID: ${{ steps.custody-upload.outputs.artifact-id }}\n");
        }
        workflow.push_str("        run: |\n");
        for line in include_str!("custody.py").lines() {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
    Ok(GeneratedFile {
        relative_path: PathBuf::from(PATH),
        content: workflow,
    })
}
