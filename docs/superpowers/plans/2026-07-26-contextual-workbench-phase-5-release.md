# Contextual Workbench Phase 5 Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Phase 5 release claims, versioning, packaging, checksums, installers, CI, and final product-truth gates mechanically truthful and release-ready.

**Architecture:** Add release validation as first-class project behavior instead of relying on manual checks. Tests drive decisions about live-log claims, MSRV truth, version consistency, archive reproducibility, checksum verification, installer behavior, package contents, release artifact naming, and product-claim coverage.

**Tech Stack:** Rust/Cargo, GitHub Actions YAML, Bash, PowerShell 7 parser-compatible scripts, POSIX shell installer tests, local fixture HTTP servers, SHA-256 checksum manifests, Cargo package smoke tests, and existing Ratatui/in-app help surfaces.

---

## File Structure

Phase 5 intentionally touches only the current release, installer, source, docs, and workflow files named by the spec. Do not introduce broad framework rewrites.

- `Cargo.toml`: authoritative crate version, `rust-version`, release profile assumptions, dev test dependencies only if already acceptable under MSRV.
- `Cargo.lock`: regenerated only when dependency or Rust version choices require it.
- `README.md`: install examples, version references, MSRV statement, live-log/product claims, checksum verification instructions, artifact names.
- `.github/workflows/ci.yml`: fmt, clippy, tests, MSRV, release build, installer syntax/parser, package smoke, checksum gate, product-claim gate.
- `.github/workflows/release.yml`: release build matrix, deterministic archive creation, manifest generation, upload of archives and checksum manifest.
- `scripts/install.sh`: Unix installer with injectable download base and mandatory checksum verification before extraction/install.
- `scripts/install.ps1`: Windows installer with injectable download base and mandatory checksum verification before extraction/install.
- `src/api/ws.rs`: inspect only to confirm `spawn_log_follow` remains transport-only and is not wired by this phase.
- `src/app/command.rs`: Phase 2 single owner for canonical command labels, descriptions, keybinding help metadata, and availability truth.
- `src/ui/components/help.rs`: generated contextual help surfaces after Phase 3.
- `src/ui/components/palette.rs`: command palette render/use-site truth and description integration after Phase 3.
- `src/ui/tabs/logs.rs`: log tab text and tests proving claims say on-demand refresh and do not promise continuous tailing.
- `images/contextual-workbench-wide.svg`, `images/contextual-workbench-medium.svg`, `images/contextual-workbench-compact.svg`: Phase 3 screenshot truth references checked against README.
- `images/stdb*.jpg`: legacy screenshots remain reviewed when present so stale visible claims do not survive.
- `CHANGELOG.md`: modify the existing file created in Phase 1. Preserve earlier Phase 1-4 entries and add the 0.1.0/Phase 5 release notes and product-claim checklist without overwriting prior sections.
- `scripts/release/validate-version.sh`: version and tag consistency validation.
- `scripts/release/make-archives.sh`: reproducible archive and manifest generation.
- `scripts/release/verify-checksums.sh`: checksum manifest verifier used by CI and installers.
- `scripts/release/product-claim-check.sh`: grep-backed checklist for README, in-app help, keybinding help, palette descriptions, screenshots, and release notes.
- `tests/release_package_smoke.rs`: fatal package binary smoke tests and package archive smoke.
- `tests/release_installers.rs`: local fixture HTTP/injectable download-base tests for `install.sh` and `install.ps1`.
- `tests/release_version_truth.rs`: Rust tests for versions, artifact names, README examples, and CHANGELOG consistency.

## Dependency-Ordered Tasks

### Task 1: Remove the unsupported continuous log-tail claim

The current repository defines `src/api/ws.rs::spawn_log_follow` but the application has no runtime call site. This phase takes the approved product-truth option of keeping on-demand log refresh and removing the unsupported continuous-tail promise. Connecting log follow remains a separate future feature that must receive its own scoped ownership and resource design.

**Files:**
- Create: `tests/product_truth.rs`
- Modify: `README.md`
- Modify: `src/ui/tabs/logs.rs`
- Modify: `src/ui/components/help.rs`
- Modify: `src/app/command.rs`
- Modify: `src/ui/components/palette.rs`
- Inspect only: `src/api/ws.rs`

- [ ] **Step 1: Confirm the transport helper still has no runtime owner**

Run:
```bash
rg -n "spawn_log_follow" src
```
Expected: one definition in `src/api/ws.rs` and no call from `src/app.rs`, `src/effects/`, or UI code. If a prior phase added a real scoped owner, stop this task and replace it with direct ownership and bounded-buffer tests for that implementation before changing claims.

- [ ] **Step 2: Write the failing product-truth test**

Create `tests/product_truth.rs`:

```rust
const README: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"));
const LOGS_VIEW: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/tabs/logs.rs"));
const HELP: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/components/help.rs"));
const COMMANDS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/command.rs"));
const PALETTE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui/components/palette.rs"));

#[test]
fn log_surfaces_describe_on_demand_refresh_not_continuous_tail() {
    let surfaces = [README, LOGS_VIEW, HELP, COMMANDS, PALETTE];
    let forbidden_positive_claims = [
        "tail structured logs",
        "continuous live logs",
        "live log tailing is enabled",
        "start live log tailing",
        "follow logs live",
    ];

    for text in surfaces {
        let lowercase = text.to_ascii_lowercase();
        for claim in forbidden_positive_claims {
            assert!(
                !lowercase.contains(claim),
                "unsupported positive log claim remains: {claim}"
            );
        }
    }

    assert!(README.contains("Logs can be refreshed on demand. Continuous log tailing is not enabled in this release."));
    assert!(LOGS_VIEW.contains("Logs can be refreshed on demand. Continuous log tailing is not enabled in this release."));

    let refresh_description = "Refresh the active resource on demand. Log views refresh on demand and are not continuously tailed in this release.";
    assert!(COMMANDS.contains(refresh_description), "canonical command text must live in src/app/command.rs");
    assert!(!HELP.contains(refresh_description), "help source must render command metadata, not duplicate command text");
    assert!(!PALETTE.contains(refresh_description), "palette source must render command metadata, not duplicate command text");
    assert!(HELP.contains("CommandRegistry"), "help use-site must consume command registry metadata");
    assert!(PALETTE.contains("CommandRegistry"), "palette use-site must consume command registry metadata");
    assert!(PALETTE.contains("registry.by_id") && PALETTE.contains("spec.description"), "palette render/use-site must resolve CommandSpec descriptions from the registry");
}
```

- [ ] **Step 3: Run the test and observe the current claim fail**

Run:
```bash
cargo test --test product_truth log_surfaces_describe_on_demand_refresh_not_continuous_tail
```
Expected: FAIL because README currently says `Tail structured logs` and the required on-demand wording is absent.

- [ ] **Step 4: Replace claims with exact truthful wording**

Use this README and logs-view sentence:

```text
Logs can be refreshed on demand. Continuous log tailing is not enabled in this release.
```

Use this canonical command description in `src/app/command.rs`; `src/ui/components/help.rs` and `src/ui/components/palette.rs` must render or consume that command metadata rather than duplicating this exact text in UI source. The palette implementation should contain mechanical registry-use fragments such as `CommandRegistry`, `registry.by_id`, and `spec.description`:

```text
Refresh the active resource on demand. Log views refresh on demand and are not continuously tailed in this release.
```

Do not call `spawn_log_follow`, add a detached task, or claim a connection state that the app does not own.

- [ ] **Step 5: Run the focused and full product-truth tests**

Run:
```bash
cargo test --test product_truth
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tests/product_truth.rs README.md src/ui/tabs/logs.rs src/ui/components/help.rs src/app/command.rs src/ui/components/palette.rs
git commit -m "docs: make log refresh claims truthful"
```

### Task 2: Establish truthful Rust version/MSRV and CI job

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `README.md`
- Modify: `.github/workflows/ci.yml`
- Create: `scripts/release/check-msrv.sh`

- [ ] **Step 1: Write the failing MSRV validation script**

Create `scripts/release/check-msrv.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."

manifest_msrv="$(awk -F ' = ' '$1 == "rust-version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)"
if [ -z "$manifest_msrv" ]; then
  echo "Cargo.toml is missing package.rust-version" >&2
  exit 1
fi

if ! grep -Eq "Rust ${manifest_msrv}|MSRV[[:space:]:-]+${manifest_msrv}|rust-version[[:space:]]*=[[:space:]]*\"${manifest_msrv}\"" README.md; then
  echo "README.md does not state the Cargo.toml MSRV ${manifest_msrv}" >&2
  exit 1
fi

if ! grep -q "toolchain: ${manifest_msrv}" .github/workflows/ci.yml; then
  echo ".github/workflows/ci.yml does not run an MSRV job for ${manifest_msrv}" >&2
  exit 1
fi

echo "MSRV ${manifest_msrv} is declared in Cargo.toml, README.md, and CI"
```

- [ ] **Step 2: Run validation to verify it fails**

Run:
```bash
bash scripts/release/check-msrv.sh
```
Expected: FAIL with `Cargo.toml is missing package.rust-version`, or `README.md does not state`, or `ci.yml does not run an MSRV job`.

- [ ] **Step 3: Determine actual MSRV by command**

Run:
```bash
for v in 1.78.0 1.79.0 1.80.0 1.81.0 1.82.0 1.83.0 1.84.0 1.85.0 1.86.0 1.87.0 1.88.0 1.89.0 1.90.0 1.91.0 1.92.0 1.93.0; do
  echo "checking Rust $v"
  if rustup toolchain install "$v" --profile minimal >/dev/null 2>&1 && cargo +"$v" check --locked --all-targets; then
    echo "MSRV_CANDIDATE=$v"
    found_msrv="$v"
    break
  fi
done
if [ -z "${found_msrv:-}" ]; then
  echo "no MSRV candidate from 1.78.0 through 1.93.0 passed cargo check --locked --all-targets" >&2
  exit 1
fi
```
Expected: prints the first `MSRV_CANDIDATE=x.y.z` that passes `cargo check --locked --all-targets`. If 1.78.0 fails because dependencies require newer Rust, keep the first passing version and update all claims to that value.

- [ ] **Step 4: Implement MSRV truth**

Set `Cargo.toml` package metadata to the discovered version. Example if the first passing candidate is `1.82.0`:

```toml
[package]
version = "0.1.0"
rust-version = "1.82.0"
```

Add this README text under installation or development requirements:

```markdown
### Rust version

This release is tested with Rust stable and has a minimum supported Rust version (MSRV) of Rust 1.82.0. CI builds the crate with that exact toolchain and with stable.
```

Add this CI job entry in `.github/workflows/ci.yml`:

```yaml
  msrv:
    name: MSRV
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.82.0
      - uses: Swatinem/rust-cache@v2
      - run: cargo check --locked --all-targets
```

Replace `1.82.0` in the snippets above with the actual `MSRV_CANDIDATE` from Step 3.

- [ ] **Step 5: Run validation and MSRV build**

Run:
```bash
bash scripts/release/check-msrv.sh
cargo +$(awk -F ' = ' '$1 == "rust-version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml) check --locked --all-targets
```
Expected: PASS and prints `MSRV <version> is declared in Cargo.toml, README.md, and CI`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock README.md .github/workflows/ci.yml scripts/release/check-msrv.sh
git commit -m "ci: establish tested msrv"
```

### Task 3: Validate version consistency across Cargo.toml, tag, and README examples

**Files:**
- Modify: `Cargo.toml`
- Modify: `README.md`
- Modify: `.github/workflows/ci.yml`
- Create: `scripts/release/validate-version.sh`
- Test: `tests/release_version_truth.rs`

- [ ] **Step 1: Write failing version validation script**

Create `scripts/release/validate-version.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

version="$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)"
if [ -z "$version" ]; then
  echo "Cargo.toml package.version not found" >&2
  exit 1
fi

expected_tag="v${version}"
release_tag="${RELEASE_TAG:-}"
if [ "${GITHUB_REF_TYPE:-}" = "tag" ]; then
  release_tag="${GITHUB_REF_NAME:-}"
fi
if [ -n "$release_tag" ] && [ "$release_tag" != "$expected_tag" ]; then
  echo "tag ${release_tag} does not match Cargo.toml version ${expected_tag}" >&2
  exit 1
fi
if [ -n "${GITHUB_REF_NAME:-}" ] && [ "${GITHUB_REF_TYPE:-}" != "tag" ] && [ "${GITHUB_REF_NAME}" != "$expected_tag" ]; then
  echo "ignoring non-tag GITHUB_REF_NAME=${GITHUB_REF_NAME}; release version remains ${expected_tag}"
fi

if ! grep -q "${expected_tag}" README.md; then
  echo "README.md does not mention release tag ${expected_tag}" >&2
  exit 1
fi

if grep -R "spacetimedb-tui-v[0-9]" -n README.md .github/workflows scripts 2>/dev/null | grep -v "${expected_tag}"; then
  echo "found artifact examples that do not match ${expected_tag}" >&2
  exit 1
fi

echo "version ${version} is consistent with ${expected_tag}"
```

Create `tests/release_version_truth.rs`:

```rust
#[test]
fn readme_examples_use_cargo_version_tag() {
    let cargo = std::fs::read_to_string("Cargo.toml").expect("Cargo.toml");
    let version = cargo
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("package version");
    let tag = format!("v{version}");
    let readme = std::fs::read_to_string("README.md").expect("README.md");
    assert!(readme.contains(&tag), "README examples must mention {tag}");
    assert!(!readme.contains("v0.0.0"), "README must not contain dummy version examples");
}

#[test]
fn readme_artifact_examples_use_current_cargo_version_and_existing_shipped_targets_only() {
    let cargo = std::fs::read_to_string("Cargo.toml").expect("Cargo.toml");
    let version = cargo
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
        .expect("package version");
    let tag = format!("v{version}");
    let readme = std::fs::read_to_string("README.md").expect("README.md");
    for target in [
        "x86_64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        let suffix = if target.ends_with("windows-msvc") { "zip" } else { "tar.gz" };
        let artifact = format!("spacetimedb-tui-{tag}-{target}.{suffix}");
        assert!(readme.contains(&artifact), "README missing current shipped artifact example {artifact}");
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:
```bash
bash scripts/release/validate-version.sh
cargo test --test release_version_truth
```
Expected: FAIL because README examples are missing the Cargo-derived tag or list artifacts that do not match the currently shipped target support.

- [ ] **Step 3: Implement version truth in docs and CI**

Add README install example using baseline version `v0.1.0`:

```markdown
Download archives from GitHub Releases using names of the form:

- `spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
- `spacetimedb-tui-v0.1.0-x86_64-apple-darwin.tar.gz`
- `spacetimedb-tui-v0.1.0-aarch64-apple-darwin.tar.gz`
- `spacetimedb-tui-v0.1.0-x86_64-pc-windows-msvc.zip`

Integrity verification and Linux aarch64 release support are added later in the release hardening tasks.
```

Add CI step:

```yaml
      - name: Validate version truth
        run: bash scripts/release/validate-version.sh
```

- [ ] **Step 4: Run tests to verify pass**

Run:
```bash
bash scripts/release/validate-version.sh
cargo test --test release_version_truth
```
Expected: PASS and `version 0.1.0 is consistent with v0.1.0`; release workflow and expanded platform matrix assertions are deferred to later tasks.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml README.md .github/workflows/ci.yml scripts/release/validate-version.sh tests/release_version_truth.rs
git commit -m "test: enforce release version truth"
```

### Task 4: Generate reproducible archive manifest and SHA-256 checksums

**Files:**
- Create: `scripts/release/make-archives.sh`
- Create: `scripts/release/write-checksum-manifest.sh`
- Create: `scripts/release/verify-checksums.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`
- Test: `tests/release_version_truth.rs`

- [ ] **Step 1: Write failing archive scripts and workflow ownership test**

Create `scripts/release/verify-checksums.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
manifest="${1:?usage: verify-checksums.sh SHA256SUMS.txt}"
base_dir="$(cd "$(dirname "$manifest")" && pwd)"
cd "$base_dir"
sha256sum -c "$(basename "$manifest")"
```

Create `scripts/release/write-checksum-manifest.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

version="$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)"
tag="${1:-${RELEASE_TAG:-v${version}}}"
if [ "$tag" != "v${version}" ]; then
  echo "tag ${tag} does not match Cargo.toml version v${version}" >&2
  exit 1
fi
out_dir="target/release-artifacts"
manifest="${out_dir}/spacetimedb-tui-${tag}-SHA256SUMS.txt"
: > "$manifest"
found=0
while IFS= read -r -d '' archive; do
  found=1
  (cd "$out_dir" && sha256sum "$(basename "$archive")") >> "$manifest"
done < <(find "$out_dir" -maxdepth 1 -type f \( -name "spacetimedb-tui-${tag}-*.tar.gz" -o -name "spacetimedb-tui-${tag}-*.zip" \) -print0 | sort -z)
if [ "$found" -eq 0 ]; then
  echo "no release archives found for ${tag}" >&2
  exit 1
fi
cat "$manifest"
```

Create `scripts/release/make-archives.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

target="${1:?usage: make-archives.sh TARGET [TAG]}"
version="$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)"
tag="${2:-${RELEASE_TAG:-v${version}}}"
if [ "$tag" != "v${version}" ]; then
  echo "tag ${tag} does not match Cargo.toml version v${version}" >&2
  exit 1
fi
out_dir="target/release-artifacts"
stage_root="${out_dir}/stage"
archive_base="spacetimedb-tui-${tag}-${target}"
stage="${stage_root}/${archive_base}"
bin="target/${target}/release/spacetimedb-tui"
rm -rf "$stage"
mkdir -p "$stage" "$out_dir"

if [[ "$target" == *windows* ]]; then
  bin="${bin}.exe"
  test -f "$bin"
else
  test -x "$bin"
fi
cp "$bin" "$stage/"
cp README.md "$stage/"

if [[ "$target" == *windows* ]]; then
  command -v zip >/dev/null 2>&1 || { echo "zip is required in Git Bash on the Windows release runner" >&2; exit 1; }
  (cd "$stage_root" && TZ=UTC find "$archive_base" -exec touch -t 198001010000 {} + && find "$archive_base" -type f | sort | zip -X -q "../${archive_base}.zip" -@)
else
  tar --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner -C "$stage_root" -cf - "$archive_base" | gzip -n > "$out_dir/${archive_base}.tar.gz"
fi
rm -rf "$stage"
rmdir "$stage_root" 2>/dev/null || true
```

Append the Task 3 release workflow ownership that was intentionally not asserted before archives exist:

```rust
#[test]
fn release_workflow_archive_names_are_version_owned_and_scripted() {
    let workflow = std::fs::read_to_string(".github/workflows/release.yml").expect("release workflow");
    assert!(workflow.contains("needs: version"), "release workflow must derive the tag from Cargo.toml first");
    assert!(workflow.contains("spacetimedb-tui-${{ needs.version.outputs.tag }}-${{ matrix.target }}"));
    assert!(workflow.contains("make-archives.sh ${{ matrix.target }} ${{ needs.version.outputs.tag }}"));
}
```

- [ ] **Step 2: Run scripts to verify failure before release binary exists**

Run:
```bash
host=$(rustc -vV | sed -n 's/^host: //p')
bash scripts/release/make-archives.sh "$host"
```
Expected: FAIL with `target/${host}/release/spacetimedb-tui: No such file or directory` or `test -x` failure on the local host target. The workflow ownership test also fails until `release.yml` has `needs: version`, version-owned artifact names, and a `make-archives.sh` call.

- [ ] **Step 3: Build release binary and generate archive twice**

Run:
```bash
host=$(rustc -vV | sed -n 's/^host: //p')
rustup target add "$host"
cargo build --release --locked --target "$host"
rm -rf target/release-artifacts
bash scripts/release/make-archives.sh "$host"
bash scripts/release/write-checksum-manifest.sh v0.1.0
first_hash=$(sha256sum target/release-artifacts/spacetimedb-tui-v0.1.0-${host}.tar.gz | awk '{print $1}')
rm -rf target/release-artifacts
bash scripts/release/make-archives.sh "$host"
bash scripts/release/write-checksum-manifest.sh v0.1.0
second_hash=$(sha256sum target/release-artifacts/spacetimedb-tui-v0.1.0-${host}.tar.gz | awk '{print $1}')
test "$first_hash" = "$second_hash"
```
Expected: PASS. The archive hash is identical across both runs.

Run:
```bash
archive="target/release-artifacts/spacetimedb-tui-v0.1.0-${host}.tar.gz"
tar -tzf "$archive" | sed -n '1,5p'
tar -tzf "$archive" | grep -q "^spacetimedb-tui-v0.1.0-${host}/"
test ! -e target/release-artifacts/stage
bash scripts/release/write-checksum-manifest.sh v0.1.0
grep -Fx "$(sha256sum "$archive" | sed 's#target/release-artifacts/##')" target/release-artifacts/spacetimedb-tui-v0.1.0-SHA256SUMS.txt
```
Expected: PASS. The archive root is exactly `spacetimedb-tui-v0.1.0-${host}/`, no `stage/` directory remains for artifact upload, and the combined manifest contains the exact archive hash entry.

- [ ] **Step 4: Verify checksum manifest**

Run:
```bash
bash scripts/release/verify-checksums.sh target/release-artifacts/spacetimedb-tui-v0.1.0-SHA256SUMS.txt
cargo test --test release_version_truth release_workflow_archive_names_are_version_owned_and_scripted
```
Expected: PASS with `spacetimedb-tui-v0.1.0-${host}.tar.gz: OK`.

- [ ] **Step 5: Wire CI/release archive gates**

Add CI step:

```yaml
      - name: Archive checksum smoke on Ubuntu x86_64
        run: |
          rustup target add x86_64-unknown-linux-gnu
          cargo build --release --locked --target x86_64-unknown-linux-gnu
          bash scripts/release/make-archives.sh x86_64-unknown-linux-gnu
          bash scripts/release/write-checksum-manifest.sh v0.1.0
          bash scripts/release/verify-checksums.sh target/release-artifacts/spacetimedb-tui-v0.1.0-SHA256SUMS.txt
```

Use the archive script from release workflow matrix jobs after building each target. Each matrix job uploads archives only, never `stage/`. The Windows matrix job runs the archive script under Git Bash, requires `zip`, checks the Windows binary with `test -f`, and lets the publish Ubuntu job create and verify the single combined release manifest:

```yaml
      - name: Package archive
        run: bash scripts/release/make-archives.sh ${{ matrix.target }} ${{ needs.version.outputs.tag }}
```

- [ ] **Step 6: Commit**

```bash
git add scripts/release/make-archives.sh scripts/release/write-checksum-manifest.sh scripts/release/verify-checksums.sh .github/workflows/ci.yml .github/workflows/release.yml tests/release_version_truth.rs
git commit -m "build: generate reproducible release archives"
```

### Task 5: Release workflow uploads checksum manifest and complete artifact matrix

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `README.md`
- Test: `tests/release_version_truth.rs`

- [ ] **Step 1: Extend failing workflow test**

Append to `tests/release_version_truth.rs`:

```rust
#[test]
fn release_workflow_uploads_expected_artifact_matrix_readme_claims_and_checksum_manifest() {
    let workflow = std::fs::read_to_string(".github/workflows/release.yml").expect("release workflow");
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(workflow.contains(target), "release matrix missing {target}");
    }
    assert!(workflow.contains("spacetimedb-tui-${{ needs.version.outputs.tag }}-SHA256SUMS.txt"));
    assert!(workflow.contains("write-checksum-manifest.sh ${{ needs.version.outputs.tag }}"));
    assert!(workflow.contains("verify-checksums.sh target/release-artifacts/spacetimedb-tui-${{ needs.version.outputs.tag }}-SHA256SUMS.txt"));
    assert!(workflow.contains("needs: version"));
    assert!(workflow.contains("softprops/action-gh-release"));
    assert!(workflow.contains("merge-multiple: true"));
    assert!(workflow.contains("target/release-artifacts/*.tar.gz"));
    assert!(workflow.contains("target/release-artifacts/*.zip"));
    assert!(!workflow.contains("target/release-artifacts/*\n"), "release workflow must not upload stage directories");

    let readme = std::fs::read_to_string("README.md").expect("README.md");
    assert!(readme.contains("spacetimedb-tui-v0.1.0-aarch64-unknown-linux-gnu.tar.gz"), "README may claim Linux aarch64 only once this Task 5 matrix support exists");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cargo test --test release_version_truth release_workflow_uploads_expected_artifact_matrix_readme_claims_and_checksum_manifest
```
Expected: FAIL naming the first missing matrix target, checksum manifest, or upload action.

- [ ] **Step 3: Implement release workflow matrix and manifest merge**

Set release matrix and upload steps to this concrete structure, then add the Linux aarch64 artifact example to README in the same commit so the claim appears only after the matrix can publish it:

```yaml
name: Release

on:
  push:
    tags:
      - 'v*.*.*'

jobs:
  version:
    runs-on: ubuntu-latest
    outputs:
      tag: ${{ steps.version.outputs.tag }}
      version: ${{ steps.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - id: version
        shell: bash
        run: |
          version="$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)"
          tag="v${version}"
          if [ "${GITHUB_REF_NAME}" != "$tag" ]; then
            echo "release tag ${GITHUB_REF_NAME} does not match Cargo.toml version $tag" >&2
            exit 1
          fi
          echo "version=$version" >> "$GITHUB_OUTPUT"
          echo "tag=$tag" >> "$GITHUB_OUTPUT"

  build:
    needs: version
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
          - os: ubuntu-24.04-arm
            target: aarch64-unknown-linux-gnu
          - os: macos-13
            target: x86_64-apple-darwin
          - os: macos-14
            target: aarch64-apple-darwin
          - os: windows-latest
            target: x86_64-pc-windows-msvc
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --release --locked --target ${{ matrix.target }}
      - shell: bash
        run: bash scripts/release/make-archives.sh ${{ matrix.target }} ${{ needs.version.outputs.tag }}
      - uses: actions/upload-artifact@v4
        with:
          name: spacetimedb-tui-${{ needs.version.outputs.tag }}-${{ matrix.target }}
          path: |
            target/release-artifacts/*.tar.gz
            target/release-artifacts/*.zip

  publish:
    needs: [version, build]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          path: target/release-artifacts
          merge-multiple: true
      - name: Build combined checksum manifest
        run: bash scripts/release/write-checksum-manifest.sh ${{ needs.version.outputs.tag }}
      - name: Verify combined checksum manifest
        run: bash scripts/release/verify-checksums.sh target/release-artifacts/spacetimedb-tui-${{ needs.version.outputs.tag }}-SHA256SUMS.txt
      - uses: softprops/action-gh-release@v2
        with:
          files: |
            target/release-artifacts/*.tar.gz
            target/release-artifacts/*.zip
            target/release-artifacts/spacetimedb-tui-${{ needs.version.outputs.tag }}-SHA256SUMS.txt
```

- [ ] **Step 4: Run test and workflow syntax check**

Run:
```bash
cargo test --test release_version_truth release_workflow_uploads_expected_artifact_matrix_readme_claims_and_checksum_manifest
ruby -ryaml -e 'ARGV.each { |path| YAML.load_file(path); puts "#{path}: valid yaml" }' .github/workflows/ci.yml .github/workflows/release.yml
```
Expected: PASS and prints both workflow files as valid YAML using Ruby stdlib YAML, without requiring PyYAML.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml .github/workflows/ci.yml README.md tests/release_version_truth.rs
git commit -m "ci: publish release checksum manifest"
```

### Task 6: `install.sh` downloads and verifies checksum before extraction/install

**Files:**
- Modify: `scripts/install.sh`
- Modify: `README.md`
- Modify: `Cargo.toml` if `tempfile` is newly added
- Modify: `Cargo.lock` if adding `tempfile` changes the lockfile
- Test: `tests/release_installers.rs`

- [ ] **Step 1: Write failing Unix installer fixture test**

Create `tests/release_installers.rs`:

```rust
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::process::Command;
use std::thread;

fn fixture_server(files: Vec<(&'static str, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || {
        for stream in listener.incoming().take(4) {
            let mut stream = stream.expect("stream");
            let mut request = [0; 2048];
            let n = std::io::Read::read(&mut stream, &mut request).expect("read request");
            let request = String::from_utf8_lossy(&request[..n]);
            let path = request.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/");
            let name = path.rsplit('/').next().unwrap_or("");
            let body = files.iter().find(|(file, _)| *file == name).map(|(_, body)| body.clone()).unwrap_or_default();
            let status = if body.is_empty() { "404 Not Found" } else { "200 OK" };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            std::io::Write::write_all(&mut stream, &body).unwrap();
        }
    });
    format!("http://{addr}")
}

#[test]
fn install_sh_rejects_bad_checksum_before_extracting() {
    let tmp = tempfile::tempdir().expect("tmp");
    let archive = tmp.path().join("spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz");
    fs::write(&archive, b"not a tarball").expect("archive");
    let manifest = b"0000000000000000000000000000000000000000000000000000000000000000  spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz\n".to_vec();
    let base = fixture_server(vec![("spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz", fs::read(&archive).unwrap()), ("spacetimedb-tui-v0.1.0-SHA256SUMS.txt", manifest)]);

    let output = Command::new("bash")
        .arg("scripts/install.sh")
        .env("STDB_TUI_DOWNLOAD_BASE", base)
        .env("STDB_TUI_VERSION", "v0.1.0")
        .env("STDB_TUI_TARGET", "x86_64-unknown-linux-gnu")
        .env("STDB_TUI_INSTALL_DIR", tmp.path())
        .output()
        .expect("run installer");

    assert!(!output.status.success(), "installer must fail on checksum mismatch");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("checksum") || stderr.contains("FAILED"), "stderr was {stderr}");
    assert!(!tmp.path().join("spacetimedb-tui").exists(), "binary must not be installed after checksum failure");
}

#[test]
fn install_sh_rejects_missing_and_duplicate_checksum_entries_before_extracting() {
    for manifest in [
        b"".to_vec(),
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
".to_vec(),
    ] {
        let tmp = tempfile::tempdir().expect("tmp");
        let archive_name = "spacetimedb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz";
        let archive = tmp.path().join(archive_name);
        fs::write(&archive, b"not a tarball").expect("archive");
        let base = fixture_server(vec![(archive_name, fs::read(&archive).unwrap()), ("spacetimedb-tui-v0.1.0-SHA256SUMS.txt", manifest)]);
        let output = Command::new("bash")
            .arg("scripts/install.sh")
            .env("STDB_TUI_DOWNLOAD_BASE", base)
            .env("STDB_TUI_VERSION", "v0.1.0")
            .env("STDB_TUI_TARGET", "x86_64-unknown-linux-gnu")
            .env("STDB_TUI_INSTALL_DIR", tmp.path())
            .output()
            .expect("run installer");
        assert!(!output.status.success(), "installer must fail before extraction when manifest entry count is not one");
        assert!(!tmp.path().join("spacetimedb-tui").exists(), "binary must not be installed after manifest entry failure");
    }
}


#[test]
fn install_sh_preserves_public_flags_defaults_repo_and_platform_mapping() {
    let script = fs::read_to_string("scripts/install.sh").expect("install.sh");
    assert!(script.contains(r#"REPO="RazieLDG/spacetimedb-tui""#));
    assert!(script.contains("STDB_TUI_VERSION"));
    assert!(script.contains("STDB_TUI_INSTALL_DIR"));
    assert!(script.contains(r#"DEFAULT_INSTALL_DIR="${HOME}/.local/bin""#));
    assert!(script.contains("--version"));
    assert!(script.contains("--dir"));
    assert!(script.contains("-h|--help"));
    assert!(script.contains("x86_64-unknown-linux-gnu"));
    assert!(script.contains("x86_64-apple-darwin"));
    assert!(script.contains("aarch64-apple-darwin"));
    assert!(!script.contains("uname -m)-unknown-linux-gnu"), "do not replace explicit OS/arch mapping with a naive target string");
    assert!(script.contains("STDB_TUI_DOWNLOAD_BASE"));
}
```

Add `tempfile` to dev-dependencies only if it is not already present. If this changes dependencies, own both `Cargo.toml` and `Cargo.lock` in this task and stage them in the commit:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Run test to verify it fails**

Run:
```bash
cargo test --test release_installers install_sh_rejects_bad_checksum_before_extracting
cargo test --test release_installers install_sh_rejects_missing_and_duplicate_checksum_entries_before_extracting
cargo test --test release_installers install_sh_preserves_public_flags_defaults_repo_and_platform_mapping
```
Expected: FAIL because `install.sh` does not support injectable base/checksum verification or tries to extract before checksum verification.

- [ ] **Step 3: Implement checksum-first Unix installer**

Integrate checksum-first behavior into the existing `scripts/install.sh`; do not replace the installer with a minimized script. Preserve all current public behavior: `REPO="RazieLDG/spacetimedb-tui"`, `STDB_TUI_VERSION`, `STDB_TUI_INSTALL_DIR`, default `${HOME}/.local/bin`, `--version`, `--dir`, existing `--help` output, color helpers, latest-version resolution, macOS target mapping, PATH hint, smoke test, and install messages. Add only these new private/test hooks and checksum steps:

- `STDB_TUI_DOWNLOAD_BASE`, optional. When set, download `${ARCHIVE}` and `spacetimedb-tui-${VERSION}-SHA256SUMS.txt` from that base URL. When unset, keep the current GitHub URL under `https://github.com/${REPO}/releases/download/${VERSION}`.
- `STDB_TUI_TARGET`, optional. Use it only as a test override after existing OS/arch mapping. Do not replace the explicit mapping with a synthesized `uname -m` Linux target. If Linux aarch64 support is added by Task 5, extend the Linux mapping with `aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;` while keeping existing x86_64 and macOS behavior.
- Download the checksum manifest before extraction. Select exactly one manifest line whose second field equals `${ARCHIVE}`. Fail before `tar -xzf` if the entry is missing, duplicated, or the digest check fails.
- Use `sha256sum -c` when available and `shasum -a 256 -c` fallback on macOS.
- Ensure no string in the installer points to a foreign GitHub owner.

The checksum block should be inserted between archive download and extraction, equivalent to:

```bash
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "${archive}.sha256"
else
  shasum -a 256 -c "${archive}.sha256"
fi
```

- [ ] **Step 4: Run test and shell syntax**

Run:
```bash
bash -n scripts/install.sh
cargo test --test release_installers install_sh_rejects_bad_checksum_before_extracting
cargo test --test release_installers install_sh_rejects_missing_and_duplicate_checksum_entries_before_extracting
cargo test --test release_installers install_sh_preserves_public_flags_defaults_repo_and_platform_mapping
```
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add scripts/install.sh README.md tests/release_installers.rs Cargo.toml Cargo.lock
git commit -m "fix: verify unix installer checksums"
```

### Task 7: `install.ps1` downloads and verifies checksum before extraction/install

**Files:**
- Modify: `scripts/install.ps1`
- Modify: `.github/workflows/ci.yml`
- Test: `tests/release_installers.rs`

- [ ] **Step 1: Add failing PowerShell installer parser and checksum tests**

Append only the PowerShell-owned tests to `tests/release_installers.rs`; Unix checksum entry-count coverage belongs to Task 6 and must not be introduced here:

```rust
#[test]
fn install_ps1_rejects_bad_checksum_before_expanding() {
    let powershell = if Command::new("pwsh").arg("-NoLogo").arg("-NoProfile").arg("-Command").arg("$PSVersionTable.PSVersion.ToString()").output().is_ok() {
        "pwsh"
    } else {
        eprintln!("pwsh not available; parser coverage is provided by CI on Windows");
        return;
    };

    let tmp = tempfile::tempdir().expect("tmp");
    let archive_name = "spacetimedb-tui-v0.1.0-x86_64-pc-windows-msvc.zip";
    let archive_root = "spacetimedb-tui-v0.1.0-x86_64-pc-windows-msvc";
    assert_eq!(format!("{archive_root}.zip"), archive_name);
    let manifest_name = "spacetimedb-tui-v0.1.0-SHA256SUMS.txt";
    let manifest = b"0000000000000000000000000000000000000000000000000000000000000000  spacetimedb-tui-v0.1.0-x86_64-pc-windows-msvc.zip\n".to_vec();
    let base = fixture_server(vec![(archive_name, b"not a zip".to_vec()), (manifest_name, manifest)]);

    let output = Command::new(powershell)
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg("scripts/install.ps1")
        .arg("-Version")
        .arg("v0.1.0")
        .arg("-Target")
        .arg("x86_64-pc-windows-msvc")
        .arg("-DownloadBase")
        .arg(base)
        .arg("-InstallDir")
        .arg(tmp.path())
        .output()
        .expect("run powershell installer");

    assert!(!output.status.success(), "PowerShell installer must fail on checksum mismatch");
    let all = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(all.contains("checksum") || all.contains("hash"), "output was {all}");
    assert!(!tmp.path().join("spacetimedb-tui.exe").exists(), "binary must not be installed after checksum failure");
}

#[test]
fn install_ps1_preserves_public_flags_defaults_repo_and_path_behavior() {
    let script = fs::read_to_string("scripts/install.ps1").expect("install.ps1");
    assert!(script.contains("$Repo   = 'RazieLDG/spacetimedb-tui'"));
    assert!(script.contains("$env:STDB_TUI_VERSION"));
    assert!(script.contains("$env:STDB_TUI_INSTALL_DIR"));
    assert!(script.contains("LOCALAPPDATA"));
    assert!(script.contains(r"Programs\spacetimedb-tui") || script.contains(r"Programs\spacetimedb-tui"));
    assert!(script.contains("[switch]$AddToPath"));
    assert!(script.contains("SetEnvironmentVariable('Path'"));
    assert!(script.contains("x86_64-pc-windows-msvc"));
    assert!(script.contains("DownloadBase"));
}
```

- [ ] **Step 2: Run parser and test to verify failure**

Run:
```bash
pwsh -NoLogo -NoProfile -Command '$null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/install.ps1", [ref]$null, [ref]$errors); if ($errors.Count) { $errors | ForEach-Object { $_.Message }; exit 1 }'
cargo test --test release_installers install_ps1_rejects_bad_checksum_before_expanding
cargo test --test release_installers install_ps1_preserves_public_flags_defaults_repo_and_path_behavior
```
Expected: parser PASS if current script is syntactically valid, test FAIL because checksum verification/injection is not implemented.

- [ ] **Step 3: Implement checksum-first PowerShell installer**

Integrate checksum-first behavior into the existing `scripts/install.ps1`; do not replace it with a minimized script. Preserve all current public behavior: `$Repo = 'RazieLDG/spacetimedb-tui'`, `$Version = $env:STDB_TUI_VERSION`, `$InstallDir = $env:STDB_TUI_INSTALL_DIR`, default `$env:LOCALAPPDATA\Programs\spacetimedb-tui`, `$Target = 'x86_64-pc-windows-msvc'`, TLS 1.2 setup, latest-version resolution, `-AddToPath`, PATH warning, smoke test, help/comments, and `Invoke-WebRequest -UseBasicParsing` where currently used. Add only:

- Optional `[string]$DownloadBase` and `[string]$TargetOverride` or `[string]$Target` without changing the default target. Tests may pass the override, but normal users keep the current default behavior.
- When `$DownloadBase` is absent, build it from `https://github.com/$Repo/releases/download/$Version`. No hard-coded foreign owner is allowed.
- Download `spacetimedb-tui-$Version-SHA256SUMS.txt` before `Expand-Archive`. Select exactly one line for `$archiveName`, compare `Get-FileHash -Algorithm SHA256`, and fail before expansion/install on missing, duplicate, or mismatched checksum entries.
- Keep `-AddToPath` behavior and the existing default install path unchanged.

The checksum portion should be equivalent to:

```powershell
$checksumsName = "$BinName-$Version-SHA256SUMS.txt"
$checksumsPath = Join-Path $tmp $checksumsName
Invoke-WebRequest -Uri "$DownloadBase/$checksumsName" -OutFile $checksumsPath -UseBasicParsing
$matches = @(Get-Content $checksumsPath | Where-Object { ($_ -split '\s+', 2)[1] -eq $archiveName })
if ($matches.Count -ne 1) { throw "expected exactly one checksum entry for $archiveName, found $($matches.Count)" }
$expected = (($matches[0] -split '\s+')[0]).ToLowerInvariant()
$actual = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLowerInvariant()
if ($actual -ne $expected) { throw "checksum mismatch for $archiveName expected $expected got $actual" }
```

- [ ] **Step 4: Run parser and test**

Run:
```bash
pwsh -NoLogo -NoProfile -Command '$tokens=$null; $errors=$null; $null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/install.ps1", [ref]$tokens, [ref]$errors); if ($errors.Count) { $errors | ForEach-Object { $_.Message }; exit 1 }'
cargo test --test release_installers install_ps1_rejects_bad_checksum_before_expanding
cargo test --test release_installers install_ps1_preserves_public_flags_defaults_repo_and_path_behavior
```
Expected: PASS.

- [ ] **Step 5: Add CI parser step**

Add to `.github/workflows/ci.yml`:

```yaml
      - name: PowerShell installer parser
        shell: pwsh
        run: |
          $tokens=$null
          $errors=$null
          $null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/install.ps1", [ref]$tokens, [ref]$errors)
          if ($errors.Count) { $errors | ForEach-Object { $_.Message }; exit 1 }
```

- [ ] **Step 6: Commit**

```bash
git add scripts/install.ps1 .github/workflows/ci.yml tests/release_installers.rs
git commit -m "fix: verify windows installer checksums"
```

### Task 8: Fatal package binary smoke tests, package list, and cargo package smoke

**Files:**
- Create: `tests/release_package_smoke.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write failing package smoke tests**

Create `tests/release_package_smoke.rs`:

```rust
use std::process::Command;

#[test]
fn packaged_binary_has_fatal_smoke_output() {
    let exe = std::env::var("CARGO_BIN_EXE_spacetimedb-tui").expect("binary path from cargo test");
    let output = Command::new(exe)
        .arg("--help")
        .output()
        .expect("run binary help");
    assert!(output.status.success(), "--help must succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("spacetimedb"), "help should identify the product");
    assert!(stdout.contains("--host") || stdout.contains("USAGE") || stdout.contains("Usage"), "help should include CLI usage");
}

#[test]
fn cargo_package_list_contains_release_critical_files() {
    let output = Command::new("cargo")
        .args(["package", "--list", "--allow-dirty"])
        .output()
        .expect("cargo package --list");
    assert!(output.status.success(), "cargo package --list failed: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "scripts/install.sh",
        "scripts/install.ps1",
        "src/app.rs",
        "src/api/ws.rs",
        "src/ui/tabs/logs.rs",
        "images/contextual-workbench-wide.svg",
        "images/contextual-workbench-medium.svg",
        "images/contextual-workbench-compact.svg",
    ] {
        assert!(stdout.contains(path), "cargo package missing {path}");
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:
```bash
cargo test --test release_package_smoke
cargo package --locked --allow-dirty
```
Expected: FAIL if package excludes scripts/README/Cargo.lock or binary help fails. If both already pass, continue because the regression tests now lock the behavior.

- [ ] **Step 3: Implement package inclusions**

If `Cargo.toml` has `exclude` or `include` rules that omit release-critical files, set package include to this concrete list:

```toml
include = [
  "Cargo.toml",
  "Cargo.lock",
  "README.md",
  "LICENSE*",
  "src/**",
  "scripts/install.sh",
  "scripts/install.ps1",
  "images/contextual-workbench-*.svg",
  "images/stdb*.jpg",
]
```

Do not include generated archives or target outputs.

- [ ] **Step 4: Run package smoke**

Run:
```bash
cargo test --test release_package_smoke
cargo package --locked --allow-dirty
version=$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)
pkg="target/package/spacetimedb-tui-${version}.crate"
work="$(mktemp -d)"
tar -xzf "$pkg" -C "$work"
(cd "$work/spacetimedb-tui-${version}" && cargo test --all-features --locked)
```
Expected: PASS. The package archive unpacks and tests successfully.

- [ ] **Step 5: Add CI package smoke step**

Add to `.github/workflows/ci.yml`:

```yaml
      - name: Package smoke
        run: |
          cargo test --test release_package_smoke
          cargo package --locked
```

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock .github/workflows/ci.yml tests/release_package_smoke.rs
git commit -m "test: smoke packaged binary and crate"
```

### Task 9: CHANGELOG, release notes, and product-claim checklist

**Files:**
- Modify: `CHANGELOG.md`
- Create: `scripts/release/product-claim-check.sh`
- Modify: `README.md`
- Modify: `src/app/command.rs`
- Modify: `src/ui/components/help.rs`
- Modify: `src/ui/components/palette.rs`
- Modify: `src/app/command.rs`
- Modify: `src/ui/components/palette.rs`
- Modify: `src/ui/tabs/logs.rs`
- Modify: `.github/workflows/ci.yml`
- Inspect: `images/contextual-workbench-wide.svg`
- Inspect: `images/contextual-workbench-medium.svg`
- Inspect: `images/contextual-workbench-compact.svg`
- Inspect: `images/stdb*.jpg`

- [ ] **Step 1: Write failing product-claim checklist script**

Create `scripts/release/product-claim-check.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

require() {
  local pattern="$1"
  local file="$2"
  local message="$3"
  if ! grep -Eq "$pattern" "$file"; then
    echo "$message" >&2
    exit 1
  fi
}

reject() {
  local pattern="$1"
  local file="$2"
  local message="$3"
  if grep -Eq "$pattern" "$file"; then
    echo "$message" >&2
    exit 1
  fi
}

require "Phase 5|0\.1\.0" CHANGELOG.md "CHANGELOG.md must contain Phase 5 or 0.1.0 release notes"
require "checksum|SHA-256|SHA256" README.md "README.md must document checksum verification"
require "MSRV|minimum supported Rust|Rust [0-9]+\.[0-9]+\.[0-9]+" README.md "README.md must state MSRV"
require "Logs can be refreshed on demand\. Continuous log tailing is not enabled in this release\." src/ui/tabs/logs.rs "logs tab must contain exact on-demand log wording"
require "Refresh the active resource on demand\. Log views refresh on demand and are not continuously tailed in this release\." src/app/command.rs "command registry must contain exact active-resource log truth"
require "CommandRegistry|command_registry|commands\(" src/ui/components/help.rs "generated help must use the command registry rather than duplicate command text"
require "CommandRegistry|command_registry|commands\(" src/ui/components/palette.rs "palette UI must render command registry descriptions rather than duplicate command text"
require "README" CHANGELOG.md "release notes must mention README/product claim updates"
require "in-app help" CHANGELOG.md "release notes must mention in-app help review"
require "keybinding help" CHANGELOG.md "release notes must mention keybinding help review"
require "palette" CHANGELOG.md "release notes must mention palette descriptions review"
require "screenshots" CHANGELOG.md "release notes must mention screenshot review"

reject "Tail structured logs|continuous live logs|live log tailing is enabled|start live log tailing|follow logs live|unverified live logs|database-wide live feed|complete live table" README.md "README.md contains unsupported positive live-data claim"
reject "Tail structured logs|continuous live logs|live log tailing is enabled|start live log tailing|follow logs live" src/ui/tabs/logs.rs "logs tab contains unsupported positive live-tail claim"
reject "Tail structured logs|continuous live logs|live log tailing is enabled|start live log tailing|follow logs live" src/app/command.rs "command registry contains unsupported positive live-tail claim"
reject "Tail structured logs|continuous live logs|live log tailing is enabled|start live log tailing|follow logs live" src/ui/components/help.rs "generated help contains unsupported positive live-tail claim"
reject "Tail structured logs|continuous live logs|live log tailing is enabled|start live log tailing|follow logs live" src/ui/components/palette.rs "palette UI contains unsupported positive live-tail claim"

for screenshot in images/contextual-workbench-wide.svg images/contextual-workbench-medium.svg images/contextual-workbench-compact.svg; do
  if [ ! -f "$screenshot" ]; then
    echo "missing Phase 3 screenshot $screenshot" >&2
    exit 1
  fi
  require "$(basename "$screenshot")" README.md "README.md must reference $screenshot"
done
require "screenshots reviewed: images/contextual-workbench-wide.svg, images/contextual-workbench-medium.svg, images/contextual-workbench-compact.svg" CHANGELOG.md "CHANGELOG.md must state Phase 3 screenshot review"
if ls images/stdb*.jpg >/dev/null 2>&1; then
  require "legacy screenshots reviewed: images/stdb" CHANGELOG.md "CHANGELOG.md must state legacy screenshot review for images/stdb*.jpg"
fi

echo "product claims checked"
```

Add or extend Rust product-truth tests so help and palette derivation are proven without duplicating the literal command description outside `src/app/command.rs`:

```rust
#[test]
fn help_and_palette_consume_refresh_active_resource_description_from_registry() {
    let registry = CommandRegistry::default();
    let command = registry
        .by_id(CommandId::RefreshActiveResource)
        .expect("refresh active resource command");
    assert_eq!(
        command.description,
        "Refresh the active resource on demand. Log views refresh on demand and are not continuously tailed in this release."
    );

    let help_lines = registry.help_lines();
    assert!(
        help_lines
            .iter()
            .any(|line| line.command_id == command.id && line.description == command.description),
        "generated help must consume the CommandSpec description for RefreshActiveResource"
    );

    let mut palette = CommandPalette::new();
    palette.query.set("logs");
    let palette_command_ids = palette.filter();
    assert!(
        palette_command_ids.iter().any(|id| {
            registry.by_id(*id)
                .map(|spec| spec.id == command.id && spec.description == command.description)
                .unwrap_or(false)
        }),
        "palette filtering must return RefreshActiveResource and resolve its description from CommandRegistry"
    );

    let registry_entry_count = registry.iter()
        .filter(|spec| spec.id == CommandId::RefreshActiveResource)
        .count();
    assert_eq!(registry_entry_count, 1, "RefreshActiveResource must have one registry source");
}
```

- [ ] **Step 2: Run checklist to verify failure**

Run:
```bash
bash scripts/release/product-claim-check.sh
```
Expected: FAIL because the existing CHANGELOG lacks the Phase 5 release notes/product-claim section or one of the product-claim sections is missing.

- [ ] **Step 3: Implement release notes and claim checklist content**

Modify the existing `CHANGELOG.md` created by Phase 1. Preserve all existing Phase 1-4 entries. Add this exact 0.1.0/Phase 5 section above older entries if the changelog uses newest-first order, or below older entries if it uses chronological order:

```markdown
# Changelog

## 0.1.0 - 2026-07-26

### Phase 5 release hardening

- Verified product claims against connected behavior before release.
- README now states the tested MSRV, release artifact names, checksum verification, and installer behavior.
- Installers verify SHA-256 checksums before extracting or installing binaries.
- Release workflow publishes a combined `spacetimedb-tui-v0.1.0-SHA256SUMS.txt` manifest.
- Package smoke tests cover the release binary and Cargo package contents.

### Product-claim checklist

- README reviewed: install, MSRV, artifact names, checksum verification, and log behavior match the release.
- command registry reviewed: `src/app/command.rs` descriptions match implemented commands and log refresh truth.
- in-app help reviewed: generated help in `src/ui/components/help.rs` does not claim unsupported live-tail behavior.
- keybinding help reviewed: shortcut descriptions generated from the command registry match implemented commands.
- palette descriptions reviewed: `src/ui/components/palette.rs` renders command registry descriptions that match implemented behavior.
- screenshots reviewed: images/contextual-workbench-wide.svg, images/contextual-workbench-medium.svg, images/contextual-workbench-compact.svg are referenced from README and contain no stale visible claims.
- legacy screenshots reviewed: images/stdb*.jpg were checked when present for stale visible claims.
```

Ensure README, command registry, generated help, keybinding help, palette UI rendering, and log tab text use the log truth decided in Task 1 and do not claim unsupported live tailing. Ensure README references `images/contextual-workbench-wide.svg`, `images/contextual-workbench-medium.svg`, and `images/contextual-workbench-compact.svg`.

- [ ] **Step 4: Run checklist to verify pass**

Run:
```bash
bash scripts/release/product-claim-check.sh
```
Expected: PASS and prints `product claims checked`.

- [ ] **Step 5: Add CI product-claim gate**

Add to `.github/workflows/ci.yml`:

```yaml
      - name: Product claim checklist
        run: bash scripts/release/product-claim-check.sh
```

- [ ] **Step 6: Commit**

```bash
git add CHANGELOG.md README.md src/app/command.rs src/ui/components/help.rs src/ui/components/palette.rs src/ui/tabs/logs.rs scripts/release/product-claim-check.sh .github/workflows/ci.yml
git commit -m "docs: add phase 5 product truth checklist"
```

### Task 10: Final product-truth release gate

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `.github/workflows/release.yml`
- Modify: `README.md`

- [ ] **Step 1: Write the final local gate script command in CI**

Add one CI job step named `Final release truth gate` that runs this exact command block:

```yaml
      - name: Final release truth gate
        shell: bash
        run: |
          set -euo pipefail
          cargo fmt --all -- --check
          cargo clippy --all-targets --all-features --locked -- -D warnings
          cargo test --all-features --locked
          cargo build --release --locked
          cargo +$(awk -F ' = ' '$1 == "rust-version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml) check --locked --all-targets
          bash -n scripts/install.sh
          pwsh -NoLogo -NoProfile -Command '$tokens=$null; $errors=$null; $null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/install.ps1", [ref]$tokens, [ref]$errors); if ($errors.Count) { $errors | ForEach-Object { $_.Message }; exit 1 }'
          cargo package --locked
          bash scripts/release/check-msrv.sh
          bash scripts/release/validate-version.sh
          bash scripts/release/product-claim-check.sh
          version=$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)
          tag="v${version}"
          host=$(rustc -vV | sed -n 's/^host: //p')
          rustup target add "$host"
          cargo build --release --locked --target "$host"
          bash scripts/release/make-archives.sh "$host" "$tag"
          bash scripts/release/write-checksum-manifest.sh "$tag"
          bash scripts/release/verify-checksums.sh "target/release-artifacts/spacetimedb-tui-${tag}-SHA256SUMS.txt"
```

- [ ] **Step 2: Run the final gate locally to verify failure before last fixes**

Run:
```bash
set -euo pipefail
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
cargo +$(awk -F ' = ' '$1 == "rust-version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml) check --locked --all-targets
bash -n scripts/install.sh
pwsh -NoLogo -NoProfile -Command '$tokens=$null; $errors=$null; $null = [System.Management.Automation.Language.Parser]::ParseFile("scripts/install.ps1", [ref]$tokens, [ref]$errors); if ($errors.Count) { $errors | ForEach-Object { $_.Message }; exit 1 }'
cargo package --locked
bash scripts/release/check-msrv.sh
bash scripts/release/validate-version.sh
bash scripts/release/product-claim-check.sh
version=$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)
tag="v${version}"
host=$(rustc -vV | sed -n 's/^host: //p')
rustup target add "$host"
cargo build --release --locked --target "$host"
bash scripts/release/make-archives.sh "$host" "$tag"
bash scripts/release/write-checksum-manifest.sh "$tag"
bash scripts/release/verify-checksums.sh "target/release-artifacts/spacetimedb-tui-${tag}-SHA256SUMS.txt"
```
Expected: FAIL only on concrete remaining issues from prior tasks. Fix those issues in their owning files, not by weakening the gate.

- [ ] **Step 3: Implement only required fixes exposed by the final gate**

Use these allowed fix categories:

```text
- Formatting failures: run cargo fmt and commit the resulting Rust formatting.
- Clippy failures: change the named Rust code to satisfy the warning without adding #[allow] unless the lint is a documented false positive.
- Test failures: fix the behavior or fixture that failed; do not delete assertions.
- MSRV failures: either raise Cargo.toml rust-version and README/CI together, or pin the dependency causing the raise if that is a deliberate policy choice.
- Installer parser/syntax failures: fix shell or PowerShell syntax while keeping checksum-first behavior.
- Package failures: update Cargo.toml include/exclude so release-critical files are present and generated files remain absent.
- Checksum failures: fix archive names, combined manifest path, or deterministic archive script; do not skip verification.
- Product-claim failures: correct the user-facing claim to the supported on-demand behavior; do not connect `spawn_log_follow` in Phase 5 and do not loosen the checklist regex unless it is demonstrably checking the wrong file.
```

- [ ] **Step 4: Run final gate to verify pass**

Run the exact command block from Step 2 again.
Expected: PASS. Existing test count should remain at least the baseline 127 tests plus the new Phase 5 tests.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock README.md CHANGELOG.md .github/workflows/ci.yml .github/workflows/release.yml scripts/install.sh scripts/install.ps1 scripts/release/check-msrv.sh scripts/release/validate-version.sh scripts/release/make-archives.sh scripts/release/write-checksum-manifest.sh scripts/release/verify-checksums.sh scripts/release/product-claim-check.sh src/api/ws.rs src/app/command.rs src/ui/components/help.rs src/ui/components/palette.rs src/ui/tabs/logs.rs tests/release_version_truth.rs tests/release_installers.rs tests/release_package_smoke.rs images/contextual-workbench-wide.svg images/contextual-workbench-medium.svg images/contextual-workbench-compact.svg
git commit -m "ci: enforce final release truth gate"
```

## Coverage Checklist

- Phase 5 live-log truth is covered by Task 1 with a failing test that removes unsupported continuous live-tail claims and preserves on-demand log refresh wording.
- MSRV truth is covered by Task 2 with a script, README statement, Cargo metadata, and CI job.
- Version consistency across `Cargo.toml`, tags, and README examples is covered by Task 3; archive naming ownership starts in Task 4.
- Reproducible archives and the single combined SHA-256 manifest are covered by Task 4.
- Release workflow artifact matrix and combined checksum manifest upload are covered by Task 5.
- `install.sh` and `install.ps1` checksum-first behavior with local fixture HTTP/injectable download base is covered by Tasks 6 and 7.
- Fatal packaged binary smoke, `cargo package --list`, and package smoke are covered by Task 8.
- Existing CHANGELOG release notes/product-claim checklist for README, command registry, generated in-app help, keybinding help, palette descriptions, Phase 3 contextual-workbench screenshots, and legacy screenshots is covered by Task 9 while preserving Phase 1-4 entries.
- Final fmt, clippy, all tests, release build, MSRV, shell syntax, PowerShell parser, package, and checksum gates are covered by Task 10.
- Every task states concrete release behavior. Every task includes files, concrete failing validation, implementation code or exact replacement text, a pass command, and a commit command.
