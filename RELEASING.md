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
6. Publish the draft in GitHub Releases.

A tag-triggered run deliberately leaves the GitHub Release as a draft. To build and publish in one
explicit action, manually run the Release workflow for an existing tag with `draft` disabled. The
workflow refuses to overwrite an already-published release.

## Recovery

- Before publication, fix the source, replace the unannounced tag, and rerun the workflow.
- After publication, do not replace assets or move the tag. Increment the patch version and cut a
  new release.
- Cedar currently distributes complete app downloads; it does not have an in-app auto-updater.
