# Release Process

## Status
Windows signing: PENDING — certificate not yet obtained
Apple notarization: PENDING — Developer account not yet obtained

Update this file when Phase 0.5 is complete.

## Release Artifacts

| Platform | Format | Signing Required |
|----------|--------|-----------------|
| Windows | MSI | Yes — code signing certificate |
| macOS Apple Silicon | DMG | Yes — Apple notarization |
| Linux | AppImage | GPG signing (recommended) |

## Version Format

Semantic versioning: MAJOR.MINOR.PATCH
v1 release: 1.0.0

## Release Checklist

### Pre-Release
- [ ] All Phase 5 exit criteria met
- [ ] No critical or high severity issues open
- [ ] E2E smoke test passing on all target platforms
- [ ] Release artifacts built reproducibly by CI
- [ ] All artifacts signed and notarized
- [ ] Documentation reviewed and complete

### Windows — MSI Signing
1. Build MSI via GitHub Actions
2. Sign with code signing certificate (EV)
3. Verify signature: `signtool verify /pa /v installer.msi`

### macOS — DMG Notarization
1. Build DMG via GitHub Actions
2. Submit for notarization: `xcrun notarytool submit app.dmg --wait`
3. Staple: `xcrun stapler staple app.dmg`
4. Verify: `spctl --assess --type open --context context:primary-signature app.dmg`

### Linux — AppImage
1. Build AppImage via GitHub Actions
2. GPG-sign: `gpg --detach-sign llm-toolkit.AppImage`
3. Publish signature alongside artifact

## GitHub Actions Release Workflow

Trigger: push of tag matching `v*.*.*`

Steps:
1. Build on Windows runner → produce MSI
2. Build on macOS runner → produce DMG
3. Build on Linux runner → produce AppImage
4. Sign / notarize each artifact
5. Create GitHub Release
6. Attach all artifacts

## Self-Update Policy

v1 ships no in-app update mechanism (D-04).
Users update by downloading the new release from the GitHub Releases page.
Document this in user-facing FAQ.

## External Dependencies

| Dependency | Cost | Lead Time | Status |
|------------|------|-----------|--------|
| Apple Developer Account | $99/year | 1–3 days | PENDING |
| Windows EV Code Signing Certificate | $200–500/year | 3–14 days (identity verification required) | PENDING |

Initiate both on Day 1 of Phase 0. Do not wait until Phase 4.