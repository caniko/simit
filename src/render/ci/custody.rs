//! Trusted default-branch observer; producer sources and artifacts remain data.
use super::*;
use crate::config::custody::Config;

pub const PATH: &str = ".github/workflows/producer-custody.yaml";
// GitHub documents a 500 KB workflow-file limit. Use decimal bytes, including
// every embedded program, repeated policy, indentation and YAML/JSON escape.
const MAX_WORKFLOW_BYTES: usize = 500_000;

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
    workflow.push_str("    types: [completed]\n\npermissions:\n  contents: read\n  actions: read\n  statuses: write\n\njobs:\n  identity:\n    runs-on: ubuntu-24.04\n    timeout-minutes: 2\n    permissions:\n      contents: read\n    outputs:\n      group: ${{ steps.identity.outputs.group }}\n      eligible: ${{ steps.identity.outputs.eligible }}\n    steps:\n      - name: Establish completion target identity\n        id: identity\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: identity\n");
    workflow.push_str(&format!(
        "          SIMIT_CUSTODY_CONFIG: '{}'\n        run: |\n",
        serde_json::to_string(config)?.replace('\'', "''")
    ));
    push_program(&mut workflow);
    workflow.push_str("  collect:\n    needs: [identity]\n    if: needs.identity.outputs.eligible == 'true'\n    permissions:\n      contents: read\n      actions: read\n      checks: read\n      statuses: write\n    concurrency:\n      group: custody-${{ needs.identity.outputs.group }}\n      cancel-in-progress: false\n    runs-on: ubuntu-24.04\n    timeout-minutes: 25\n    env:\n      GH_HOST: github.com\n      GH_TOKEN: ${{ github.token }}\n");
    workflow.push_str(&format!(
        "      SIMIT_CUSTODY_CONFIG: '{}'\n    steps:\n",
        serde_json::to_string(config)?.replace('\'', "''")
    ));
    for phase in ["capture", "report"] {
        if phase == "report" {
            workflow.push_str("      - name: Upload completed-producer custody\n        id: custody-upload\n        if: always()\n        timeout-minutes: 5\n");
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
        workflow.push_str(&format!("      - name: {name}\n        if: always()\n        shell: python\n        env:\n          SIMIT_CUSTODY_ROOT: ${{{{ runner.temp }}}}/simit-producer-custody\n          SIMIT_CUSTODY_PHASE: {phase}\n"));
        if phase == "report" {
            workflow.push_str("          SIMIT_CUSTODY_UPLOAD_OUTCOME: ${{ steps.custody-upload.outcome }}\n          SIMIT_CUSTODY_ARTIFACT_ID: ${{ steps.custody-upload.outputs.artifact-id }}\n");
        }
        workflow.push_str("        run: |\n");
        push_program(&mut workflow);
    }
    if workflow.len() > MAX_WORKFLOW_BYTES {
        bail!(
            "[ci.custody] renders {} bytes, exceeding the {MAX_WORKFLOW_BYTES}-byte workflow limit; shorten enrolled names/inventories",
            workflow.len()
        );
    }
    Ok(GeneratedFile {
        relative_path: PathBuf::from(PATH),
        content: workflow,
    })
}

fn push_program(workflow: &mut String) {
    for line in include_str!("custody.py").lines() {
        if line.is_empty() {
            workflow.push('\n');
        } else {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
}
