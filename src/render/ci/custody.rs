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
    if config.protocol == crate::config::custody::Protocol::SealedV2 {
        return sealed_file(config);
    }
    let runner = ResolvedRunner::literal("ubuntu-24.04")?;
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    workflow.push_str(
        "name: Retain completed producer custody\n\non:\n  workflow_run:\n    workflows:\n",
    );
    for name in config.producers.keys() {
        workflow.push_str(&format!("      - \"{}\"\n", yaml_double_quote(name)));
    }
    // Completions of the same producer workflow, commit, event, branch, PR and
    // source repository supersede each other: `collect` re-reads every sibling
    // producer run, so only the newest pending completion needs a runner.
    // Without this every completion (PR branches included) queued its own
    // identity job. The producer's workflow id keeps a same-named unenrolled
    // workflow from replacing a pending enrolled completion.
    // ponytail: Actions groups are case-insensitive and see only the first
    // associated PR, so rare distinct targets can share this queue; a replaced
    // run then leaves that target's custody status missing (fails closed) until
    // its next completion, and the job-level digest group stays exact.
    workflow.push_str("    types: [completed]\n\nconcurrency:\n  group: producer-custody-${{ github.event.workflow_run.workflow_id }}-${{ github.event.workflow_run.head_repository.id }}-${{ github.event.workflow_run.head_sha }}-${{ github.event.workflow_run.event }}-${{ github.event.workflow_run.head_branch }}-${{ github.event.workflow_run.pull_requests[0].number }}\n  cancel-in-progress: false\n\npermissions:\n  contents: read\n  actions: read\n  statuses: write\n\njobs:\n  identity:\n    runs-on: ubuntu-24.04\n    timeout-minutes: 2\n    permissions:\n      contents: read\n    outputs:\n      group: ${{ steps.identity.outputs.group }}\n      eligible: ${{ steps.identity.outputs.eligible }}\n    steps:\n      - name: Establish completion target identity\n        id: identity\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: identity\n");
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

fn sealed_file(config: &Config) -> Result<GeneratedFile> {
    let runner = ResolvedRunner::literal("ubuntu-24.04")?;
    let policy = serde_json::to_string(config)?.replace('\'', "''");
    let mut workflow = String::new();
    push_generated_workflow_header(&mut workflow);
    workflow.push_str(
        "name: Retain completed producer custody\n\non:\n  workflow_run:\n    workflows:\n",
    );
    for name in config.producers.keys() {
        workflow.push_str(&format!("      - \"{}\"\n", yaml_double_quote(name)));
    }
    workflow.push_str("    types: [completed]\n\npermissions:\n  contents: read\n\njobs:\n  identity:\n    runs-on: ubuntu-24.04\n    timeout-minutes: 2\n    outputs:\n      group: ${{ steps.identity.outputs.group }}\n      eligible: ${{ steps.identity.outputs.eligible }}\n    steps:\n      - name: Establish completion target identity\n        id: identity\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: identity\n");
    workflow.push_str(&format!(
        "          SIMIT_CUSTODY_CONFIG: '{policy}'\n        run: |\n"
    ));
    push_sealed_program(&mut workflow);
    workflow.push_str("  seal:\n    needs: [identity]\n    if: needs.identity.outputs.eligible == 'true'\n    permissions:\n      contents: read\n      actions: read\n      checks: read\n    concurrency:\n      group: custody-seal-${{ needs.identity.outputs.group }}-${{ github.event.workflow_run.id }}\n      cancel-in-progress: false\n    runs-on: ubuntu-24.04\n    timeout-minutes: 15\n    outputs:\n      artifact_id: ${{ steps.capsule-upload.outputs.artifact-id }}\n      artifact_digest: ${{ steps.capsule-upload.outputs.artifact-digest }}\n      upload_outcome: ${{ steps.capsule-upload.outcome }}\n    env:\n      GH_HOST: github.com\n      GH_TOKEN: ${{ github.token }}\n");
    workflow.push_str(&format!("      SIMIT_CUSTODY_CONFIG: '{policy}'\n    steps:\n      - name: Capture and admit completed original capsule\n        id: capture\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: capture\n          SIMIT_CUSTODY_ROOT: ${{{{ runner.temp }}}}/simit-producer-capsule\n        run: |\n"));
    push_sealed_program(&mut workflow);
    workflow.push_str("      - name: Seal original capsule\n        id: capsule-upload\n        if: always()\n        timeout-minutes: 5\n");
    push_action_uses(
        &mut workflow,
        Platform::Github,
        Some(&runner),
        "upload-artifact",
        "v7.0.2",
        "v4.6.2",
    );
    workflow.push_str("\n        with:\n          name: ${{ steps.capture.outputs.capsule_name || format('custody-diagnostic-{0}-{1}', github.run_id, github.run_attempt) }}\n          retention-days: 32\n          if-no-files-found: error\n          path: ${{ runner.temp }}/simit-producer-capsule\n  aggregate:\n    needs: [identity, seal]\n    if: always() && needs.identity.outputs.eligible == 'true'\n    permissions:\n      contents: read\n      actions: read\n      checks: read\n      statuses: write\n    concurrency:\n      group: custody-${{ needs.identity.outputs.group }}\n      cancel-in-progress: false\n    runs-on: ubuntu-24.04\n    timeout-minutes: 25\n    env:\n      GH_HOST: github.com\n      GH_TOKEN: ${{ github.token }}\n");
    workflow.push_str(&format!("      SIMIT_CUSTODY_CONFIG: '{policy}'\n      SIMIT_CUSTODY_CAPSULE_ID: ${{{{ needs.seal.outputs.artifact_id }}}}\n      SIMIT_CUSTODY_CAPSULE_DIGEST: ${{{{ needs.seal.outputs.artifact_digest }}}}\n      SIMIT_CUSTODY_SEAL_OUTCOME: ${{{{ needs.seal.outputs.upload_outcome }}}}\n    steps:\n      - name: Independently verify and aggregate sealed originals\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: aggregate\n          SIMIT_CUSTODY_ROOT: ${{{{ runner.temp }}}}/simit-producer-custody\n        run: |\n"));
    push_sealed_program(&mut workflow);
    workflow.push_str("      - name: Upload verified capsule aggregate\n        id: custody-upload\n        if: always()\n        timeout-minutes: 5\n");
    push_action_uses(
        &mut workflow,
        Platform::Github,
        Some(&runner),
        "upload-artifact",
        "v7.0.2",
        "v4.6.2",
    );
    workflow.push_str("\n        with:\n          name: producer-custody-v2-${{ github.run_id }}-${{ github.run_attempt }}\n          retention-days: 32\n          if-no-files-found: error\n          path: ${{ runner.temp }}/simit-producer-custody\n      - name: Revalidate sealed custody and report source status\n        if: always()\n        shell: python\n        env:\n          SIMIT_CUSTODY_PHASE: report\n          SIMIT_CUSTODY_ROOT: ${{ runner.temp }}/simit-producer-custody\n          SIMIT_CUSTODY_UPLOAD_OUTCOME: ${{ steps.custody-upload.outcome }}\n          SIMIT_CUSTODY_ARTIFACT_ID: ${{ steps.custody-upload.outputs.artifact-id }}\n          SIMIT_CUSTODY_ARTIFACT_DIGEST: ${{ steps.custody-upload.outputs.artifact-digest }}\n        run: |\n");
    push_sealed_program(&mut workflow);
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

fn push_sealed_program(workflow: &mut String) {
    for line in include_str!("custody.py")
        .lines()
        .take_while(|line| !line.starts_with("if __name__ =="))
        .chain(include_str!("custody_sealed.py").lines())
    {
        if line.is_empty() {
            workflow.push('\n');
        } else {
            workflow.push_str(&format!("          {line}\n"));
        }
    }
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
