# Windows, Linux and macOS release-ref nightly: supervised manual go-live

**Enabled for supervised manual runs, not yet qualified.** The producer (`nightly.yml`, main) and the
signer (`nightly-sign.yml`, on the dedicated `nightly-signer` branch) keep only their exact context
guards and run when one of the reviewed manual actors dispatches them. There is no schedule yet. The
installed policy [`.github/nightly-signing-policy.json`](../.github/nightly-signing-policy.json) is the
v3 policy for SoundCraft; its `producer_workflow_id` is the registration ID of `nightly.yml` on `main`
(`429730684`), read with:

```bash
gh api repos/storytold/soundcraft/actions/workflows/nightly.yml --jq .id
```

The callers pass no platform secrets. The unsigned reusable caller's maximum OIDC/attestation
permissions are for its isolated attester; its compiler narrows to contents and Actions read
only, with no OIDC, attestation write, release environment or platform signing secrets.
Shared tooling is pinned to `be00e5ccd18b68e950c8a08532f2230e7c3d0cba`, the craft-common-actions
main commit that merged [#371](https://github.com/storytold/craft-common-actions/pull/371) (the
candidate is admitted on the built commit's own Sonar analysis, not main's latest) after
[#370](https://github.com/storytold/craft-common-actions/pull/370) (the Windows and macOS nightly
registry rows for the 14 apps) and
[#369](https://github.com/storytold/craft-common-actions/pull/369) (shared helper checkouts kept out
of the app's clean-source check, and macOS on `macos-15`), following
[#364](https://github.com/storytold/craft-common-actions/pull/364) (the signer may run on the
policy's signer branch) and [#363](https://github.com/storytold/craft-common-actions/pull/363) (the
multi-app pipeline, which makes the signer's `app-name` input required and admits any reviewed
registry app), with #342 (`a4f6fd51`, policy v3 and the `platforms` input) and #339 (`605d5876`)
earlier. Both callers name it in `uses` and `tooling-revision`. A pin is reviewed source
availability, not provider, queue, native signing or crypto qualification.
Existing stable workflows are unchanged.

**First runs.** The first runs are manual dispatches in this order: `nightly.yml` on `main` (only a
listed manual actor passes the producer's plan step), then `nightly-sign.yml` on `nightly-signer` with
that producer run ID and attempt, by a listed manual actor.

**Signer branch.** The signer runs on `refs/heads/nightly-signer`, not on the stable `release` branch,
so nightly signer files never need a stable release sync. The policy's `release_branch` names it and
the signer's guard requires it; this app has no `nightly-dispatch.yml` at all, so nothing transports
a run automatically. The branch only fast-forwards to reviewed main commits, through the settings
script's `advance` (no queued signer run, a trust-closure scan, then the full diff of `.github/` and
`packaging/`), and the existing `release` environment must admit `nightly-signer` alongside
`release` before the first dispatch (a repository settings change, not a workflow one). Its trust
closure is all of `.github/` and `packaging/`, including `packaging/env.sh`, which `import-cert.sh`
sources with the Apple keys. On that branch only the pinned `nightly-sign.yml` caller may reach an
environment: no app workflow binds one and nothing else calls `nightly-release.yml`.
`packaging/nightly-release-tests.py` checks that in-repo part on every main PR. Stable releases
still come only from `refs/heads/release`.

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

Read on 2026-11-24; re-read anything that may have moved before `nightly-signer` is advanced to it.

| Field | Value | Source |
|---|---|---|
| `repository_id` | `1408486424` | `gh api repos/storytold/soundcraft --jq .id` |
| `producer_workflow_id` | `429730684` | `gh api repos/storytold/soundcraft/actions/workflows/nightly.yml --jq .id` (path `.github/workflows/nightly.yml`, state `active`) |
| `producer_tooling_revision`, `signer_tooling_revision`, `producer_workflow_ref` | `be00e5cc…` (`craft-common-actions` exact-analysis admission, #371 after #370 and #369, following #364) | The callers' pin |
| `release_branch` | `nightly-signer` | The shared validator allows `release` or `nightly-signer`; the signer guard names the same ref |
| `manual_actors` / `automatic_signer_actors` | the five release managers / `[]` | `release-managers` team (id `19837241`); equals the producer caller's `manual-actors` |
| `max_age_seconds`, `max_artifact_bytes` | `86400`, `2147483648` | Go-live checklist |
| `platforms` | three rows | Storytold stable signing identities (`windows-x86_64`, `linux-x86_64`, `macos-aarch64`) |
| `candidate_policy.required_workflows` | CI (`ci.yml`, workflow `380135771`), `gate / gate (ubuntu-24.04)` | Workflow list API and recent `main` push runs |
| `candidate_policy.sonar` | `storytold` / `storytold_soundcraft`, app `12526` | `sonar-project.properties`; the SonarCloud check on `main` commits |
| Sonar conditions | `new_security_rating` accepted (GT 1, maximum 5); the other five required | Quality gate of `storytold_soundcraft` `main`: the same six conditions, and a failing rating is admitted up to the reviewed A..E bound |
| Sonar `issue_types` / `debt` | `VULNERABILITY`; `[]` (0 unresolved vulnerabilities on `main`) | `api/issues/search` on `main` |

## Ownership and interface

- `.github/workflows/nightly.yml`: default-main registration of the unsigned producer, named
  `SoundCraft unsigned nightly`, enabled for manual dispatch by `manual-actors`; no signing secrets
  may be passed or inherited.
- `.github/workflows/nightly-sign.yml`: top-level signer registration, enabled for manual dispatch on
  `nightly-signer`, which carries a reviewed main commit. It rejects wrong repository/event/actual branch,
  including same-name tags. Its constant whole-run concurrency is distinct from stable,
  cancellation is off. The caller itself passes no credentials and names the registry `app-name`;
  only the shared callee's sign jobs bind the `release` environment.
- Shared reusable entrypoints are `nightly-unsigned.yml` and `nightly-release.yml` in
  `storytold/craft-common-actions`. Both must resolve to a **real reviewed public immutable
  commit**; the defining workflow SHA and tooling input must match.

**Every Linux Nightly format starts the app the flavor's way.** The `.deb`/`.rpm` wrapper, the
AppImage's `AppRun`, the Arch launcher and the Flatpak wrapper all exec the real GUI with the literal
`--launch-profile nightly-v1 --profile-root <${XDG_CONFIG_HOME:-~/.config}/soundcraft-nightly> --`,
and pass a Launcher protocol or a `--version`/`-V` query through untouched, so `soundcraft --version`,
the Launcher's version query and the smoke checks still answer without a profile. The plain
`nightly.tar.gz` (and `package.sh`'s tree tarball, which installs the same file) is the exception: its
`soundcraft` is craft-common-actions' shared launcher template
(#363, rendered for this app), which execs `soundcraft-bin` beside it with
`--profile-root <${XDG_DATA_HOME:-~/.local/share}/ai.storyteller.soundcraft.nightly> --`, forwards a
first-argument `--launch-profile` pair unchanged, and asks a bare `--version`/`-V` in the Launcher's
own form. The profile holds
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
