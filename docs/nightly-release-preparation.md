# Windows, Linux and macOS release-ref nightly: supervised manual go-live

**Enabled for supervised manual runs, not yet qualified.** The producer (`nightly.yml`, main) and the
signer (`nightly-sign.yml`, release) keep only their exact context guards and run when one of the reviewed
manual actors dispatches them. There is no schedule yet. The installed policy
[`.github/nightly-signing-policy.json`](../.github/nightly-signing-policy.json) is the v3 policy for
SoundCraft. Its `producer_workflow_id` is currently `null` until `nightly.yml` is registered on `main`;
once merged to `main`, read the workflow ID and populate `producer_workflow_id` before promoting the
policy to `release`:

```bash
gh api repos/storytold/soundcraft/actions/workflows/nightly.yml --jq .id
```

The callers pass no platform secrets. The unsigned reusable caller's maximum OIDC/attestation
permissions are for its isolated attester; its compiler narrows to contents and Actions read
only, with no OIDC, attestation write, release environment or platform signing secrets.
Shared tooling is pinned to `605d58766e8f536564da8fc81d3676ac86816dea`, the `craft-common-actions`
main commit that merged [#339](https://github.com/storytold/craft-common-actions/pull/339) (bounded
lineage compares and an issue reread at the end of candidate admission) on top of
[#342](https://github.com/storytold/craft-common-actions/pull/342) (policy v3, the `platforms` input
and the Linux and macOS legs). Both callers name it in `uses` and `tooling-revision`. A pin is reviewed
source availability, not provider, queue, native signing or crypto qualification.
Existing stable workflows are unchanged.

**First runs.** The first runs are manual dispatches in this order: `nightly.yml` on `main` (only a
listed manual actor passes the producer's plan step), then `nightly-sign.yml` on `release` with that
producer run ID and attempt, by a listed manual actor.

**Manual actors.** `bflatastic`, `dylanmtaylor`, `echelon`, `FredApps` and `skylartaylor`, in the
producer's `manual-actors` and the policy's `manual_actors`. The list mirrors the `release-managers`
team (id `19837241`), leaving out `echelon-robot`; update it by PR in both places when the team changes.

**Admission timing.** The candidate check reads the newest CI run on the candidate commit and
`main`'s latest finished Sonar analysis, so when to dispatch matters:
- The candidate must be `main`'s latest finished Sonar analysis. Pick a quiet `main`, wait for its
  CI and SonarCloud analysis to finish green, then dispatch the producer and, once it succeeds,
  the signer, before another `main` push lands a newer analysis.

**Platform rows.** The producer caller lists
`platforms: '["windows-x86_64","linux-x86_64","macos-aarch64"]'` on its single job: one producer
run builds every listed platform for one all-or-nothing release (one failed or
skipped platform stops that night's release, Windows included), and the release-ref signer admits
platforms only from the protected policy, so there is no separate Linux or macOS caller job and
the signer caller has no platform input.

## Installed policy values and their sources

Read on 2026-11-24; re-read anything that may have moved (and populate `producer_workflow_id` after `nightly.yml` lands on `main`) before the policy is promoted to `release`.

| Field | Value | Source |
|---|---|---|
| `repository_id` | `1408486424` | `gh api repos/storytold/soundcraft --jq .id` |
| `producer_workflow_id` | `null` (pending registration) | Read via `gh api repos/storytold/soundcraft/actions/workflows/nightly.yml --jq .id` once `nightly.yml` lands on `main` |
| `producer_tooling_revision`, `signer_tooling_revision`, `producer_workflow_ref` | `605d5876…` (`craft-common-actions` #339) | The callers' pin |
| `manual_actors` / `automatic_signer_actors` | the five release managers / `[]` | `release-managers` team (id `19837241`); equals the producer caller's `manual-actors` |
| `max_age_seconds`, `max_artifact_bytes` | `86400`, `2147483648` | Go-live checklist |
| `platforms` | three rows | Storytold stable signing identities (`windows-x86_64`, `linux-x86_64`, `macos-aarch64`) |
| `candidate_policy.required_workflows` | CI (`ci.yml`, workflow `380135771`), `gate / gate (ubuntu-24.04)` | Workflow list API and recent `main` push runs |
| `candidate_policy.sonar` | `storytold` / `storytold_soundcraft`, app `12526` | `sonar-project.properties`; the SonarCloud check on `main` commits |
| Sonar conditions | All six quality-gate conditions required (`accepted_conditions: []`) | Quality gate of `storytold_soundcraft` `main` (all OK) |
| Sonar `issue_types` / `debt` | `VULNERABILITY`; `[]` (0 unresolved vulnerabilities on `main`) | `api/issues/search` on `main` |

## Ownership and interface

- `.github/workflows/nightly.yml`: default-main registration of the unsigned producer, named
  `SoundCraft unsigned nightly`, enabled for manual dispatch by `manual-actors`; no signing secrets
  may be passed or inherited.
- `.github/workflows/nightly-sign.yml`: top-level release registration, enabled for manual dispatch,
  and identical on `main` and `release` after reviewed promotion. It rejects wrong repository/event/actual branch,
  including same-name tags. Its constant whole-run concurrency is distinct from stable,
  cancellation is off and `queue: max` must be hosted-qualified. The caller itself passes no
  credentials; only the shared callee's sign jobs bind the `release` environment.
- Shared reusable entrypoints are `nightly-unsigned.yml` and `nightly-release.yml` in
  `storytold/craft-common-actions`. Both must resolve to a **real reviewed public immutable
  commit**; the defining workflow SHA and tooling input must match.

**Every Linux Nightly format starts the app the flavor's way.** The `.deb`/`.rpm` wrapper, the
AppImage's `AppRun`, the Arch launcher, the Flatpak wrapper, the launcher inside the plain
`nightly.tar.gz` and the FHS tree in `package.sh`'s `.tar.gz` all exec the real GUI with the literal
`--launch-profile nightly-v1 --profile-root <${XDG_CONFIG_HOME:-~/.config}/soundcraft-nightly> --`,
and pass a Launcher protocol or a `--version`/`-V` query through untouched, so `soundcraft --version`,
the Launcher's version query and the smoke checks still answer without a profile. The profile holds
`ui.json`, `Autosave/`, `Presets/` and `logs/`; it is outside stable's settings folder and outside
the unpacked or installed copy, which the flavor's fence would otherwise refuse. The Nightly tarballs
therefore ship `soundcraft` as the launcher next to `soundcraft-bin`, and the Nightly Flatpak — the
one sandbox whose own app id (`ai.storyteller.soundcraft.nightly`) the fence accepts, because
everything inside it is already that package's own — starts the GUI from `/app/bin/soundcraft-bin`.

## Local regression checks

Run `bash packaging/nightly-checks.sh` for everything that needs no compiler: the synthetic suites
below, the tarball and Arch launcher tests, the FHS tar's launcher split, and `bash -n` over every
Nightly shell script. Run `python3 packaging/nightly-release-tests.py` to verify:
- Both callers pin the same immutable 40-hex `craft-common-actions` commit in `uses` and `tooling-revision`.
- Both callers keep their exact context guards, have no registered schedule, and maintain the no-cache/no-platform-secret boundary.
- `nightly-sign.yml` preserves its literal root `concurrency` serialization (`soundcraft-nightly-release`, `cancel-in-progress: false`, `queue: max`).
- `.github/nightly-signing-policy.json` matches the callers, repository identity, CI workflow, SonarCloud project properties, and reviewed release managers list.
