# Tauri iOS build

The Tauri shell lives in the `web` submodule under `web/src-tauri`.

## Local build

iOS builds only work on macOS with the full Xcode app installed.

```bash
nix-shell
qxp-ios-build --export-method development
```

Without Nix:

```bash
./scripts/build-ios-local.sh --export-method development
```

Useful environment variables:

- `APPLE_DEVELOPMENT_TEAM`: Apple Developer Team ID used by Tauri/Xcode signing.
- `QXP_RUNTIME_CONFIG_URL`: URL used by `web/scripts/sync-runtime-config.mjs` to copy runtime server/RTC settings into the packaged app.
- `QXP_SERVER_ORIGIN`: overrides the packaged API/WebSocket origin.
- `QXP_FORCE_IOS_INIT=1`: regenerates the Tauri Apple project before building.

The IPA is generated under:

```text
web/src-tauri/gen/apple/build/arm64/
```

## Development on a simulator or device

```bash
nix-shell
qxp-ios-dev
```

You can pass the Tauri iOS device/simulator arguments through the script:

```bash
./scripts/dev-ios-local.sh --open
```

## GitHub Actions signing secrets

The workflow always builds a simulator artifact on macOS. For a signed IPA, run the workflow manually with `signed=true` and configure these repository secrets:

- `APPLE_DEVELOPMENT_TEAM`: Apple Developer Team ID.
- `APPLE_CERTIFICATE_P12_BASE64`: base64 encoded Apple signing certificate in `.p12` format.
- `APPLE_CERTIFICATE_PASSWORD`: password for the `.p12` certificate.
- `APPLE_PROVISIONING_PROFILE_BASE64`: base64 encoded `.mobileprovision` profile.
- `APPLE_KEYCHAIN_PASSWORD`: optional temporary keychain password.

Example encoding commands on macOS:

```bash
base64 -i Certificates.p12 | pbcopy
base64 -i QxProtocol.mobileprovision | pbcopy
```

The signed IPA artifact is uploaded from:

```text
web/src-tauri/gen/apple/build/arm64/*.ipa
```
