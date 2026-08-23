# Releasing Cedar

Cedar releases are native Rust + GPUI artifacts. Node, React, Vite, Tauri, WebView2 bundling, and
Enigma Virtual Box are not part of the build or release chain.

## Local Windows release proof

Run the same native quality gates used by CI:

```powershell
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
```

The packager writes exactly three files to `dist-release/windows/`:

- `Cedar_<version>_windows-x64.exe`
- `Cedar_<version>_windows-x64.zip`
- `SHA256SUMS-windows.txt`

Re-run artifact verification without rebuilding:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/verify-windows-release.ps1
```

The verifier checks the exact file set, SHA-256 manifest, zip contents, executable identity, version
output, and a short launch of the packaged GPUI application.

## GitHub configuration

Windows Authenticode signing is optional until a certificate is configured:

- `WINDOWS_CODESIGN_CERTIFICATE`: base64-encoded PFX certificate
- `WINDOWS_CODESIGN_PASSWORD`: PFX password

Set the release workflow's `require_windows_signing` input to `true` when signed Windows artifacts
are mandatory. Signing happens before the zip and checksum files are created.

macOS releases are always signed and notarized. Configure these repository secrets:

- `APPLE_CERTIFICATE`: base64-encoded Developer ID Application P12 certificate
- `APPLE_CERTIFICATE_PASSWORD`: P12 password
- `APPLE_SIGNING_IDENTITY`: Developer ID Application identity
- `APPLE_ID`: notarization Apple ID
- `APPLE_PASSWORD`: app-specific Apple ID password
- `APPLE_TEAM_ID`: Apple Developer team ID
- `KEYCHAIN_PASSWORD`: optional temporary CI keychain password

The macOS job builds both Apple Silicon and Intel binaries, combines them into a universal app,
signs and notarizes the app and DMG, staples their tickets, and verifies Gatekeeper acceptance.

## Native updater contract

Cedar checks the latest published release from `oshtz/cedar`. Automatic checks are enabled by
default in release builds and can be disabled in Settings. Installation always remains a user
action.

- Windows updates use `Cedar_<version>_windows-x64.exe` and `SHA256SUMS-windows.txt`.
- macOS updates use `Cedar_<version>_macos.app.zip` and `SHA256SUMS-macos.txt`.
- Asset URLs must be HTTPS downloads from this repository's GitHub Releases.
- Downloads are limited to 512 MiB and must match both GitHub's recorded size and the exact
  SHA-256 manifest entry.
- Windows Authenticode is verified when present; an invalid signature is rejected. Unsigned
  packages remain accepted until Windows signing is mandatory for releases.
- macOS app updates must also pass `codesign` and Gatekeeper verification.
- The installer keeps the previous executable or app bundle until the replacement opens a GPUI
  window and reports healthy. If that does not happen within 20 seconds, Cedar restores and
  relaunches the previous version.

The asset names and manifests above are therefore a compatibility contract. Do not rename them or
remove the standalone Windows executable / macOS app zip from future releases.

## Cut a release

1. Update `version` in `Cargo.toml` and run the local release proof.
2. Commit and push the release-ready source to `main`.
3. Create and push a matching annotated tag:

   ```powershell
   git tag -a v0.2.0 -m "Cedar 0.2.0"
   git push origin v0.2.0
   ```

4. Wait for the Release workflow to finish.
5. Download the draft assets, verify the checksum manifests, and smoke-test Windows and macOS.
6. Install the draft from the previous public Cedar version and verify the restart reaches the new
   version without showing a terminal window.
7. Publish the draft in GitHub Releases.

A tag-triggered run deliberately leaves the GitHub Release as a draft. To build and publish in one
explicit action, manually run the Release workflow for an existing tag with `draft` disabled. The
workflow refuses to overwrite an already-published release.

## Recovery

- Before publication, fix the source, replace the unannounced tag, and rerun the workflow.
- After publication, do not replace assets or move the tag. Increment the patch version and cut a
  new release.
- If an updater installation fails, Cedar retains or restores the previous version and writes the
  reason to the platform local-data directory under `Cedar/updates/install-error.txt`. The next
  successful launch surfaces that recovery in Settings.
